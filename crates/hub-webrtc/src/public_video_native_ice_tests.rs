//! Real local ICE/DTLS workers under the proof-owned UDP boundary. The reused
//! fixture maps public-looking SDP labels to exact localhost ports; it never
//! sends UDP to those labels and never contacts a TURN service.

use super::*;
use crate::{direct_policy::SelectedRoute, ice_policy::IceMode, public_video::PublicVideoScope};
use hub_identity::{pairing::PairingCandidate, session::SessionRole, Identity};
use rtc::peer_connection::transport::{RTCDtlsTransportState, RTCIceTransportState};
use std::net::{Ipv4Addr, SocketAddrV4};
use tokio::time::{sleep, timeout};
use uuid::Uuid;
use webrtc::peer_connection::{PeerConnection, RTCConfigurationBuilder};

#[path = "../../hub-rendezvous-probe/src/bin/v2_signaling/public_video_fixture_net.rs"]
mod fixture_net;
use fixture_net::{FixtureNat, FixtureSide};

const WAIT: Duration = Duration::from_secs(4);
const NOW: u64 = 50_000;
const LOOPBACK: Ipv4Addr = Ipv4Addr::LOCALHOST;

fn scope() -> PublicVideoScope {
    let label = Ipv4Addr::new(93, 184, 216, 34);
    PublicVideoScope {
        wss: SocketAddrV4::new(label, 8443),
        ca_sha256: [9; 32],
        turn: SocketAddrV4::new(label, 3478),
        relay_ports: (49152, 49200),
        mode: IceMode::DirectPreferred,
        expires_at: NOW + 120,
    }
}

fn fixture_owner<'a>(
    local: &'a Identity,
    peer: &Identity,
    nat: &FixtureNat,
    room: Uuid,
    role: SessionRole,
    side: FixtureSide,
) -> PublicVideoScopedIce<'a> {
    let expected = peer.public();
    let pin = local
        .verify_peer(
            &PairingCandidate::parse(&expected.pairing_code()).unwrap(),
            &expected.fingerprint,
        )
        .unwrap();
    let proof = PublicVideoProof::prepare(local, &pin, role, room, [3; 32], scope(), NOW).unwrap();
    let server = nat.stun_server();
    let address = server.urls[0]
        .strip_prefix("stun:")
        .unwrap()
        .parse::<SocketAddrV4>()
        .unwrap();
    assert_eq!(*address.ip(), LOOPBACK);
    PublicVideoScopedIce::new_loopback_fixture(
        proof,
        room,
        SocketAddrV4::new(LOOPBACK, 0),
        address,
        RTCConfigurationBuilder::default()
            .with_ice_servers(vec![server])
            .build(),
        nat.runtime(side),
        NOW,
    )
    .unwrap()
}

fn only_candidate(sdp: &str) -> SocketAddr {
    let candidates = crate::public_video_ice::validate(
        sdp,
        scope(),
        sdp.lines().any(|line| line == "a=sendonly"),
        NOW,
    )
    .unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].kind, crate::direct_policy::Kind::Srflx);
    SocketAddr::V4(candidates[0].address)
}

