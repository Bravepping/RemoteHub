use super::{Flow, Kind, Scope, MAX_FRAME_BYTES, PAIRED, PATH};
use crate::{client::dial_bound, ClientError as RelayError, LoopbackTlsTrust, Role};
use futures_util::{SinkExt, StreamExt};
use rustls::pki_types::ServerName;
use sha2::{Digest, Sha256};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddrV4},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::TcpStream,
    time::{timeout_at, Instant},
};
use tokio_rustls::{client::TlsStream, TlsConnector};
use tokio_tungstenite::{
    client_async_with_config,
    tungstenite::{
        client::IntoClientRequest,
        http::{header, HeaderValue, StatusCode},
        protocol::WebSocketConfig,
        Error as WsError, Message,
    },
    WebSocketStream,
};
use zeroize::Zeroizing;

const MAX_LEASE_SECONDS: u64 = 300;
const IO_TIMEOUT: Duration = Duration::from_secs(3);
const ROLE_HEADER: &str = "x-rh-fixture-role";
const ROOM_HEADER: &str = "x-rh-fixture-room";
const GENERATION_HEADER: &str = "x-rh-fixture-generation";
const MAX_CA_DER: usize = 64 * 1024;

type Socket = WebSocketStream<TlsStream<TcpStream>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ClientError {
    #[error("SYNTHETIC_WSS_ENDPOINT")]
    Endpoint,
    #[error("SYNTHETIC_WSS_EXPIRY")]
    Expiry,
    #[error("SYNTHETIC_WSS_INVITATION")]
    Invitation,
    #[error("SYNTHETIC_WSS_CA_PIN")]
    CaPin,
    #[error("SYNTHETIC_WSS_TLS")]
    Tls,
    #[error("SYNTHETIC_WSS_BIND")]
    Bind,
    #[error("SYNTHETIC_WSS_TRANSPORT")]
    Transport,
    #[error("SYNTHETIC_WSS_UNAUTHORIZED")]
    Unauthorized,
    #[error("SYNTHETIC_WSS_UPGRADE")]
    Upgrade,
    #[error("SYNTHETIC_WSS_FRAME")]
    Frame,
    #[error("SYNTHETIC_WSS_TIMEOUT")]
    Timeout,
    #[error("SYNTHETIC_WSS_CLOSED")]
    Closed,
}

fn wall() -> Result<u64, ClientError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|now| now.as_secs())
        .map_err(|_| ClientError::Expiry)
}

fn remaining(expiry: u64, now: u64) -> Result<Duration, ClientError> {
    let seconds = expiry.saturating_sub(now);
    if !(1..=MAX_LEASE_SECONDS).contains(&seconds) {
        return Err(ClientError::Expiry);
    }
    Ok(Duration::from_secs(seconds))
}

/// A literal, explicitly approved source and destination, limited to IPv4
/// loopback. Construct the next endpoint only after a separate approval.
pub struct SyntheticEndpoint {
    source: Ipv4Addr,
    relay: SocketAddrV4,
    expires_at: u64,
    deadline: Instant,
}

impl SyntheticEndpoint {
    /// `now` is Unix seconds when the caller accepts the short approval.
    /// Connecting and frame I/O also independently sample real time.
    pub fn new(
        source: Ipv4Addr,
        relay: SocketAddrV4,
        expires_at: u64,
        now: u64,
    ) -> Result<Self, ClientError> {
        if !source.is_loopback() || !relay.ip().is_loopback() || relay.port() == 0 {
            return Err(ClientError::Endpoint);
        }
        Ok(Self {
            source,
            relay,
            expires_at,
            deadline: Instant::now() + remaining(expires_at, now)?,
        })
    }

    fn effective_deadline(&mut self) -> Result<Instant, ClientError> {
        let now = Instant::now();
        self.deadline = self
            .deadline
            .min(now + remaining(self.expires_at, wall()?)?);
        if now >= self.deadline {
            return Err(ClientError::Expiry);
        }
        Ok(self.deadline)
    }

