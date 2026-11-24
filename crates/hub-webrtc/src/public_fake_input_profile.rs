//! Offline preflight for exactly two synthetic-marker diagnostic profiles.
//! This is not an authorization, session proof, or networking configuration.
//! Both snapshots in a comparison must have been validated in this process;
//! the monotonic clock samples are not portable across hosts or serializable.

use crate::{
    ice_policy::IceMode,
    public_fake_input::{FakeInputProofScope, FakeInputTestMode},
};
use hub_identity::{pairing::VerifiedPeer, session::SessionRole, Identity};
use std::time::{Duration, Instant};

const MAX_PROFILE_SECONDS: u64 = 300;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Mode,
    Role,
    Scope,
    Expired,
    Identity,
    PairMismatch,
}

pub struct SyntheticProfile<'a> {
    pub role: SessionRole,
    pub mode: FakeInputTestMode,
    pub scope: FakeInputProofScope,
    pub local_fingerprint: &'a str,
    pub peer_fingerprint: &'a str,
}

/// A validated offline snapshot. It does not retain credentials, identity
/// secrets, given fingerprints, or capability to open a listener/connection.
pub struct ValidatedSyntheticProfile {
    role: SessionRole,
    scope: FakeInputProofScope,
    local_key: [u8; 32],
    peer_key: [u8; 32],
    issued_wall: u64,
    issued_mono: Instant,
    expires_mono: Instant,
}

impl SyntheticProfile<'_> {
    /// The local role, identity and manually verified peer must come from
    /// independently established native state, not from the profile.
    /// At validation the remaining wall-clock window must be at most 300s.
    pub fn validate(
        self,
        expected_role: SessionRole,
        local: &Identity,
        pinned_peer: &VerifiedPeer,
        now: u64,
    ) -> Result<ValidatedSyntheticProfile, Error> {
        self.validate_at(expected_role, local, pinned_peer, now, Instant::now())
    }

    fn validate_at(
        self,
        expected_role: SessionRole,
        local: &Identity,
        pinned_peer: &VerifiedPeer,
        now: u64,
        mono: Instant,
    ) -> Result<ValidatedSyntheticProfile, Error> {
        if self.mode != FakeInputTestMode::Enabled || self.scope.route.mode != IceMode::RelayOnly {
            return Err(Error::Mode);
        }
        if self.role != expected_role {
            return Err(Error::Role);
        }
        if self.scope.room.is_nil() || self.scope.generation == [0; 32] {
            return Err(Error::Scope);
        }
        let ttl = self.scope.route.expires_at.saturating_sub(now);
        if ttl == 0 {
            return Err(Error::Expired);
        }
        if ttl > MAX_PROFILE_SECONDS || self.scope.route.checked_mode(now).is_err() {
            return Err(Error::Scope);
        }
        let local = local.public();
        if !valid_fingerprint(self.local_fingerprint)
            || !valid_fingerprint(self.peer_fingerprint)
            || self.local_fingerprint != local.fingerprint
            || self.peer_fingerprint != pinned_peer.fingerprint()
            || pinned_peer
                .session_binding(
                    &local,
                    self.role,
                    *self.scope.room.as_bytes(),
                    self.scope.generation,
                )
                .is_err()
        {
            return Err(Error::Identity);
        }
        Ok(ValidatedSyntheticProfile {
            role: self.role,
            scope: self.scope,
            local_key: local.public_key,
            peer_key: *pinned_peer.public_key(),
            issued_wall: now,
            issued_mono: mono,
            expires_mono: mono
                .checked_add(Duration::from_secs(ttl))
                .ok_or(Error::Expired)?,
        })
    }
}

