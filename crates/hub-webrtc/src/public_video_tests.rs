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
