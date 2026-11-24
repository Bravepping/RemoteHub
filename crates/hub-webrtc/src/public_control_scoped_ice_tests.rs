//! Native bundled SCTP + H264 negotiation over loopback mock TURN only.
use super::*;
use crate::{
    ice_policy::IceMode,
    public_video::{PublicControlProof, PublicVideoScope},
    public_video_wss::wall_seconds,
};
use hub_identity::{pairing::PairingCandidate, Identity};
use uuid::Uuid;

#[path = "public_video_turn_fixture_net.rs"]
#[allow(dead_code)]
mod local_turn;

#[tokio::test]
async fn control_and_video_share_the_same_signed_loopback_turn_path() {
    exchange(false, false, true).await;
}

#[tokio::test]
async fn immediate_control_batch_clamps_the_live_shared_boundary_once() {
    exchange(true, false, true).await;
}

#[tokio::test]
async fn control_prefers_direct_when_both_paths_are_available() {
    exchange(false, true, false).await;
}
#[tokio::test]
async fn control_automatically_uses_turn_when_direct_is_blocked() {
    exchange(false, true, true).await;
}
#[tokio::test]
async fn direct_batch_round_clamps_video_and_input_to_sixty_seconds() {
    exchange(true, true, false).await;
}
#[tokio::test]
async fn direct_batch_round_automatically_falls_back_to_turn() {
    exchange(true, true, true).await;
}

#[cfg(feature = "v2-public-control-sustained")]
#[tokio::test]
async fn sustained_signed_mock_turn_path_sets_exact_one_shot_media_deadline() {
    exchange_inner(false, false, true, true).await;
}

async fn exchange(batch: bool, automatic: bool, blocked: bool) {
    exchange_inner(batch, automatic, blocked, false).await;
}

