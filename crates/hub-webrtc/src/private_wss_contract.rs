use crate::{
    authenticated_relay::{AuthenticatedRelayEvent, SignalKind},
    lan_wire::{self, Codec},
};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
};

pub(super) const READY: &str = "device.wss-ready.v1";
pub(super) const ACCEPTED: &str = "device.wss-answer-accepted.v1";
pub(super) const AUTH_FAILED: &str = "LAN_WSS_AUTH_FAILED";

pub(super) async fn unexpected_tcp(socket: Option<&TcpStream>) -> String {
    let Some(socket) = socket else {
        return std::future::pending().await;
    };
    let mut byte = [0_u8; 1];
    match socket.peek(&mut byte).await {
        Ok(0) | Err(_) => "LAN_WSS_TCP_CLOSED".into(),
        Ok(_) => "LAN_WSS_TCP_UNEXPECTED".into(),
    }
}

pub(super) async fn expect_tcp(
    socket: &mut (impl AsyncRead + Unpin),
    codec: &mut Codec,
    kind: &str,
    payload: &str,
) -> Result<(), String> {
    let frame = lan_wire::read(socket)
        .await
        .map_err(|_| AUTH_FAILED.to_owned())?;
    let (received_kind, received_payload) =
        codec.accept(frame).map_err(|_| AUTH_FAILED.to_owned())?;
    if received_kind != kind || received_payload != payload {
        return Err(AUTH_FAILED.into());
    }
    Ok(())
}

pub(super) async fn send_tcp(
    socket: &mut (impl AsyncWrite + Unpin),
    codec: &mut Codec,
    kind: &str,
    payload: String,
) -> Result<(), String> {
    let frame = codec
        .encode(kind, payload)
        .map_err(|_| AUTH_FAILED.to_owned())?;
    lan_wire::write(socket, frame)
        .await
        .map_err(|_| AUTH_FAILED.to_owned())
}

pub(super) fn answer_digest(nonce: &[u8; 32], answer: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"RemoteHub.LAN.WSS.answer-accepted.v1\0");
    hash.update(nonce);
    hash.update(answer.as_bytes());
    format!("{:x}", hash.finalize())
}

pub(super) enum ExpectedRelayEvent {
    Waiting,
    Paired,
    Signal(SignalKind),
}

pub(super) fn check_relay_event(
    event: AuthenticatedRelayEvent,
    expected: ExpectedRelayEvent,
) -> Result<Option<String>, String> {
    match (event, expected) {
        (AuthenticatedRelayEvent::Waiting, ExpectedRelayEvent::Waiting)
        | (AuthenticatedRelayEvent::Paired, ExpectedRelayEvent::Paired) => Ok(None),
        (AuthenticatedRelayEvent::Signal { kind, data }, ExpectedRelayEvent::Signal(expected))
            if kind == expected =>
        {
            Ok(Some(data))
        }
        (AuthenticatedRelayEvent::PeerLeft, _) => Err("LAN_WSS_PEER_LEFT".into()),
        _ => Err(AUTH_FAILED.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lan_wire::Key;
    use std::sync::Arc;

    #[test]
    fn private_wss_rejects_unexpected_or_trickled_events() {
        assert!(check_relay_event(
            AuthenticatedRelayEvent::Waiting,
            ExpectedRelayEvent::Waiting
        )
        .unwrap()
        .is_none());
        assert_eq!(
            check_relay_event(AuthenticatedRelayEvent::Paired, ExpectedRelayEvent::Waiting)
                .unwrap_err(),
            AUTH_FAILED
        );
        let trickle = AuthenticatedRelayEvent::Signal {
            kind: SignalKind::Ice,
            data: "synthetic".into(),
        };
        assert_eq!(
            check_relay_event(trickle, ExpectedRelayEvent::Signal(SignalKind::Answer)).unwrap_err(),
            AUTH_FAILED
        );
        assert_eq!(
            check_relay_event(
                AuthenticatedRelayEvent::PeerLeft,
                ExpectedRelayEvent::Paired
            )
            .unwrap_err(),
            "LAN_WSS_PEER_LEFT"
        );
        let wrong_role = AuthenticatedRelayEvent::Signal {
            kind: SignalKind::HostChallenge,
            data: "synthetic".into(),
        };
        assert_eq!(
            check_relay_event(
                wrong_role,
                ExpectedRelayEvent::Signal(SignalKind::ViewerChallenge)
            )
            .unwrap_err(),
            AUTH_FAILED
        );
    }

    #[tokio::test]
    async fn tcp_ready_and_accepted_are_ordered_exact_and_binary_bound() {
        let (mut host, mut viewer) = tokio::io::duplex(1024);
        let key = Arc::new(Key::generate());
        let nonce = uuid::Uuid::new_v4().to_string();
        let mut host_codec = Codec::new(key.clone(), nonce.clone(), true).unwrap();
        let mut viewer_codec = Codec::new(key, nonce, false).unwrap();
        let session = [8; 32];
        let answer = answer_digest(&session, "v=0\r\n");
        assert_ne!(answer, answer_digest(&session, "v=0\n"));
        assert_ne!(answer, answer_digest(&[9; 32], "v=0\r\n"));

        send_tcp(&mut host, &mut host_codec, READY, String::new())
            .await
            .unwrap();
        expect_tcp(&mut viewer, &mut viewer_codec, READY, "")
            .await
            .unwrap();
        send_tcp(&mut viewer, &mut viewer_codec, ACCEPTED, answer.clone())
            .await
            .unwrap();
        expect_tcp(&mut host, &mut host_codec, ACCEPTED, &answer)
            .await
            .unwrap();
        send_tcp(&mut viewer, &mut viewer_codec, READY, String::new())
            .await
            .unwrap();
        assert_eq!(
            expect_tcp(&mut host, &mut host_codec, ACCEPTED, &answer)
                .await
                .unwrap_err(),
            AUTH_FAILED
        );
    }

    #[tokio::test]
    async fn closed_tcp_interrupts_pending_relay_stage_without_consuming_bytes() {
        use std::time::Duration;
        use tokio::{io::AsyncReadExt, io::AsyncWriteExt, net::TcpListener, time::timeout};

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut peer = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (mut local, _) = listener.accept().await.unwrap();
        peer.write_all(b"!").await.unwrap();
        assert_eq!(
            timeout(Duration::from_secs(1), unexpected_tcp(Some(&local)))
                .await
                .unwrap(),
            "LAN_WSS_TCP_UNEXPECTED"
        );
        assert_eq!(local.read_u8().await.unwrap(), b'!');
        drop(peer);
        assert_eq!(
            timeout(Duration::from_secs(1), unexpected_tcp(Some(&local)))
                .await
                .unwrap(),
            "LAN_WSS_TCP_CLOSED"
        );
    }
}