    fn operation_deadline(&mut self) -> Result<Instant, ClientError> {
        self.effective_deadline()
            .map(|deadline| deadline.min(Instant::now() + IO_TIMEOUT))
    }
}

/// One native credential bound to a role, room/generation and an independently
/// accepted SHA-256 digest of the DER trust anchor. Not serializable or cloneable.
pub struct SyntheticInvitation {
    scope: Scope,
    role: Role,
    bearer: Zeroizing<String>,
    ca_sha256: [u8; 32],
}

impl SyntheticInvitation {
    pub fn new(
        scope: Scope,
        role: Role,
        bearer: String,
        ca_sha256: [u8; 32],
    ) -> Result<Self, ClientError> {
        let bearer = Zeroizing::new(bearer);
        if bearer.len() != 64
            || !bearer
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
            || bearer.bytes().all(|b| b == b'0')
        {
            return Err(ClientError::Invitation);
        }
        if ca_sha256 == [0; 32] {
            return Err(ClientError::CaPin);
        }
        Ok(Self {
            scope,
            role,
            bearer,
            ca_sha256,
        })
    }
}

/// Dedicated, one-attempt loopback client. Neither the relay invitation nor
/// successful TLS grants peer identity, SCTP permission, or input consent.
pub async fn connect_loopback_wss(
    mut endpoint: SyntheticEndpoint,
    invitation: SyntheticInvitation,
    ca_der: &[u8],
) -> Result<SyntheticRelay, ClientError> {
    let deadline = endpoint.operation_deadline()?;
    if ca_der.is_empty()
        || ca_der.len() > MAX_CA_DER
        || <[u8; 32]>::from(Sha256::digest(ca_der)) != invitation.ca_sha256
    {
        return Err(ClientError::CaPin);
    }
    let trust = LoopbackTlsTrust::from_ca_der(ca_der).map_err(|_| ClientError::CaPin)?;
    let address = endpoint.relay;
    let config = WebSocketConfig::default()
        .write_buffer_size(0)
        .max_write_buffer_size(MAX_FRAME_BYTES * 2)
        .max_frame_size(Some(MAX_FRAME_BYTES))
        .max_message_size(Some(MAX_FRAME_BYTES));
    let socket =
        timeout_at(deadline, async {
            endpoint.effective_deadline()?;
            let tcp = dial_bound(endpoint.source, address, RelayError::Endpoint)
                .await
                .map_err(|error| match error {
                    RelayError::Endpoint => ClientError::Bind,
                    _ => ClientError::Transport,
                })?;
            endpoint.effective_deadline()?;
            let name = ServerName::IpAddress(IpAddr::V4(*address.ip()).into());
            let tls = TlsConnector::from(trust.0)
                .connect(name, tcp)
                .await
                .map_err(|_| ClientError::Tls)?;
            endpoint.effective_deadline()?;
            // Construct the credential-bearing HTTP request only after both the
            // verified bound TCP tuple and the verified TLS peer.
            let mut request = format!("wss://{address}{PATH}")
                .into_client_request()
                .map_err(|_| ClientError::Upgrade)?;
            let auth = Zeroizing::new(format!("Bearer {}", invitation.bearer.as_str()));
            let mut auth = HeaderValue::from_str(&auth).map_err(|_| ClientError::Invitation)?;
            auth.set_sensitive(true);
            let room = invitation.scope.room.simple().to_string();
            let generation = invitation.scope.generation.iter().fold(
                String::with_capacity(64),
                |mut hex, byte| {
                    use std::fmt::Write as _;
                    write!(hex, "{byte:02x}").expect("writing to String");
                    hex
                },
            );
            let headers = request.headers_mut();
            headers.insert(header::AUTHORIZATION, auth);
            headers.insert(
                ROLE_HEADER,
                HeaderValue::from_static(match invitation.role {
                    Role::Host => "host",
                    Role::Viewer => "viewer",
                }),
            );
            headers.insert(
                ROOM_HEADER,
                HeaderValue::from_str(&room).map_err(|_| ClientError::Upgrade)?,
            );
            headers.insert(
                GENERATION_HEADER,
                HeaderValue::from_str(&generation).map_err(|_| ClientError::Upgrade)?,
            );
            // client_async_with_config on this exact stream does not reconnect or
            // follow any redirect, even to another loopback listener.
            let (socket, _) = client_async_with_config(request, tls, Some(config))
                .await
                .map_err(|error| match error {
                    WsError::Http(response)
                        if matches!(
                            response.status(),
                            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
                        ) =>
                    {
                        ClientError::Unauthorized
                    }
                    _ => ClientError::Upgrade,
                })?;
            endpoint.effective_deadline()?;
            Ok::<_, ClientError>(socket)
        })
        .await
        .map_err(|_| ClientError::Timeout)??;
    Ok(SyntheticRelay {
        endpoint,
        socket: Some(socket),
        role: invitation.role,
        flow: Flow::new(invitation.scope),
        paired: false,
    })
}

