//! Match a current ICE observation to the ordered, device-approved relay
//! tuples. No RTC sessions, socket I/O, or consent state live here.
use crate::ice_policy::public_v4;
use rtc::peer_connection::transport::{
    RTCIceCandidate, RTCIceCandidatePair, RTCIceCandidateType, RTCIceProtocol,
};
use std::net::{Ipv4Addr, SocketAddrV4};

const REJECTED: &str = "PUBLIC_FAKE_INPUT_SELECTED_PAIR_REJECTED";

/// The owner supplies approved expectations in local/remote order and a
/// fresh native observation, not data from the signaling peer. A successful
/// comparison alone is not evidence that DTLS/ICE or local consent is active.
pub(crate) fn verify(
    expected_local: SocketAddrV4,
    expected_remote: SocketAddrV4,
    observed: Option<&RTCIceCandidatePair>,
) -> Result<(), &'static str> {
    let remote_ip = *expected_remote.ip();
    if !public_v4(remote_ip)
        || *expected_local.ip() != remote_ip
        || expected_local.port() < 49152
        || expected_remote.port() < 49152
        || expected_local == expected_remote
    {
        return Err(REJECTED);
    }
    let observed = observed.ok_or(REJECTED)?;
    if !matches_candidate(observed.local(), expected_local)
        || !matches_candidate(observed.remote(), expected_remote)
    {
        return Err(REJECTED);
    }
    Ok(())
}

fn matches_candidate(candidate: &RTCIceCandidate, expected: SocketAddrV4) -> bool {
    // Related addresses describe the base socket and are not selected
    // endpoints. Never compare, report, or expose that private metadata.
    candidate.typ == RTCIceCandidateType::Relay
        && candidate.protocol == RTCIceProtocol::Udp
        && candidate.component == 1
        && candidate.port == expected.port()
        && candidate.address.parse::<Ipv4Addr>() == Ok(*expected.ip())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELAY: Ipv4Addr = Ipv4Addr::new(8, 8, 8, 8);
    const LOCAL: SocketAddrV4 = SocketAddrV4::new(RELAY, 49160);
    const REMOTE: SocketAddrV4 = SocketAddrV4::new(RELAY, 49161);

    fn candidate(endpoint: SocketAddrV4) -> RTCIceCandidate {
        RTCIceCandidate {
            address: endpoint.ip().to_string(),
            port: endpoint.port(),
            typ: RTCIceCandidateType::Relay,
            protocol: RTCIceProtocol::Udp,
            component: 1,
            ..Default::default()
        }
    }

    #[test]
    fn exact_native_pair_matches_only_in_the_approved_order() {
        let mut local = candidate(LOCAL);
        local.related_address = "192.168.1.4".to_owned();
        local.related_port = 50100;
        let remote = candidate(REMOTE);
        let pair = RTCIceCandidatePair::new(local.clone(), remote.clone());
        assert_eq!(verify(LOCAL, REMOTE, Some(&pair)), Ok(()));
        assert_eq!(verify(REMOTE, LOCAL, Some(&pair)), Err(REJECTED));
        assert_eq!(
            verify(
                LOCAL,
                REMOTE,
                Some(&RTCIceCandidatePair::new(remote, local))
            ),
            Err(REJECTED)
        );
        assert_eq!(verify(LOCAL, REMOTE, None), Err(REJECTED));
    }

    #[test]
    fn wrong_tuple_or_route_metadata_fails_on_either_side() {
        for local_side in [true, false] {
            let expected = if local_side { LOCAL } else { REMOTE };
            let c = candidate(expected);
            for bad in [
                RTCIceCandidate {
                    port: 49162,
                    ..c.clone()
                },
                RTCIceCandidate {
                    address: "8.8.4.4".into(),
                    ..c.clone()
                },
                RTCIceCandidate {
                    address: "8.8.8.008".into(),
                    ..c.clone()
                },
                RTCIceCandidate {
                    address: "::ffff:8.8.8.8".into(),
                    ..c.clone()
                },
                RTCIceCandidate {
                    address: "relay.invalid".into(),
                    ..c.clone()
                },
                RTCIceCandidate {
                    typ: RTCIceCandidateType::Host,
                    ..c.clone()
                },
                RTCIceCandidate {
                    typ: RTCIceCandidateType::Srflx,
                    ..c.clone()
                },
                RTCIceCandidate {
                    protocol: RTCIceProtocol::Tcp,
                    ..c.clone()
                },
                RTCIceCandidate {
                    component: 2,
                    ..c.clone()
                },
                RTCIceCandidate::default(),
            ] {
                let pair = if local_side {
                    RTCIceCandidatePair::new(bad, candidate(REMOTE))
                } else {
                    RTCIceCandidatePair::new(candidate(LOCAL), bad)
                };
                assert_eq!(verify(LOCAL, REMOTE, Some(&pair)), Err(REJECTED));
            }
        }
    }

    #[test]
    fn invalid_expectations_fail_even_when_the_observation_matches() {
        for (local, remote) in [
            (LOCAL, LOCAL),
            (SocketAddrV4::new(RELAY, 3478), REMOTE),
            (
                LOCAL,
                SocketAddrV4::new(Ipv4Addr::new(8, 8, 4, 4), REMOTE.port()),
            ),
            (
                SocketAddrV4::new(Ipv4Addr::LOCALHOST, LOCAL.port()),
                SocketAddrV4::new(Ipv4Addr::LOCALHOST, REMOTE.port()),
            ),
        ] {
            let pair = RTCIceCandidatePair::new(candidate(local), candidate(remote));
            assert_eq!(verify(local, remote, Some(&pair)), Err(REJECTED));
        }
    }
}
