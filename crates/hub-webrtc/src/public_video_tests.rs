use super::*;

const NOW: u64 = 50_000;
const OFFER: &str = "v=0\r\ns=fixture offer\r\n";
const ANSWER: &str = "v=0\r\ns=fixture answer\r\n";

fn scope() -> PublicVideoScope {
    // These literals are classified as public; tests do not perform any I/O.
    PublicVideoScope {
        wss: "93.184.216.34:8443".parse().unwrap(),
        ca_sha256: [3; 32],
        turn: "93.184.216.34:3478".parse().unwrap(),
        relay_ports: (49152, 49200),
        mode: IceMode::DirectPreferred,
        expires_at: NOW + 120,
    }
}

fn pin(local: &Identity, peer: &Identity) -> VerifiedPeer {
    local
        .verify_peer(
            &hub_identity::pairing::PairingCandidate::parse(&peer.pairing_code()).unwrap(),
            &peer.public().fingerprint,
        )
        .unwrap()
}

#[test]
fn pinned_devices_verify_one_exact_sdp_per_role_in_both_modes() {
    let host = Identity::generate_ephemeral().unwrap();
    let viewer = Identity::generate_ephemeral().unwrap();
    for mode in [IceMode::DirectPreferred, IceMode::RelayOnly] {
        let s = PublicVideoScope { mode, ..scope() };
        let room = Uuid::new_v4();
        let mut h = PublicVideoProof::prepare(
            &host,
            &pin(&host, &viewer),
            SessionRole::Host,
            room,
            [1; 32],
            s,
            NOW,
        )
        .unwrap();
        let mut v = PublicVideoProof::prepare(
            &viewer,
            &pin(&viewer, &host),
            SessionRole::Viewer,
            room,
            [1; 32],
            s,
            NOW,
        )
        .unwrap();
        let challenge = h.challenge().unwrap();
        let offer = v.seal(challenge, OFFER, NOW).unwrap();
        assert_eq!(h.open(OFFER, &offer, NOW), Ok(OFFER));
        assert!(h.challenge().is_none());
        assert_eq!(h.open(OFFER, &offer, NOW), Err(REJECTED));
        assert_eq!(v.seal(challenge, OFFER, NOW), Err(REJECTED));
        let answer = h.seal(v.challenge().unwrap(), ANSWER, NOW).unwrap();
        assert_eq!(v.open(ANSWER, &answer, NOW), Ok(ANSWER));
    }
}

#[test]
fn changing_scope_room_nonce_or_exact_sdp_rejects_and_closes_proof_slot() {
    let h = Identity::generate_ephemeral().unwrap();
    let v = Identity::generate_ephemeral().unwrap();
    let room = Uuid::new_v4();
    let s = scope();
    let cases = [
        (
            PublicVideoScope {
                mode: IceMode::RelayOnly,
                ..s
            },
            room,
            [1; 32],
            SessionRole::Viewer,
            OFFER,
        ),
        (
            PublicVideoScope {
                wss: "93.184.216.34:8444".parse().unwrap(),
                ..s
            },
            room,
            [1; 32],
            SessionRole::Viewer,
            OFFER,
        ),
        (
            PublicVideoScope {
                ca_sha256: [4; 32],
                ..s
            },
            room,
            [1; 32],
            SessionRole::Viewer,
            OFFER,
        ),
        (
            PublicVideoScope {
                turn: "93.184.216.34:3479".parse().unwrap(),
                ..s
            },
            room,
            [1; 32],
            SessionRole::Viewer,
            OFFER,
        ),
        (
            PublicVideoScope {
                relay_ports: (49152, 49201),
                ..s
            },
            room,
            [1; 32],
            SessionRole::Viewer,
            OFFER,
        ),
        (
            PublicVideoScope {
                expires_at: NOW + 119,
                ..s
            },
            room,
            [1; 32],
            SessionRole::Viewer,
            OFFER,
        ),
        (s, Uuid::new_v4(), [1; 32], SessionRole::Viewer, OFFER),
        (s, room, [2; 32], SessionRole::Viewer, OFFER),
        (
            s,
            room,
            [1; 32],
            SessionRole::Viewer,
            "v=0\ns=fixture offer\n",
        ),
    ];
    for (signed_scope, signed_room, nonce, role, received) in cases {
        let mut host =
            PublicVideoProof::prepare(&h, &pin(&h, &v), SessionRole::Host, room, [1; 32], s, NOW)
                .unwrap();
        let mut viewer = PublicVideoProof::prepare(
            &v,
            &pin(&v, &h),
            role,
            signed_room,
            nonce,
            signed_scope,
            NOW,
        )
        .unwrap();
        let signature = viewer.seal(host.challenge().unwrap(), OFFER, NOW).unwrap();
        assert_eq!(host.open(received, &signature, NOW), Err(REJECTED));
        assert!(host.challenge().is_none());
    }
}