pub struct SyntheticRelay {
    endpoint: SyntheticEndpoint,
    socket: Option<Socket>,
    role: Role,
    flow: Flow,
    paired: bool,
}

impl SyntheticRelay {
    /// Wait once for the TLS-authenticated relay to confirm both transport
    /// roles are active. This does not verify the peer or confer consent.
    pub async fn wait_paired(&mut self) -> Result<(), ClientError> {
        let mut socket = self.socket.take().ok_or(ClientError::Closed)?;
        let deadline = self.endpoint.effective_deadline()?;
        if self.paired {
            return Err(ClientError::Frame);
        }
        let frame = timeout_at(deadline, socket.next())
            .await
            .map_err(|_| ClientError::Timeout)?;
        match frame {
            Some(Ok(Message::Text(text))) if text.as_str() == PAIRED => {}
            Some(Ok(Message::Close(_))) | None => return Err(ClientError::Closed),
            Some(Ok(_)) | Some(Err(WsError::Capacity(_))) => return Err(ClientError::Frame),
            Some(Err(_)) => return Err(ClientError::Transport),
        }
        self.endpoint.effective_deadline()?;
        self.paired = true;
        self.socket = Some(socket);
        Ok(())
    }

    /// Frame bytes must already be constructed by the endpoint proof protocol.
    /// Errors and cancellation drop the socket; no resume or retry is supported.
    pub async fn send_frame(&mut self, bytes: Vec<u8>) -> Result<Kind, ClientError> {
        let mut socket = self.socket.take().ok_or(ClientError::Closed)?;
        let deadline = self.endpoint.operation_deadline()?;
        if !self.paired {
            return Err(ClientError::Frame);
        }
        let kind = self
            .flow
            .accept(self.role, &bytes)
            .map_err(|_| ClientError::Frame)?;
        timeout_at(deadline, socket.send(Message::Binary(bytes.into())))
            .await
            .map_err(|_| ClientError::Timeout)?
            .map_err(|_| ClientError::Transport)?;
        self.endpoint.effective_deadline()?;
        self.socket = Some(socket);
        Ok(kind)
    }

    /// Return only the peer's next allowed binary frame, without rewriting
    /// its bytes. The caller still verifies the end-to-end proof.
    pub async fn recv_frame(&mut self) -> Result<(Kind, Vec<u8>), ClientError> {
        let mut socket = self.socket.take().ok_or(ClientError::Closed)?;
        let deadline = self.endpoint.operation_deadline()?;
        if !self.paired {
            return Err(ClientError::Frame);
        }
        let packet = timeout_at(deadline, socket.next())
            .await
            .map_err(|_| ClientError::Timeout)?;
        let payload = match packet {
            Some(Ok(Message::Binary(payload))) => payload,
            Some(Ok(Message::Close(_))) | None => return Err(ClientError::Closed),
            Some(Ok(_)) | Some(Err(WsError::Capacity(_))) => return Err(ClientError::Frame),
            Some(Err(_)) => return Err(ClientError::Transport),
        };
        self.endpoint.effective_deadline()?;
        let kind = self
            .flow
            .accept(self.role.peer(), &payload)
            .map_err(|_| ClientError::Frame)?;
        self.socket = Some(socket);
        Ok((kind, payload.to_vec()))
    }

