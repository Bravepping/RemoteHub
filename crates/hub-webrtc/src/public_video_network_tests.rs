//! Network admission tests never dial the public address literals.

use super::*;
use crate::ice_policy::{IceMode, IceServer, RelayTarget};
use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};

const NOW: u64 = 52_000;
const EXPIRY: u64 = NOW + 120;

fn fixture(
    der: &[u8],
) -> (
    PublicVideoScope,
    RelayGrant,
    IceConfig,
    PublicVideoWssInvitation,
) {
    let room = Uuid::new_v4();
    let scope = PublicVideoScope {
        // Labels only: tests exercise synchronous preflight and never dial.
        wss: "93.184.216.34:8443".parse().unwrap(),
        turn: "93.184.216.34:3478".parse().unwrap(),
        ca_sha256: Sha256::digest(der).into(),
        mode: IceMode::DirectPreferred,
        relay_ports: (49152, 49200),
        expires_at: EXPIRY,
    };
    let target = RelayTarget {
        local: "192.168.1.2".parse().unwrap(),
        turn: scope.turn,
        relay: *scope.turn.ip(),
        port_min: scope.relay_ports.0,
        port_max: scope.relay_ports.1,
    };
    let grant = RelayGrant::new(target, &room.to_string(), EXPIRY, true).unwrap();
    let expiry = NOW + 100;
    let config = IceConfig {
        mode: scope.mode,
        ice_servers: vec![IceServer {
            urls: vec![format!("turn:{}?transport=udp", scope.turn)],
            username: format!("{expiry}:{room}"),
            credential: "AAAAAAAAAAAAAAAAAAAAAAAAAAA=".into(),
        }],
        expires_at: expiry,
        session_id: room.to_string(),
    };
    let invite = PublicVideoWssInvitation::new(room, Role::Viewer, EXPIRY, token()).unwrap();
    (scope, grant, config, invite)
}

fn token() -> RelayToken {
    RelayToken::new("a".repeat(64)).unwrap()
}

fn ca() -> Vec<u8> {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params
        .self_signed(&KeyPair::generate().unwrap())
        .unwrap()
        .der()
        .to_vec()
}

#[test]
fn public_tcp_and_ice_share_approved_room_interface_and_expiry() {
    let der = ca();
    let (scope, grant, config, invite) = fixture(&der);
    let expected_source = grant.target().local;
    let expected_end = config.expires_at;
    let prepared = prepare_public_route(scope, &invite, &der, grant, config, NOW).unwrap();
    assert_eq!(prepared.plan.local_ip(), expected_source);
    assert_eq!(prepared.endpoint.source(), expected_source);
    assert_eq!(prepared.endpoint.relay(), scope.wss);
    assert_eq!(prepared.plan.expires_at(), expected_end);
    assert!(prepared.deadline > Instant::now());
    assert!(prepared.deadline <= Instant::now() + Duration::from_secs(expected_end - NOW));
    assert_eq!(invite.role, Role::Viewer);
}

#[test]
fn rejects_different_or_malformed_ca_even_when_profile_is_otherwise_valid() {
    let der = ca();
    let other = ca();
    let (scope, grant, config, invite) = fixture(&der);
    assert_eq!(
        prepare_public_route(scope, &invite, &other, grant, config, NOW).err(),
        Some("PUBLIC_VIDEO_WSS_CA_PIN"),
    );

    let malformed = b"not a certificate";
    let (scope, grant, config, invite) = fixture(malformed);
    assert_eq!(
        prepare_public_route(scope, &invite, malformed, grant, config, NOW).err(),
        Some("PUBLIC_VIDEO_WSS_CA_PIN"),
    );
}

#[test]
fn rejects_room_expiry_and_no_public_endpoint_before_dial() {
    let der = ca();
    let (scope, grant, config, _) = fixture(&der);
    let wrong_room =
        PublicVideoWssInvitation::new(Uuid::new_v4(), Role::Host, EXPIRY, token()).unwrap();
    assert_eq!(
        prepare_public_route(scope, &wrong_room, &der, grant, config, NOW).err(),
        Some("PUBLIC_VIDEO_ICE_GRANT_MISMATCH"),
    );

    let (scope, grant, config, mut invite) = fixture(&der);
    invite.expires_at -= 1;
    assert_eq!(
        prepare_public_route(scope, &invite, &der, grant, config, NOW).err(),
        Some("PUBLIC_VIDEO_WSS_INVITATION"),
    );

    let (mut scope, grant, config, invite) = fixture(&der);
    scope.wss = "127.0.0.1:8443".parse().unwrap();
    assert_eq!(
        prepare_public_route(scope, &invite, &der, grant, config, NOW).err(),
        Some("PUBLIC_VIDEO_SCOPE_INVALID"),
    );

    let (scope, grant, config, invite) = fixture(&der);
    assert_eq!(
        prepare_public_route(scope, &invite, &der, grant, config, EXPIRY).err(),
        Some("PUBLIC_VIDEO_SCOPE_EXPIRED"),
    );
    assert!(PublicVideoWssInvitation::new(Uuid::nil(), Role::Host, EXPIRY, token()).is_err());
}