#[test]
fn invalid_scope_wrong_local_pin_and_expiry_fail_closed() {
    let h = Identity::generate_ephemeral().unwrap();
    let v = Identity::generate_ephemeral().unwrap();
    let room = Uuid::new_v4();
    let s = scope();
    let pin_h = pin(&h, &v);
    let prepare =
        |s| PublicVideoProof::prepare(&h, &pin_h, SessionRole::Host, room, [1; 32], s, NOW);
    for invalid in [
        PublicVideoScope {
            mode: IceMode::LanPinned,
            ..s
        },
        PublicVideoScope {
            wss: "192.168.1.2:8443".parse().unwrap(),
            ..s
        },
        PublicVideoScope {
            turn: "8.8.8.8:3478".parse().unwrap(),
            ..s
        },
        PublicVideoScope {
            ca_sha256: [0; 32],
            ..s
        },
        PublicVideoScope {
            relay_ports: (49152, 49500),
            ..s
        },
    ] {
        assert_eq!(prepare(invalid).err(), Some(INVALID_SCOPE));
    }
    assert_eq!(
        prepare(PublicVideoScope {
            expires_at: NOW + 601,
            ..s
        })
        .err(),
        Some(EXPIRED)
    );
    assert_eq!(
        PublicVideoProof::prepare(&v, &pin_h, SessionRole::Host, room, [1; 32], s, NOW).err(),
        Some(IDENTITY)
    );
    let short = PublicVideoScope {
        expires_at: NOW + 1,
        ..s
    };
    let mut host = prepare(short).unwrap();
    let mut viewer = PublicVideoProof::prepare(
        &v,
        &pin(&v, &h),
        SessionRole::Viewer,
        room,
        [1; 32],
        short,
        NOW,
    )
    .unwrap();
    let signature = viewer.seal(host.challenge().unwrap(), OFFER, NOW).unwrap();
    assert_eq!(host.open(OFFER, &signature, NOW + 1), Err(EXPIRED));
}

#[test]
fn an_answer_cannot_precede_or_be_paired_with_a_different_offer() {
    let h = Identity::generate_ephemeral().unwrap();
    let v = Identity::generate_ephemeral().unwrap();
    let host_pin = pin(&h, &v);
    let viewer_pin = pin(&v, &h);
    let room = Uuid::new_v4();
    let host = || {
        PublicVideoProof::prepare(
            &h,
            &host_pin,
            SessionRole::Host,
            room,
            [1; 32],
            scope(),
            NOW,
        )
        .unwrap()
    };
    let viewer = || {
        PublicVideoProof::prepare(
            &v,
            &viewer_pin,
            SessionRole::Viewer,
            room,
            [1; 32],
            scope(),
            NOW,
        )
        .unwrap()
    };
    let mut h_early = host();
    let mut h = host();
    let mut v1 = viewer();
    let mut v2 = viewer();
    assert_eq!(
        h_early.seal(v1.challenge().unwrap(), ANSWER, NOW),
        Err(REJECTED)
    );
    let sig = v1.seal(h.challenge().unwrap(), OFFER, NOW).unwrap();
    assert_eq!(h.open(OFFER, &sig, NOW), Ok(OFFER));
    v2.seal(h_early.challenge().unwrap(), "v=0\r\ns=other\r\n", NOW)
        .unwrap();
    let answer = h.seal(v2.challenge().unwrap(), ANSWER, NOW).unwrap();
    assert_eq!(v2.open(ANSWER, &answer, NOW), Err(REJECTED));
}

