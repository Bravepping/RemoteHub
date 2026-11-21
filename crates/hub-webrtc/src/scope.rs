use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct LanScope {
    pub local: Ipv4Addr,
    pub peer: Ipv4Addr,
}
impl LanScope {
    pub fn new(local: Ipv4Addr, peer: Ipv4Addr) -> Result<Self, String> {
        if !local.is_private()
            || !peer.is_private()
            || local == peer
            || [local, peer]
                .iter()
                .any(|ip| matches!(ip.octets()[3], 0 | 255))
        {
            return Err("DISTINCT_PRIVATE_IPV4_REQUIRED".into());
        }
        Ok(Self { local, peer })
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub enum NetworkScope {
    #[default]
    Loopback,
    Lan(LanScope),
}
impl NetworkScope {
    pub fn local(self) -> Ipv4Addr {
        match self {
            Self::Loopback => Ipv4Addr::LOCALHOST,
            Self::Lan(lan) => lan.local,
        }
    }
    pub fn permits_peer(self, ip: IpAddr) -> bool {
        match self {
            Self::Loopback => ip.is_loopback(),
            Self::Lan(lan) => ip == IpAddr::V4(lan.peer),
        }
    }
    pub fn is_lan(self) -> bool {
        matches!(self, Self::Lan(_))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lan_never_defaults_to_a_public_wildcard_or_same_machine_address() {
        for (local, peer) in [
            ("0.0.0.0", "192.168.10.3"),
            ("192.168.10.2", "8.8.8.8"),
            ("192.168.10.2", "192.168.10.2"),
            ("192.168.10.2", "192.168.10.255"),
        ] {
            assert!(LanScope::new(local.parse().unwrap(), peer.parse().unwrap()).is_err());
        }
        assert_eq!(NetworkScope::default().local(), Ipv4Addr::LOCALHOST);
    }
}