    pub async fn close(mut self) -> Result<(), ClientError> {
        let mut socket = self.socket.take().ok_or(ClientError::Closed)?;
        let deadline = self.endpoint.operation_deadline()?;
        timeout_at(deadline, socket.close(None))
            .await
            .map_err(|_| ClientError::Timeout)?
            .map_err(|_| ClientError::Transport)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use uuid::Uuid;

    fn invitation(pin: [u8; 32]) -> SyntheticInvitation {
        SyntheticInvitation::new(
            Scope::new(Uuid::from_bytes([7; 16]), [8; 32]).unwrap(),
            Role::Host,
            "ab".repeat(32),
            pin,
        )
        .unwrap()
    }

    #[test]
    fn endpoint_and_invitation_require_exact_loopback_and_bounded_approval() {
        let localhost = Ipv4Addr::LOCALHOST;
        let addr = SocketAddrV4::new(localhost, 9443);
        assert!(SyntheticEndpoint::new(Ipv4Addr::new(127, 0, 0, 2), addr, 60, 10).is_ok());
        assert!(matches!(
            SyntheticEndpoint::new(Ipv4Addr::UNSPECIFIED, addr, 60, 10),
            Err(ClientError::Endpoint)
        ));
        assert!(matches!(
            SyntheticEndpoint::new(
                localhost,
                SocketAddrV4::new(Ipv4Addr::new(1, 1, 1, 1), 9443),
                60,
                10
            ),
            Err(ClientError::Endpoint)
        ));
        assert!(matches!(
            SyntheticEndpoint::new(localhost, SocketAddrV4::new(localhost, 0), 60, 10),
            Err(ClientError::Endpoint)
        ));
        for (expiry, now) in [(70, 70), (69, 70), (371, 70)] {
            assert!(matches!(
                SyntheticEndpoint::new(localhost, addr, expiry, now),
                Err(ClientError::Expiry)
            ));
        }
        let scope = invitation([1; 32]).scope;
        for invalid in ["ab".repeat(31), "AB".repeat(32), "00".repeat(32)] {
            assert!(matches!(
                SyntheticInvitation::new(scope, Role::Viewer, invalid, [1; 32]),
                Err(ClientError::Invitation)
            ));
        }
        assert!(matches!(
            SyntheticInvitation::new(scope, Role::Viewer, "ab".repeat(32), [0; 32]),
            Err(ClientError::CaPin)
        ));
    }

    #[tokio::test]
    async fn bad_pin_and_expired_approval_never_dial() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let std::net::SocketAddr::V4(relay) = listener.local_addr().unwrap() else {
            panic!("IPv4 listener");
        };
        let now = wall().unwrap();
        let endpoint = SyntheticEndpoint::new(Ipv4Addr::LOCALHOST, relay, now + 10, now).unwrap();
        assert!(matches!(
            connect_loopback_wss(endpoint, invitation([1; 32]), b"not DER").await,
            Err(ClientError::CaPin)
        ));
        let expired = SyntheticEndpoint::new(Ipv4Addr::LOCALHOST, relay, now, now - 1).unwrap();
        assert!(matches!(
            connect_loopback_wss(expired, invitation([1; 32]), b"not DER").await,
            Err(ClientError::Expiry)
        ));
        assert!(
            tokio::time::timeout(Duration::from_millis(40), listener.accept())
                .await
                .is_err()
        );
    }
}