fn route_fixture(
    scope: PublicVideoScope,
    room: Uuid,
) -> (crate::ice_policy::RelayTarget, IceConfig) {
    let target = crate::ice_policy::RelayTarget {
        local: "192.168.1.2".parse().unwrap(),
        turn: scope.turn,
        relay: *scope.turn.ip(),
        port_min: scope.relay_ports.0,
        port_max: scope.relay_ports.1,
    };
    let expires_at = scope.expires_at - 10;
    let config = IceConfig {
        mode: scope.mode,
        expires_at,
        session_id: room.to_string(),
        ice_servers: vec![crate::ice_policy::IceServer {
            urls: vec![format!("turn:{}?transport=udp", scope.turn)],
            username: format!("{expires_at}:{room}"),
            credential: "AAAAAAAAAAAAAAAAAAAAAAAAAAA=".into(),
        }],
    };
    (target, config)
}

#[test]
fn video_ice_plan_reuses_only_scope_turn_and_explicit_path_policy() {
    use webrtc::peer_connection::RTCIceTransportPolicy;

    let host = Identity::generate_ephemeral().unwrap();
    let viewer = Identity::generate_ephemeral().unwrap();
    let room = Uuid::new_v4();
    for mode in [IceMode::DirectPreferred, IceMode::RelayOnly] {
        let scope = PublicVideoScope { mode, ..scope() };
        let proof = PublicVideoProof::prepare(
            &host,
            &pin(&host, &viewer),
            SessionRole::Host,
            room,
            [1; 32],
            scope,
            NOW,
        )
        .unwrap();
        let (target, config) = route_fixture(scope, room);
        let local = target.local;
        let grant = RelayGrant::new(target, &room.to_string(), scope.expires_at, true).unwrap();
        let deadline = config.expires_at;
        let plan = proof.prepare_ice(grant, config, NOW).unwrap();
        assert_eq!(plan.local_ip(), local);
        assert!(!plan.host_candidates_allowed());
        assert_eq!(plan.expires_at(), deadline);
        let candidate_config = plan.rtc_configuration(NOW).unwrap();
        let policy = if mode == IceMode::DirectPreferred {
            RTCIceTransportPolicy::All
        } else {
            RTCIceTransportPolicy::Relay
        };
        assert_eq!(candidate_config.ice_transport_policy(), policy);
        let mut urls = vec![format!("turn:{}?transport=udp", scope.turn)];
        if mode == IceMode::DirectPreferred {
            urls.push(format!("stun:{}", scope.turn));
        }
        assert_eq!(
            candidate_config
                .ice_servers()
                .iter()
                .flat_map(|s| s.urls.iter())
                .collect::<Vec<_>>(),
            urls.iter().collect::<Vec<_>>()
        );
        assert!(plan.rtc_configuration(deadline).is_err());
    }
}