fn valid_fingerprint(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Supply two snapshots validated in the same process in host/viewer order.
/// Neither snapshot may be serialized or transferred to a different process.
/// Equality covers both exact routes and mirrored keys. Successful comparison
/// grants nothing and does not replace on-wire proof.
pub fn check_pair(
    host: &ValidatedSyntheticProfile,
    viewer: &ValidatedSyntheticProfile,
    now: u64,
) -> Result<(), Error> {
    check_pair_at(host, viewer, now, Instant::now())
}

fn check_pair_at(
    host: &ValidatedSyntheticProfile,
    viewer: &ValidatedSyntheticProfile,
    now: u64,
    mono: Instant,
) -> Result<(), Error> {
    if (host.role, viewer.role) != (SessionRole::Host, SessionRole::Viewer) {
        return Err(Error::Role);
    }
    for profile in [host, viewer] {
        if !(profile.issued_wall..profile.scope.route.expires_at).contains(&now)
            || !(profile.issued_mono..profile.expires_mono).contains(&mono)
        {
            return Err(Error::Expired);
        }
    }
    let a = host.scope;
    let b = viewer.scope;
    if a.room != b.room
        || a.generation != b.generation
        || a.route.mode != b.route.mode
        || a.route.wss != b.route.wss
        || a.route.ca_sha256 != b.route.ca_sha256
        || a.route.turn != b.route.turn
        || a.route.relay_ports != b.route.relay_ports
        || a.route.expires_at != b.route.expires_at
        || host.local_key != viewer.peer_key
        || host.peer_key != viewer.local_key
    {
        return Err(Error::PairMismatch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::public_video::PublicVideoScope;
    use hub_identity::pairing::PairingCandidate;
    use uuid::Uuid;

    const NOW: u64 = 50_000;

    fn make_scope() -> FakeInputProofScope {
        FakeInputProofScope {
            room: Uuid::new_v4(),
            generation: [3; 32],
            route: PublicVideoScope {
                // Address literals are offline comparison fixtures only.
                wss: "93.184.216.34:8443".parse().unwrap(),
                ca_sha256: [2; 32],
                turn: "93.184.216.34:3478".parse().unwrap(),
                relay_ports: (49152, 49200),
                mode: IceMode::RelayOnly,
                expires_at: NOW + MAX_PROFILE_SECONDS,
            },
        }
    }

    fn pin(local: &Identity, peer: &Identity) -> VerifiedPeer {
        let public = peer.public();
        let candidate = PairingCandidate::parse(&public.pairing_code()).unwrap();
        local.verify_peer(&candidate, &public.fingerprint).unwrap()
    }

    fn profile<'a>(
        scope: FakeInputProofScope,
        role: SessionRole,
        local_fingerprint: &'a str,
        peer_fingerprint: &'a str,
    ) -> SyntheticProfile<'a> {
        SyntheticProfile {
            role,
            mode: FakeInputTestMode::Enabled,
            scope,
            local_fingerprint,
            peer_fingerprint,
        }
    }

    fn validate(
        scope: FakeInputProofScope,
        role: SessionRole,
        local: &Identity,
        remote: &Identity,
        mono: Instant,
    ) -> ValidatedSyntheticProfile {
        let local_fp = local.public().fingerprint;
        let peer = pin(local, remote);
        profile(scope, role, &local_fp, peer.fingerprint())
            .validate_at(role, local, &peer, NOW, mono)
            .unwrap()
    }

    #[test]
    fn two_profiles_require_host_viewer_order_and_mirrored_keys() {
        let local = Identity::generate_ephemeral().unwrap();
        let remote = Identity::generate_ephemeral().unwrap();
        let third = Identity::generate_ephemeral().unwrap();
        let scope = make_scope();
        let mono = Instant::now();
        let a = validate(scope, SessionRole::Host, &local, &remote, mono);
        let b = validate(scope, SessionRole::Viewer, &remote, &local, mono);
        let mismatched = validate(scope, SessionRole::Viewer, &remote, &third, mono);
        assert_eq!(check_pair(&a, &b, NOW), Ok(()));
        assert_eq!(check_pair_at(&b, &a, NOW, mono), Err(Error::Role));
        assert_eq!(check_pair_at(&a, &a, NOW, mono), Err(Error::Role));
        assert_eq!(
            check_pair_at(&a, &mismatched, NOW, mono),
            Err(Error::PairMismatch)
        );
    }

    #[test]
    fn mode_scope_and_identity_must_match_independent_native_context() {
        let local = Identity::generate_ephemeral().unwrap();
        let remote = Identity::generate_ephemeral().unwrap();
        let third = Identity::generate_ephemeral().unwrap();
        let local_fp = local.public().fingerprint;
        let peer = pin(&local, &remote);
        let foreign_peer = pin(&third, &remote);
        let scope = make_scope();
        let valid = || profile(scope, SessionRole::Host, &local_fp, peer.fingerprint());
        let run = |input: SyntheticProfile<'_>| {
            input.validate(SessionRole::Host, &local, &peer, NOW).err()
        };
        assert_eq!(
            run(SyntheticProfile {
                mode: FakeInputTestMode::Disabled,
                ..valid()
            }),
            Some(Error::Mode)
        );
        assert_eq!(
            run(SyntheticProfile {
                role: SessionRole::Viewer,
                ..valid()
            }),
            Some(Error::Role)
        );
        assert_eq!(
            run(SyntheticProfile {
                local_fingerprint: peer.fingerprint(),
                peer_fingerprint: &local_fp,
                ..valid()
            }),
            Some(Error::Identity)
        );
        assert_eq!(
            run(SyntheticProfile {
                peer_fingerprint:
                    "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                ..valid()
            }),
            Some(Error::Identity)
        );
        assert_eq!(
            valid()
                .validate(SessionRole::Host, &local, &foreign_peer, NOW)
                .err(),
            Some(Error::Identity)
        );
        for (bad, why) in [
            (
                FakeInputProofScope {
                    room: Uuid::nil(),
                    ..scope
                },
                Error::Scope,
            ),
            (
                FakeInputProofScope {
                    generation: [0; 32],
                    ..scope
                },
                Error::Scope,
            ),
            (
                FakeInputProofScope {
                    route: PublicVideoScope {
                        mode: IceMode::DirectPreferred,
                        ..scope.route
                    },
                    ..scope
                },
                Error::Mode,
            ),
            (
                FakeInputProofScope {
                    route: PublicVideoScope {
                        ca_sha256: [0; 32],
                        ..scope.route
                    },
                    ..scope
                },
                Error::Scope,
            ),
            (
                FakeInputProofScope {
                    route: PublicVideoScope {
                        wss: "127.0.0.1:8443".parse().unwrap(),
                        turn: "127.0.0.1:3478".parse().unwrap(),
                        ..scope.route
                    },
                    ..scope
                },
                Error::Scope,
            ),
            (
                FakeInputProofScope {
                    route: PublicVideoScope {
                        turn: "8.8.8.8:3478".parse().unwrap(),
                        ..scope.route
                    },
                    ..scope
                },
                Error::Scope,
            ),
            (
                FakeInputProofScope {
                    route: PublicVideoScope {
                        relay_ports: (49152, 49500),
                        ..scope.route
                    },
                    ..scope
                },
                Error::Scope,
            ),
            (
                FakeInputProofScope {
                    route: PublicVideoScope {
                        expires_at: NOW,
                        ..scope.route
                    },
                    ..scope
                },
                Error::Expired,
            ),
            (
                FakeInputProofScope {
                    route: PublicVideoScope {
                        expires_at: NOW + MAX_PROFILE_SECONDS + 1,
                        ..scope.route
                    },
                    ..scope
                },
                Error::Scope,
            ),
        ] {
            assert_eq!(
                run(SyntheticProfile {
                    scope: bad,
                    ..valid()
                }),
                Some(why)
            );
        }
    }

    #[test]
    fn pair_requires_exact_room_generation_and_each_route_field() {
        let local = Identity::generate_ephemeral().unwrap();
        let remote = Identity::generate_ephemeral().unwrap();
        let scope = make_scope();
        let mono = Instant::now();
        let host = validate(scope, SessionRole::Host, &local, &remote, mono);
        let route = scope.route;
        for different in [
            FakeInputProofScope {
                room: Uuid::new_v4(),
                ..scope
            },
            FakeInputProofScope {
                generation: [4; 32],
                ..scope
            },
            FakeInputProofScope {
                route: PublicVideoScope {
                    wss: "93.184.216.34:8444".parse().unwrap(),
                    ..route
                },
                ..scope
            },
            FakeInputProofScope {
                route: PublicVideoScope {
                    ca_sha256: [3; 32],
                    ..route
                },
                ..scope
            },
            FakeInputProofScope {
                route: PublicVideoScope {
                    turn: "93.184.216.34:3479".parse().unwrap(),
                    ..route
                },
                ..scope
            },
            FakeInputProofScope {
                route: PublicVideoScope {
                    relay_ports: (49152, 49201),
                    ..route
                },
                ..scope
            },
            FakeInputProofScope {
                route: PublicVideoScope {
                    expires_at: route.expires_at - 1,
                    ..route
                },
                ..scope
            },
        ] {
            let viewer = validate(different, SessionRole::Viewer, &remote, &local, mono);
            assert_eq!(
                check_pair_at(&host, &viewer, NOW, mono),
                Err(Error::PairMismatch)
            );
        }
    }

    #[test]
    fn both_wall_and_monotonic_expiry_are_enforced() {
        let local = Identity::generate_ephemeral().unwrap();
        let remote = Identity::generate_ephemeral().unwrap();
        let scope = make_scope();
        let mono = Instant::now();
        let host = validate(scope, SessionRole::Host, &local, &remote, mono);
        let viewer = validate(scope, SessionRole::Viewer, &remote, &local, mono);
        let ttl = Duration::from_secs(MAX_PROFILE_SECONDS);
        assert_eq!(
            check_pair_at(
                &host,
                &viewer,
                scope.route.expires_at - 1,
                mono + ttl - Duration::from_secs(1)
            ),
            Ok(())
        );
        assert_eq!(
            check_pair_at(&host, &viewer, scope.route.expires_at, mono),
            Err(Error::Expired)
        );
        assert_eq!(
            check_pair_at(&host, &viewer, NOW, mono + ttl),
            Err(Error::Expired)
        );
        assert_eq!(
            check_pair_at(&host, &viewer, NOW - 1, mono),
            Err(Error::Expired)
        );
    }
}