async fn exchange_inner(batch: bool, automatic: bool, blocked: bool, sustained: bool) {
    timeout(Duration::from_secs(20), async {
        let relay = local_turn::FixtureTurn::start(15).await.unwrap();
        relay.block_direct(blocked);
        let room = Uuid::new_v4();
        let now = wall_seconds().unwrap();
        let scope = PublicVideoScope {
            wss: SocketAddrV4::new(local_turn::RELAY_LABEL, 8443),
            ca_sha256: [1; 32],
            turn: SocketAddrV4::new(local_turn::RELAY_LABEL, relay.server().port()),
            relay_ports: local_turn::RELAY_PORTS,
            mode: if automatic {
                IceMode::DirectPreferred
            } else {
                IceMode::RelayOnly
            },
            expires_at: now
                + if sustained {
                    2400
                } else if batch {
                    1800
                } else {
                    90
                },
        };
        let host_identity = Identity::generate_ephemeral().unwrap();
        let viewer_identity = Identity::generate_ephemeral().unwrap();
        let proof = |local, remote: &Identity, role| {
            let remote_pub = remote.public();
            let peer = local_identity_pin(local, &remote_pub);
            #[cfg(feature = "v2-public-control-sustained")]
            if sustained {
                return PublicControlProof::prepare_sustained(
                    local, &peer, role, room, [3; 32], scope, now,
                )
                .unwrap()
                .into_inner();
            }
            let prepare = if batch {
                PublicControlProof::prepare_immediate_batch
            } else {
                PublicControlProof::prepare
            };
            prepare(local, &peer, role, room, [3; 32], scope, now)
                .unwrap()
                .into_inner()
        };
        let make = |proof, side| {
            if automatic {
                PublicVideoScopedIce::new_loopback_fixture(
                    proof,
                    room,
                    SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, 0),
                    relay.server(),
                    webrtc::peer_connection::RTCConfigurationBuilder::default()
                        .with_ice_servers(vec![relay.combined_ice_server()])
                        .build(),
                    relay.runtime(side),
                    now,
                )
            } else {
                PublicVideoScopedIce::new_turn_loopback_fixture(
                    proof,
                    relay.server(),
                    relay.ice_server(),
                    now,
                )
            }
            .unwrap()
        };
        let mut host = make(
            proof(&host_identity, &viewer_identity, SessionRole::Host),
            local_turn::FixtureDirectSide::Host,
        );
        let mut viewer = make(
            proof(&viewer_identity, &host_identity, SessionRole::Viewer),
            local_turn::FixtureDirectSide::Viewer,
        );
        let host_challenge = host.challenge().unwrap();
        let viewer_challenge = viewer.challenge().unwrap();
        assert!(host.open_control(now).await.unwrap().is_none());
        tokio::try_join!(host.start(now), viewer.start(now)).unwrap();
        let (offer, offer_sig) = viewer.local_signal(host_challenge, now).await.unwrap();
        assert!(crate::public_video_ice::validate(&offer, scope, false, now).is_err());
        host.apply_peer_signal(&offer, &offer_sig, now)
            .await
            .unwrap();
        let (answer, answer_sig) = host.local_signal(viewer_challenge, now).await.unwrap();
        viewer
            .apply_peer_signal(&answer, &answer_sig, now)
            .await
            .unwrap();
        async fn open(ice: &mut PublicVideoScopedIce<'_>) -> PublicControlTransport {
            loop {
                if let Some(control) = ice.open_control(wall_seconds().unwrap()).await.unwrap() {
                    return control;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        let (mut host_channel, mut viewer_channel) =
            tokio::join!(open(&mut host), open(&mut viewer));
        assert!(host_channel.scope() == viewer_channel.scope());
        assert_eq!(
            host_channel.scope().viewer_key,
            viewer_identity.public().public_key
        );

        if batch {
            let host_deadline = host_channel.begin_immediate_batch_round().unwrap();
            let viewer_deadline = viewer_channel.begin_immediate_batch_round().unwrap();
            assert!(host_deadline <= std::time::Instant::now() + Duration::from_secs(60));
            assert_eq!(host_deadline, host_channel.deadline());
            assert_eq!(viewer_deadline, viewer_channel.deadline());
            assert_eq!(host_deadline, host.runtime.effective_deadline());
            assert_eq!(viewer_deadline, viewer.runtime.effective_deadline());
            assert!(host_channel.begin_immediate_batch_round().is_err());
        } else {
            assert!(host_channel.begin_immediate_batch_round().is_err());
        }

        #[cfg(feature = "v2-public-control-sustained")]
        if sustained {
            assert!(host_channel.is_sustained());
            assert!(viewer_channel.is_sustained());
            let earliest = Instant::now() + Duration::from_secs(1800);
            let host_deadline = host_channel.begin_sustained_round().unwrap();
            let viewer_deadline = viewer_channel.begin_sustained_round().unwrap();
            assert!(host_deadline >= earliest);
            assert!(viewer_deadline <= Instant::now() + Duration::from_secs(1800));
            assert!(host_channel.begin_sustained_round().is_err());
            assert!(viewer_channel.begin_sustained_round().is_err());
            assert_eq!(host_deadline, host.runtime.effective_deadline());
            assert_eq!(viewer_deadline, viewer.runtime.effective_deadline());
        }

        let host_video = host.checked_track(wall_seconds().unwrap()).await.unwrap();
        assert!(host_video.pair == host_channel.pair);
        let expected = if automatic && !blocked {
            direct_policy::SelectedRoute::ServerReflexiveDirect
        } else {
            direct_policy::SelectedRoute::TurnRelay
        };
        assert_eq!(host_video.path.route, expected);
        viewer_channel.send_text("fixture:transport").await.unwrap();
        loop {
            if let Some(packet) = host_channel.recv_text().await.unwrap() {
                assert_eq!(packet.as_ref(), b"fixture:transport");
                break;
            }
        }
        let payload = bytes::BytesMut::from(&b"\x65FIXTURE"[..]);
        tokio::try_join!(
            async {
                for sequence_number in 1..=4 {
                    host.send_rtp(
                        Packet {
                            header: rtc::rtp::header::Header {
                                version: 2,
                                payload_type: VIDEO_PT,
                                ssrc: crate::video::SSRC,
                                sequence_number,
                                timestamp: u32::from(sequence_number) * 3000,
                                marker: true,
                                ..Default::default()
                            },
                            payload: payload.clone().freeze(),
                        },
                        wall_seconds()?,
                    )
                    .await?;
                    tokio::time::sleep(Duration::from_millis(40)).await;
                }
                Ok::<_, &'static str>(())
            },
            async {
                loop {
                    if let Some((packet, path)) = viewer.recv_rtp(wall_seconds()?).await? {
                        assert_eq!(path.route, expected);
                        assert_eq!(packet.payload.as_ref(), payload.as_ref());
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                Ok::<_, &'static str>(())
            },
        )
        .unwrap();
        let viewer_video = viewer.checked_track(wall_seconds().unwrap()).await.unwrap();
        assert!(viewer_video.pair == viewer_channel.pair);
        assert_eq!(viewer_video.path.route, expected);
        if automatic && !blocked {
            assert!(relay.stats().direct_forwarded > 0);
        } else {
            assert!(relay.stats().forwarded_packets > 0);
            assert_eq!(relay.stats().direct_forwarded, 0);
            if automatic {
                assert!(relay.stats().direct_checks_dropped > 0);
            }
        }
        host.close().await.unwrap();
        assert!(host_channel.recv_text().await.is_err());
        viewer.close().await.unwrap();
    })
    .await
    .unwrap();
}

fn local_identity_pin(
    local: &Identity,
    remote: &hub_identity::PublicIdentity,
) -> hub_identity::pairing::VerifiedPeer {
    local
        .verify_peer(
            &PairingCandidate::parse(&remote.pairing_code()).unwrap(),
            &remote.fingerprint,
        )
        .unwrap()
}