#[test]
fn video_ice_plan_rejects_grant_binding_and_unapproved_configuration() {
    let host = Identity::generate_ephemeral().unwrap();
    let viewer = Identity::generate_ephemeral().unwrap();
    let room = Uuid::new_v4();
    let scope = scope();
    let proof = PublicVideoProof::prepare(
        &host,
        &pin(&host, &viewer),
        SessionRole::Host,
        room,
        [1; 32],
        scope,
        NOW,
    )
    .unwrap();
    let room_text = room.to_string();
    for case in 0..10 {
        let (mut target, mut config) = route_fixture(scope, room);
        let mut grant_room = room_text.clone();
        let mut grant_expiry = scope.expires_at;
        match case {
            0 => grant_room = Uuid::new_v4().to_string(),
            1 => grant_expiry -= 1,
            2 => target.turn.set_port(target.turn.port() + 1),
            3 => target.port_min += 1,
            4 => target.relay = "1.1.1.1".parse().unwrap(),
            5 => config.mode = IceMode::RelayOnly,
            6 => config.ice_servers[0].urls[0] = format!("turn:{}?transport=tcp", scope.turn),
            7 => {
                let duplicate = config.ice_servers[0].urls[0].clone();
                config.ice_servers[0].urls.push(duplicate);
            }
            8 => {
                config.ice_servers[0].username = format!("{}:{}", config.expires_at, Uuid::new_v4())
            }
            9 => config.session_id = Uuid::new_v4().to_string(),
            _ => unreachable!(),
        }
        let grant = RelayGrant::new(target, &grant_room, grant_expiry, true).unwrap();
        let expected = if case < 5 {
            "PUBLIC_VIDEO_ICE_GRANT_MISMATCH"
        } else {
            "PUBLIC_VIDEO_ICE_CONFIG_MISMATCH"
        };
        assert_eq!(
            proof.prepare_ice(grant, config, NOW).err(),
            Some(expected),
            "case {case}"
        );
    }
}

#[cfg(feature = "v2-public-video-wss")]
#[test]
fn authenticated_public_video_sdp_with_data_channel_is_rejected_and_cannot_authorize_reply() {
    const DATA: &str = "m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\na=sctp-port:5000\r\n";
    let host = Identity::generate_ephemeral().unwrap();
    let viewer = Identity::generate_ephemeral().unwrap();
    for mode in [IceMode::DirectPreferred, IceMode::RelayOnly] {
        let s = PublicVideoScope { mode, ..scope() };
        let valid_offer = format!(
            "v=0\r\no=- 1 2 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\nm=video 9 UDP/TLS/RTP/SAVPF 102\r\nc=IN IP4 0.0.0.0\r\na=rtcp:9 IN IP4 0.0.0.0\r\na=rtcp-mux\r\na=recvonly\r\na=fingerprint:sha-256 {}\r\na=rtpmap:102 H264/90000\r\na=fmtp:102 packetization-mode=1;profile-level-id=42001f\r\na=candidate:r 1 udp 2 {} 49152 typ relay raddr 0.0.0.0 rport 0\r\n",
            ["AA"; 32].join(":"),
            s.turn.ip()
        );
        let valid_answer = valid_offer.replace("a=recvonly", "a=sendonly");
        for offer_has_data in [true, false] {
            let room = Uuid::new_v4();
            let mut h = PublicVideoProof::prepare(
                &host,
                &pin(&host, &viewer),
                SessionRole::Host,
                room,
                [1; 32],
                s,
                NOW,
            )
            .unwrap();
            let mut v = PublicVideoProof::prepare(
                &viewer,
                &pin(&viewer, &host),
                SessionRole::Viewer,
                room,
                [1; 32],
                s,
                NOW,
            )
            .unwrap();
            let host_challenge = h.challenge().unwrap();
            let viewer_challenge = v.challenge().unwrap();
            if offer_has_data {
                let with_data = valid_offer.clone() + DATA;
                // Fixture signs with a pinned identity below the media policy.
                let signature = v.seal(host_challenge, &with_data, NOW).unwrap();
                assert_eq!(
                    h.verify_video_sdp(&with_data, &signature, NOW).err(),
                    Some("PUBLIC_VIDEO_SDP_POLICY")
                );
                assert!(h.challenge().is_none());
                assert_eq!(
                    h.seal_video_sdp(viewer_challenge, &valid_answer, NOW).err(),
                    Some(REJECTED)
                );
            } else {
                let (signature, _) = v.seal_video_sdp(host_challenge, &valid_offer, NOW).unwrap();
                assert!(h.verify_video_sdp(&valid_offer, &signature, NOW).is_ok());
                let with_data = valid_answer.clone() + DATA;
                let signature = h.seal(viewer_challenge, &with_data, NOW).unwrap();
                assert_eq!(
                    v.verify_video_sdp(&with_data, &signature, NOW).err(),
                    Some("PUBLIC_VIDEO_SDP_POLICY")
                );
                assert!(v.challenge().is_none());
            }
        }
    }
}
