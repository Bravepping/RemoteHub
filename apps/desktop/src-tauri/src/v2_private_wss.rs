//! Per-process native signaling configuration; never exposed through IPC.
use hub_webrtc::authenticated_relay::{LoopbackTlsTrust, PrivateLanEndpoint, RelayToken};
use std::{
    net::{Ipv4Addr, SocketAddrV4},
    sync::Arc,
};

const CONFIG_REQUIRED: &str = "LAN_WSS_CONFIG_REQUIRED";
const MAX_CA_DER: usize = 16 * 1024;

pub(crate) struct PrivateWssSettings {
    pub relay: SocketAddrV4,
    pub token: Arc<RelayToken>,
    pub trust: Arc<LoopbackTlsTrust>,
}

impl PrivateWssSettings {
    pub(crate) fn load(source: Ipv4Addr) -> Result<Self, &'static str> {
        let relay = endpoint(source, &var("REMOTEHUB_V2_DESKTOP_RELAY")?)?;
        let ca = ca_der(&var("REMOTEHUB_V2_DESKTOP_CA_DER_HEX")?)?;
        let trust = LoopbackTlsTrust::from_ca_der(&ca).map_err(|_| CONFIG_REQUIRED)?;
        // Pass the String directly into the bounded native credential wrapper.
        let token =
            RelayToken::new(var("REMOTEHUB_V2_DESKTOP_TOKEN")?).map_err(|_| CONFIG_REQUIRED)?;
        Ok(Self {
            relay,
            token: Arc::new(token),
            trust: Arc::new(trust),
        })
    }
}

fn var(name: &str) -> Result<String, &'static str> {
    std::env::var(name).map_err(|_| CONFIG_REQUIRED)
}

fn endpoint(source: Ipv4Addr, value: &str) -> Result<SocketAddrV4, &'static str> {
    if value.len() > 21 {
        return Err(CONFIG_REQUIRED);
    }
    let relay: SocketAddrV4 = value.parse().map_err(|_| CONFIG_REQUIRED)?;
    PrivateLanEndpoint::new(source, *relay.ip(), relay.port()).map_err(|_| CONFIG_REQUIRED)?;
    Ok(relay)
}

fn ca_der(value: &str) -> Result<Vec<u8>, &'static str> {
    if value.is_empty() || value.len() > 2 * MAX_CA_DER || !value.len().is_multiple_of(2) {
        return Err(CONFIG_REQUIRED);
    }
    fn nibble(value: u8) -> Option<u8> {
        match value {
            b'0'..=b'9' => Some(value - b'0'),
            b'a'..=b'f' => Some(value - b'a' + 10),
            _ => None,
        }
    }
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let high = nibble(pair[0]).ok_or(CONFIG_REQUIRED)?;
            let low = nibble(pair[1]).ok_or(CONFIG_REQUIRED)?;
            Ok((high << 4) | low)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_parsers_require_exact_private_literals_and_bounded_canonical_cert_bytes() {
        let source = Ipv4Addr::new(192, 168, 1, 2);
        assert_eq!(endpoint(source, "192.168.1.2:48620").unwrap().port(), 48620);
        for value in [
            "127.0.0.1:48620",
            "relay.local:48620",
            "192.168.1.3:0",
            "[::1]:48620",
            "8.8.8.8:48620",
            " 192.168.1.3:48620",
        ] {
            assert_eq!(endpoint(source, value), Err(CONFIG_REQUIRED));
        }
        assert_eq!(ca_der("3000").unwrap(), [0x30, 0x00]);
        for value in ["", "Ff", "123", "ab\n0"] {
            assert_eq!(ca_der(value), Err(CONFIG_REQUIRED));
        }
        assert!(ca_der(&"aa".repeat(MAX_CA_DER + 1)).is_err());
    }
}