async fn wait_for_native_pair(pc: &dyn PeerConnection) {
    timeout(WAIT, async {
        loop {
            let transceivers = pc.get_transceivers().await;
            if let [video] = transceivers.as_slice() {
                if let Some(sender) = video.sender().await.unwrap() {
                    if let Some(dtls) = sender.transport().await.unwrap() {
                        let ice = dtls.ice_transport();
                        if dtls.state().await.unwrap() == RTCDtlsTransportState::Connected
                            && matches!(
                                ice.state().await.unwrap(),
                                RTCIceTransportState::Connected | RTCIceTransportState::Completed
                            )
                            && ice.get_selected_candidate_pair().await.unwrap().is_some()
                        {
                            return;
                        }
                    }
                }
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("real loopback ICE and DTLS");
}

#[tokio::test]
async fn native_scoped_owner_selects_signed_direct_pair_and_revokes_on_stop() {
    let nat = FixtureNat::start().await.unwrap();
    let host_identity = Identity::generate_ephemeral().unwrap();
    let viewer_identity = Identity::generate_ephemeral().unwrap();
    let room = Uuid::new_v4();
    let mut host = fixture_owner(
        &host_identity,
        &viewer_identity,
        &nat,
        room,
        SessionRole::Host,
        FixtureSide::Host,
    );
    let mut viewer = fixture_owner(
        &viewer_identity,
        &host_identity,
        &nat,
        room,
        SessionRole::Viewer,
        FixtureSide::Viewer,
    );
    host.start(NOW).await.unwrap();
    viewer.start(NOW).await.unwrap();

    let (offer, offer_sig) = viewer
        .local_signal(host.challenge().unwrap(), NOW)
        .await
        .unwrap();
    let viewer_endpoint = only_candidate(&offer);
    assert_eq!(
        viewer_endpoint.ip(),
        std::net::IpAddr::V4(FixtureSide::Viewer.ip())
    );
    let host_boundary = host.runtime.clone();
    assert!(!host_boundary.allows_turn_peer(viewer_endpoint));
    assert!(host_boundary
        .resolve_host(&viewer_endpoint.to_string())
        .await
        .is_err());
    host.apply_peer_signal(&offer, &offer_sig, NOW)
        .await
        .unwrap();
    assert!(host_boundary.allows_turn_peer(viewer_endpoint));
    assert!(host_boundary
        .resolve_host(&viewer_endpoint.to_string())
        .await
        .is_ok());
    let wrong_port = SocketAddr::new(viewer_endpoint.ip(), viewer_endpoint.port() ^ 1);
    assert!(!host_boundary.allows_turn_peer(wrong_port));
    assert!(host_boundary
        .resolve_host(&wrong_port.to_string())
        .await
        .is_err());

    let (answer, answer_sig) = host
        .local_signal(viewer.challenge().unwrap(), NOW)
        .await
        .unwrap();
    let host_endpoint = only_candidate(&answer);
    assert_eq!(
        host_endpoint.ip(),
        std::net::IpAddr::V4(FixtureSide::Host.ip())
    );
    viewer
        .apply_peer_signal(&answer, &answer_sig, NOW)
        .await
        .unwrap();
    wait_for_native_pair(host.pc.as_ref().unwrap().as_ref()).await;

    let path = host.selected_path(NOW).await.unwrap();
    assert_eq!(path.route, SelectedRoute::ServerReflexiveDirect);
    assert_eq!(path.path, "direct");

    host.close().await.unwrap();
    assert!(!host_boundary.allows_turn_peer(viewer_endpoint));
    assert!(host_boundary
        .resolve_host(&viewer_endpoint.to_string())
        .await
        .is_err());
    assert!(host.challenge().is_none());
    viewer.close().await.unwrap();
}

#[tokio::test]
async fn native_owner_never_admits_tampered_offer_on_active_ice_workers() {
    let nat = FixtureNat::start().await.unwrap();
    let host_identity = Identity::generate_ephemeral().unwrap();
    let viewer_identity = Identity::generate_ephemeral().unwrap();
    let room = Uuid::new_v4();
    let mut host = fixture_owner(
        &host_identity,
        &viewer_identity,
        &nat,
        room,
        SessionRole::Host,
        FixtureSide::Host,
    );
    let mut viewer = fixture_owner(
        &viewer_identity,
        &host_identity,
        &nat,
        room,
        SessionRole::Viewer,
        FixtureSide::Viewer,
    );
    host.start(NOW).await.unwrap();
    viewer.start(NOW).await.unwrap();
    let (offer, signature) = viewer
        .local_signal(host.challenge().unwrap(), NOW)
        .await
        .unwrap();
    let viewer_endpoint = only_candidate(&offer);
    let tampered = offer.replace(
        &format!(" {} typ srflx", viewer_endpoint.port()),
        &format!(" {} typ srflx", viewer_endpoint.port() ^ 1),
    );
    assert_ne!(offer, tampered);
    let tampered_endpoint = only_candidate(&tampered);
    let boundary = host.runtime.clone();
    assert!(host
        .apply_peer_signal(&tampered, &signature, NOW)
        .await
        .is_err());
    assert!(!boundary.allows_turn_peer(viewer_endpoint));
    assert!(!boundary.allows_turn_peer(tampered_endpoint));
    assert!(boundary
        .resolve_host(&viewer_endpoint.to_string())
        .await
        .is_err());
    assert!(boundary
        .resolve_host(&tampered_endpoint.to_string())
        .await
        .is_err());
    assert!(host.challenge().is_none());
    assert_eq!(
        host.apply_peer_signal(&offer, &signature, NOW).await,
        Err(CLOSED)
    );
    host.close().await.unwrap();
    viewer.close().await.unwrap();
}
