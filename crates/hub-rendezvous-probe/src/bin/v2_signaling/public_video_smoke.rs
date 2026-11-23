//! Native signed signaling over the actual Axum TLS route. No WebRTC/media I/O.
use super::{
    tls_smoke::{fixture, make_ca, start_tls},
    *,
};
use hub_identity::{
    pairing::{PairingCandidate, VerifiedPeer},
    Identity,
};
use hub_webrtc::{
    authenticated_relay::LoopbackTlsTrust,
    ice_policy::IceMode,
    lan_wire::Key,
    public_video::PublicVideoScope,
    public_video_wss::{PublicVideoWss, PublicVideoWssConfig, PublicVideoWssEvent},
};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::time::{sleep, timeout};

const OFFER: &str = "v=0\r\ns=public fixture offer\r\n";
const ANSWER: &str = "v=0\r\ns=public fixture answer\r\n";
const WAIT: Duration = Duration::from_secs(3);

fn pin(local: &Identity, peer: &Identity) -> VerifiedPeer {
    let peer = peer.public();
    local
        .verify_peer(
            &PairingCandidate::parse(&peer.pairing_code()).unwrap(),
            &peer.fingerprint,
        )
        .unwrap()
}

async fn assert_room_released(app: &App) {
    timeout(WAIT, async {
        while !app.rooms.lock().unwrap().is_empty() || app.active.load(Ordering::Acquire) != 0 {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("server did not release ended room");
}

#[derive(Clone, Copy)]
enum Scenario {
    RoundTrip,
    WrongTlsTrust,
    OtherRouteScope,
    OtherPinnedDevice,
    DuplicateOffer,
    PeerLeavesBeforeAnswer,
}

async fn exchange(scenario: Scenario) {
    let (app, token) = fixture();
    let ca = make_ca();
    let wrong_ca = make_ca();
    let trusted_ca = if matches!(scenario, Scenario::WrongTlsTrust) {
        &wrong_ca
    } else {
        &ca
    };
    let trust = LoopbackTlsTrust::from_ca_der(trusted_ca.der().as_ref()).unwrap();
    let (address, server) = start_tls(&ca, "127.0.0.1", app.clone()).await;
    let host = Identity::generate_ephemeral().unwrap();
    let viewer = Identity::generate_ephemeral().unwrap();
    let other = Identity::generate_ephemeral().unwrap();
    let host_pin = pin(
        &host,
        if matches!(scenario, Scenario::OtherPinnedDevice) {
            &other
        } else {
            &viewer
        },
    );
    let viewer_pin = pin(&viewer, &host);

    // Checked, signed context only. All socket I/O is to the localhost TLS fixture.
    let scope = PublicVideoScope {
        wss: "93.184.216.34:8443".parse().unwrap(),
        ca_sha256: ring::digest::digest(&ring::digest::SHA256, ca.der().as_ref())
            .as_ref()
            .try_into()
            .unwrap(),
        turn: "93.184.216.34:3478".parse().unwrap(),
        relay_ports: (49152, 49200),
        mode: IceMode::DirectPreferred,
        expires_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 120,
    };
    let room = Uuid::new_v4();
    let wire_nonce = Uuid::new_v4().to_string();
    let key = Arc::new(Key::generate());
    let host_pending = PublicVideoWss::connect_loopback_wss(PublicVideoWssConfig {
        address,
        token: &token,
        room,
        role: Role::Host,
        pairing_key: key.clone(),
        wire_nonce: wire_nonce.clone(),
        session_nonce: [9; 32],
        trust: &trust,
        local_identity: &host,
        peer: &host_pin,
        scope,
    })
    .await;
    if matches!(scenario, Scenario::WrongTlsTrust) {
        assert!(matches!(host_pending, Err("PUBLIC_VIDEO_WSS_TRANSPORT")));
        assert_room_released(&app).await;
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
        return;
    }
    let host_pending = host_pending
        .unwrap()
        .wait_for_host_registration()
        .await
        .unwrap();
    let viewer_scope = if matches!(scenario, Scenario::OtherRouteScope) {
        PublicVideoScope {
            mode: IceMode::RelayOnly,
            ..scope
        }
    } else {
        scope
    };
    let viewer_pending = PublicVideoWss::connect_loopback_wss(PublicVideoWssConfig {
        address,
        token: &token,
        room,
        role: Role::Viewer,
        pairing_key: key,
        wire_nonce,
        session_nonce: [9; 32],
        trust: &trust,
        local_identity: &viewer,
        peer: &viewer_pin,
        scope: viewer_scope,
    })
    .await
    .unwrap();
    let (h, v) = tokio::join!(host_pending.establish(), viewer_pending.establish());
    let (mut h, mut v) = (h.unwrap(), v.unwrap());
    v.send_signal(SignalKind::Offer, OFFER.into())
        .await
        .unwrap();
    match scenario {
        Scenario::WrongTlsTrust => unreachable!(),
        Scenario::OtherRouteScope | Scenario::OtherPinnedDevice => {
            assert!(matches!(
                h.next_event(WAIT).await,
                Err("PUBLIC_VIDEO_PROOF_REJECTED")
            ));
            assert!(matches!(
                h.next_event(WAIT).await,
                Err("PUBLIC_VIDEO_WSS_CLOSED")
            ));
            assert!(matches!(
                v.next_event(WAIT).await.unwrap(),
                PublicVideoWssEvent::PeerLeft
            ));
        }
        Scenario::RoundTrip | Scenario::DuplicateOffer | Scenario::PeerLeavesBeforeAnswer => {
            assert!(matches!(
                h.next_event(WAIT).await.unwrap(),
                PublicVideoWssEvent::Signal { kind: SignalKind::Offer, data } if data == OFFER
            ));
            if matches!(scenario, Scenario::DuplicateOffer) {
                assert_eq!(
                    v.send_signal(SignalKind::Offer, OFFER.into()).await,
                    Err("PUBLIC_VIDEO_WSS_PROTOCOL")
                );
                assert!(matches!(
                    h.next_event(WAIT).await.unwrap(),
                    PublicVideoWssEvent::PeerLeft
                ));
            } else if matches!(scenario, Scenario::PeerLeavesBeforeAnswer) {
                drop(v);
                assert!(matches!(
                    h.next_event(WAIT).await.unwrap(),
                    PublicVideoWssEvent::PeerLeft
                ));
            } else {
                h.send_signal(SignalKind::Answer, ANSWER.into())
                    .await
                    .unwrap();
                assert!(matches!(
                    v.next_event(WAIT).await.unwrap(),
                    PublicVideoWssEvent::Signal { kind: SignalKind::Answer, data } if data == ANSWER
                ));
                h.leave().await.unwrap();
                assert!(matches!(
                    v.next_event(WAIT).await.unwrap(),
                    PublicVideoWssEvent::PeerLeft
                ));
            }
        }
    }
    assert_room_released(&app).await;
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn public_video_wss_signs_exact_offer_and_answer_then_releases_on_stop() {
    exchange(Scenario::RoundTrip).await;
}

#[tokio::test]
async fn public_video_wss_rejects_scope_mismatch_and_wrong_device_pin() {
    exchange(Scenario::OtherRouteScope).await;
    exchange(Scenario::OtherPinnedDevice).await;
}

#[tokio::test]
async fn public_video_wss_rejects_duplicate_offer_and_ends_on_peer_disconnect() {
    exchange(Scenario::DuplicateOffer).await;
    exchange(Scenario::PeerLeavesBeforeAnswer).await;
}

#[tokio::test]
async fn public_video_wss_rejects_an_untrusted_tls_ca() {
    exchange(Scenario::WrongTlsTrust).await;
}
