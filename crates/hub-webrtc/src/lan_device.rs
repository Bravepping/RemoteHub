//! Opt-in LAN device authentication before any SDP. The existing temporary
//! pairing key authenticates the transport; device proofs pin both identities.
//! This helper does not open sockets, authorize capture, or enable input.

use crate::{
    lan_wire::{self, Codec, Frame, Key},
    scope::LanScope,
};
use hub_identity::{
    pairing::VerifiedPeer,
    session::{
        PendingSessionProof, SessionBinding, SessionChallenge, SessionRole, CHALLENGE_BYTES,
    },
    Identity,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite},
    time::timeout,
};
use uuid::Uuid;

const HELLO: &str = "device.hello.v1";
const HOST_CHALLENGE: &str = "device.host-challenge.v1";
const VIEWER_PROOF: &str = "device.viewer-proof.v1";
const HOST_PROOF: &str = "device.host-proof.v1";
const ACK: &str = "device.ack.v1";
const VIDEO_ONLY: &str = "video-only";
const MAX_AUTH_FRAME: usize = 1024;
const PAIR_BYTES: usize = CHALLENGE_BYTES * 2;
const MAX_SDP: usize = 32 * 1024;
const MAX_SIGNED_SDP: usize = 64 * 1024;
const PROOF_TTL: Duration = Duration::from_secs(15);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) async fn authenticate_host<'a>(
    io: &mut (impl AsyncRead + AsyncWrite + Unpin),
    key: Arc<Key>,
    identity: &'a Identity,
    peer: &VerifiedPeer,
    scope: LanScope,
    host_port: u16,
) -> Result<(Codec, VerifiedSdp<'a>), String> {
    with_deadline(async {
        let endpoint = Endpoint::new(scope, host_port, SessionRole::Host)?;
        let first = read_auth_frame(io).await?;
        let viewer_nonce = checked_nonce(&first.nonce)?;
        let mut codec = Codec::new(key.clone(), first.nonce.clone(), true)?;
        let (kind, payload) = codec.accept(first)?;
        if kind != HELLO || payload != VIDEO_ONLY {
            return Err("LAN_DEVICE_HELLO_REQUIRED".into());
        }
        let context = Context::new(
            endpoint,
            viewer_nonce,
            Uuid::new_v4(),
            identity,
            peer,
            SessionRole::Host,
        )?;
        let [mut expected_viewer, expected_offer] =
            issue_pair(&context.binding, SessionRole::Viewer)?;
        let host_challenges = Challenges([expected_viewer.challenge(), expected_offer.challenge()]);

        send(
            io,
            &mut codec,
            HOST_CHALLENGE,
            hex_bytes(&[context.host_nonce.as_bytes(), &host_challenges.bytes()]),
        )
        .await?;

        let proposal = receive(io, &mut codec, VIEWER_PROOF).await?;
        let proposal = from_hex::<{ PAIR_BYTES + 64 }>(&proposal)?;
        let (viewer_challenges, viewer_signature) = proposal.split_at(PAIR_BYTES);
        let viewer_challenges = Challenges::parse(viewer_challenges)?;
        let viewer_signature = signature(viewer_signature)?;
        let proof_data = context.proof_data(host_challenges, viewer_challenges);
        expected_viewer
            .verify(&viewer_signature, &proof_data, wall_seconds()?)
            .map_err(|e| e.to_string())?;

        let host_signature = identity
            .sign_session_proof(
                &context.binding,
                SessionRole::Host,
                viewer_challenges.0[0],
                &proof_data,
                wall_seconds()?,
            )
            .map_err(|e| e.to_string())?;
        send(io, &mut codec, HOST_PROOF, hex_bytes(&[&host_signature])).await?;

        let acknowledged = receive(io, &mut codec, ACK).await?;
        if from_hex::<32>(&acknowledged)? != acknowledgment(&host_signature, &proof_data) {
            return Err("LAN_DEVICE_ACK_MISMATCH".into());
        }
        let next_codec = context.next_codec(key, &proof_data, true)?;
        Ok((
            next_codec,
            VerifiedSdp {
                identity,
                binding: context.binding,
                role: SessionRole::Host,
                handshake: proof_data,
                outgoing: Some(viewer_challenges.0[1]),
                incoming: Some(expected_offer),
                offer_hash: None,
            },
        ))
    })
    .await
}

pub(crate) async fn authenticate_viewer<'a>(
    io: &mut (impl AsyncRead + AsyncWrite + Unpin),
    key: Arc<Key>,
    identity: &'a Identity,
    peer: &VerifiedPeer,
    scope: LanScope,
    host_port: u16,
) -> Result<(Codec, VerifiedSdp<'a>), String> {
    with_deadline(async {
        let endpoint = Endpoint::new(scope, host_port, SessionRole::Viewer)?;
        let viewer_nonce = Uuid::new_v4();
        let mut codec = Codec::new(key.clone(), viewer_nonce.to_string(), false)?;
        send(io, &mut codec, HELLO, VIDEO_ONLY.into()).await?;
        let challenge = receive(io, &mut codec, HOST_CHALLENGE).await?;
        let challenge = from_hex::<{ 16 + PAIR_BYTES }>(&challenge)?;
        let (host_nonce, host_challenges) = challenge.split_at(16);
        let host_nonce = Uuid::from_slice(host_nonce).map_err(|_| "LAN_DEVICE_NONCE_INVALID")?;
        if host_nonce.get_version() != Some(uuid::Version::Random) || host_nonce == viewer_nonce {
            return Err("LAN_DEVICE_NONCE_INVALID".into());
        }
        let context = Context::new(
            endpoint,
            viewer_nonce,
            host_nonce,
            identity,
            peer,
            SessionRole::Viewer,
        )?;
        let host_challenges = Challenges::parse(host_challenges)?;
        let [mut expected_host, expected_answer] = issue_pair(&context.binding, SessionRole::Host)?;
        let viewer_challenges =
            Challenges([expected_host.challenge(), expected_answer.challenge()]);
        let proof_data = context.proof_data(host_challenges, viewer_challenges);
        let viewer_signature = identity
            .sign_session_proof(
                &context.binding,
                SessionRole::Viewer,
                host_challenges.0[0],
                &proof_data,
                wall_seconds()?,
            )
            .map_err(|e| e.to_string())?;
        send(
            io,
            &mut codec,
            VIEWER_PROOF,
            hex_bytes(&[&viewer_challenges.bytes(), &viewer_signature]),
        )
        .await?;

        let host_signature = from_hex::<64>(&receive(io, &mut codec, HOST_PROOF).await?)?;
        expected_host
            .verify(&host_signature, &proof_data, wall_seconds()?)
            .map_err(|e| e.to_string())?;
        send(
            io,
            &mut codec,
            ACK,
            hex_bytes(&[&acknowledgment(&host_signature, &proof_data)]),
        )
        .await?;
        let next_codec = context.next_codec(key, &proof_data, false)?;
        Ok((
            next_codec,
            VerifiedSdp {
                identity,
                binding: context.binding,
                role: SessionRole::Viewer,
                handshake: proof_data,
                outgoing: Some(host_challenges.0[1]),
                incoming: Some(expected_answer),
                offer_hash: None,
            },
        ))
    })
    .await
}

/// The second challenge in each direction is reserved exclusively for one
/// exact media description. Authenticate the SDP before using it with WebRTC.
pub(crate) struct VerifiedSdp<'a> {
    identity: &'a Identity,
    binding: SessionBinding,
    role: SessionRole,
    handshake: [u8; 32],
    outgoing: Option<SessionChallenge>,
    incoming: Option<PendingSessionProof>,
    offer_hash: Option<[u8; 32]>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedSdp {
    sdp: String,
    signature: String,
}

impl VerifiedSdp<'_> {
    /// Seal only the next offer (viewer) or answer (host).
    pub(crate) fn seal(&mut self, sdp: String) -> Result<String, String> {
        let challenge = self.outgoing.take().ok_or("LAN_DEVICE_SDP_ORDER")?;
        let statement = self.sdp_statement(self.role, &sdp)?;
        let signature = self
            .identity
            .sign_session_proof(
                &self.binding,
                self.role,
                challenge,
                &statement,
                wall_seconds()?,
            )
            .map_err(|e| e.to_string())?;
        if self.role == SessionRole::Viewer {
            self.offer_hash = Some(digest(&[sdp.as_bytes()]));
        }
        let encoded = serde_json::to_string(&SignedSdp {
            sdp,
            signature: hex_bytes(&[&signature]),
        })
        .map_err(|_| "LAN_DEVICE_SDP_INVALID")?;
        if encoded.len() > MAX_SIGNED_SDP {
            return Err("LAN_DEVICE_SDP_SIZE".into());
        }
        Ok(encoded)
    }

    /// Validate the expected role and exact SDP before returning the bytes to
    /// the caller. The caller must then enforce SDP/candidate and consent policy.
    pub(crate) fn open(&mut self, body: &str) -> Result<String, String> {
        let mut pending = self.incoming.take().ok_or("LAN_DEVICE_SDP_ORDER")?;
        if body.len() > MAX_SIGNED_SDP {
            return Err("LAN_DEVICE_SDP_SIZE".into());
        }
        let signed: SignedSdp = serde_json::from_str(body).map_err(|_| "LAN_DEVICE_SDP_INVALID")?;
        let signature = from_hex::<64>(&signed.signature)?;
        let signer = match self.role {
            SessionRole::Host => SessionRole::Viewer,
            SessionRole::Viewer => SessionRole::Host,
        };
        let statement = self.sdp_statement(signer, &signed.sdp)?;
        pending
            .verify(&signature, &statement, wall_seconds()?)
            .map_err(|e| e.to_string())?;
        if self.role == SessionRole::Host {
            self.offer_hash = Some(digest(&[signed.sdp.as_bytes()]));
        }
        Ok(signed.sdp)
    }

    fn sdp_statement(&self, signer: SessionRole, sdp: &str) -> Result<[u8; 32], String> {
        if sdp.is_empty() || sdp.len() > MAX_SDP || sdp.contains('\0') {
            return Err("LAN_DEVICE_SDP_SIZE".into());
        }
        let domain: &[u8] = match signer {
            SessionRole::Viewer => b"RemoteHub.LAN.device-verified.sdp-offer.v1\0",
            SessionRole::Host => b"RemoteHub.LAN.device-verified.sdp-answer.v1\0",
        };
        let offer_hash = match (signer, self.offer_hash) {
            (SessionRole::Viewer, None) => [0; 32],
            (SessionRole::Host, Some(hash)) => hash,
            _ => return Err("LAN_DEVICE_SDP_ORDER".into()),
        };
        Ok(digest(&[
            domain,
            &self.handshake,
            &offer_hash,
            sdp.as_bytes(),
        ]))
    }
}

#[derive(Clone, Copy)]
struct Challenges([SessionChallenge; 2]);

impl Challenges {
    fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != PAIR_BYTES {
            return Err("LAN_DEVICE_BAD_PAYLOAD".into());
        }
        let (first, second) = bytes.split_at(CHALLENGE_BYTES);
        let first = parse_challenge(first)?;
        let second = parse_challenge(second)?;
        if first == second {
            return Err("LAN_DEVICE_DUPLICATE_CHALLENGE".into());
        }
        Ok(Self([first, second]))
    }

    fn bytes(self) -> [u8; PAIR_BYTES] {
        let mut bytes = [0; PAIR_BYTES];
        bytes[..CHALLENGE_BYTES].copy_from_slice(&self.0[0].to_bytes());
        bytes[CHALLENGE_BYTES..].copy_from_slice(&self.0[1].to_bytes());
        bytes
    }
}

fn issue_pair(
    binding: &SessionBinding,
    role: SessionRole,
) -> Result<[PendingSessionProof; 2], String> {
    let now = wall_seconds()?;
    let issue =
        || PendingSessionProof::issue(binding, role, now, PROOF_TTL).map_err(|e| e.to_string());
    let pending = [issue()?, issue()?];
    if pending[0].challenge() == pending[1].challenge() {
        return Err("LAN_DEVICE_DUPLICATE_CHALLENGE".into());
    }
    Ok(pending)
}

async fn with_deadline<T>(
    work: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    timeout(HANDSHAKE_TIMEOUT, work)
        .await
        .map_err(|_| "LAN_DEVICE_HANDSHAKE_TIMEOUT".to_owned())?
}

#[derive(Clone, Copy)]
struct Endpoint([u8; 10]);

impl Endpoint {
    fn new(scope: LanScope, port: u16, role: SessionRole) -> Result<Self, String> {
        let scope = LanScope::new(scope.local, scope.peer)?;
        if port == 0 {
            return Err("LAN_DEVICE_PORT_INVALID".into());
        }
        let (host, viewer) = match role {
            SessionRole::Host => (scope.local, scope.peer),
            SessionRole::Viewer => (scope.peer, scope.local),
        };
        let mut data = [0; 10];
        data[..4].copy_from_slice(&host.octets());
        data[4..8].copy_from_slice(&viewer.octets());
        data[8..].copy_from_slice(&port.to_be_bytes());
        Ok(Self(data))
    }
}

struct Context {
    host_nonce: Uuid,
    context_hash: [u8; 32],
    binding: SessionBinding,
}

impl Context {
    fn new(
        endpoint: Endpoint,
        viewer_nonce: Uuid,
        host_nonce: Uuid,
        identity: &Identity,
        peer: &VerifiedPeer,
        role: SessionRole,
    ) -> Result<Self, String> {
        let local = identity.public();
        let (host, viewer) = match role {
            SessionRole::Host => (&local.public_key, peer.public_key()),
            SessionRole::Viewer => (peer.public_key(), &local.public_key),
        };
        let context_hash = digest(&[
            b"RemoteHub.LAN.device-context.v1\0",
            viewer_nonce.as_bytes(),
            host_nonce.as_bytes(),
            &endpoint.0,
            host,
            viewer,
        ]);
        let binding = peer
            .session_binding(&local, role, *viewer_nonce.as_bytes(), context_hash)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            host_nonce,
            context_hash,
            binding,
        })
    }

    fn proof_data(&self, host: Challenges, viewer: Challenges) -> [u8; 32] {
        digest(&[
            b"RemoteHub.LAN.device-proof.v1.video-only\0",
            &self.context_hash,
            &host.bytes(),
            &viewer.bytes(),
        ])
    }

    fn next_codec(
        &self,
        key: Arc<Key>,
        proof_data: &[u8; 32],
        host: bool,
    ) -> Result<Codec, String> {
        let next = digest(&[
            b"RemoteHub.LAN.device-post-auth.v1\0",
            &self.context_hash,
            proof_data,
        ]);
        let nonce = Uuid::from_slice(&next[..16]).map_err(|_| "LAN_DEVICE_NONCE_INVALID")?;
        Codec::new(key, nonce.to_string(), host)
    }
}

fn digest(parts: &[&[u8]]) -> [u8; 32] {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update(part);
    }
    hash.finalize().into()
}

fn acknowledgment(host_signature: &[u8; 64], proof_data: &[u8; 32]) -> [u8; 32] {
    digest(&[b"RemoteHub.LAN.device-ack.v1\0", host_signature, proof_data])
}

fn wall_seconds() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| "LAN_DEVICE_CLOCK".into())
}

fn checked_nonce(text: &str) -> Result<Uuid, String> {
    let nonce = Uuid::parse_str(text).map_err(|_| "LAN_DEVICE_NONCE_INVALID")?;
    if nonce.get_version() != Some(uuid::Version::Random) || nonce.hyphenated().to_string() != text
    {
        return Err("LAN_DEVICE_NONCE_INVALID".into());
    }
    Ok(nonce)
}

fn parse_challenge(data: &[u8]) -> Result<SessionChallenge, String> {
    SessionChallenge::from_bytes(data.try_into().map_err(|_| "LAN_DEVICE_BAD_PAYLOAD")?)
        .map_err(|e| e.to_string())
}

fn signature(data: &[u8]) -> Result<[u8; 64], String> {
    data.try_into().map_err(|_| "LAN_DEVICE_BAD_PAYLOAD".into())
}

async fn read_auth_frame(io: &mut (impl AsyncRead + Unpin)) -> Result<Frame, String> {
    // Same length prefix as lan_wire::read but a tighter cap before allocation,
    // because no legitimate pre-SDP handshake frame is this large.
    let len = io.read_u32().await.map_err(|_| "SIGNAL_DISCONNECTED")? as usize;
    if len == 0 || len > MAX_AUTH_FRAME {
        return Err("LAN_DEVICE_FRAME_SIZE".into());
    }
    let mut data = [0; MAX_AUTH_FRAME];
    io.read_exact(&mut data[..len])
        .await
        .map_err(|_| "SIGNAL_DISCONNECTED")?;
    serde_json::from_slice(&data[..len]).map_err(|_| "LAN_DEVICE_FRAME_INVALID".into())
}

async fn send(
    io: &mut (impl AsyncWrite + Unpin),
    codec: &mut Codec,
    kind: &str,
    payload: String,
) -> Result<(), String> {
    lan_wire::write(io, codec.encode(kind, payload)?).await
}

async fn receive(
    io: &mut (impl AsyncRead + Unpin),
    codec: &mut Codec,
    expected_kind: &str,
) -> Result<String, String> {
    let (kind, payload) = codec.accept(read_auth_frame(io).await?)?;
    if kind != expected_kind {
        return Err("LAN_DEVICE_UNEXPECTED_KIND".into());
    }
    Ok(payload)
}

fn hex_bytes(parts: &[&[u8]]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(parts.iter().map(|part| part.len() * 2).sum());
    for byte in parts.iter().flat_map(|part| part.iter()).copied() {
        text.push(char::from(HEX[usize::from(byte >> 4)]));
        text.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    text
}

fn from_hex<const N: usize>(text: &str) -> Result<[u8; N], String> {
    if text.len() != N * 2 {
        return Err("LAN_DEVICE_BAD_PAYLOAD".into());
    }
    let mut bytes = [0; N];
    let decode = |digit| match digit {
        b'0'..=b'9' => Ok(digit - b'0'),
        b'a'..=b'f' => Ok(digit - b'a' + 10),
        _ => Err("LAN_DEVICE_BAD_PAYLOAD"),
    };
    for (byte, pair) in bytes.iter_mut().zip(text.as_bytes().as_chunks::<2>().0) {
        *byte = (decode(pair[0])? << 4) | decode(pair[1])?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hub_identity::pairing::PairingCandidate;
    use tokio::io::{duplex, AsyncWriteExt};

    const PORT: u16 = 48604;
    const OFFER: &str = "test-only SDP offer\r\n";
    const ANSWER: &str = "test-only SDP answer\r\n";

    fn scopes() -> (LanScope, LanScope) {
        let host = "192.168.23.4".parse().unwrap();
        let viewer = "192.168.23.5".parse().unwrap();
        (
            LanScope::new(host, viewer).unwrap(),
            LanScope::new(viewer, host).unwrap(),
        )
    }

    fn pins() -> (Identity, VerifiedPeer, Identity, VerifiedPeer) {
        let host = Identity::generate_ephemeral().unwrap();
        let viewer = Identity::generate_ephemeral().unwrap();
        let pin_viewer = pin(&host, &viewer);
        let pin_host = pin(&viewer, &host);
        (host, pin_viewer, viewer, pin_host)
    }

    fn pin(local: &Identity, peer: &Identity) -> VerifiedPeer {
        let public = peer.public();
        let candidate = PairingCandidate::parse(&public.pairing_code()).unwrap();
        local.verify_peer(&candidate, &public.fingerprint).unwrap()
    }

    async fn connect<'a>(
        host: &'a Identity,
        pin_viewer: &VerifiedPeer,
        viewer: &'a Identity,
        pin_host: &VerifiedPeer,
    ) -> ((Codec, VerifiedSdp<'a>), (Codec, VerifiedSdp<'a>)) {
        let (mut host_io, mut viewer_io) = duplex(4096);
        let (host_scope, viewer_scope) = scopes();
        let key = Arc::new(Key::generate());
        tokio::try_join!(
            authenticate_host(
                &mut host_io,
                key.clone(),
                host,
                pin_viewer,
                host_scope,
                PORT
            ),
            authenticate_viewer(&mut viewer_io, key, viewer, pin_host, viewer_scope, PORT),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn mutual_device_proofs_enable_exact_offer_and_answer_once() {
        let (host, pin_viewer, viewer, pin_host) = pins();
        let ((mut host_codec, mut host_sdp), (mut viewer_codec, mut viewer_sdp)) =
            connect(&host, &pin_viewer, &viewer, &pin_host).await;

        let offer = viewer_sdp.seal(OFFER.into()).unwrap();
        let frame = viewer_codec.encode("offer", offer).unwrap();
        let (kind, body) = host_codec.accept(frame).unwrap();
        assert_eq!(kind, "offer");
        assert_eq!(host_sdp.open(&body).unwrap(), OFFER);
        assert_eq!(host_sdp.open(&body).unwrap_err(), "LAN_DEVICE_SDP_ORDER");
        let answer = host_sdp.seal(ANSWER.into()).unwrap();
        let frame = host_codec.encode("answer", answer).unwrap();
        let (kind, body) = viewer_codec.accept(frame).unwrap();
        assert_eq!(kind, "answer");
        assert_eq!(viewer_sdp.open(&body).unwrap(), ANSWER);
        assert_eq!(viewer_sdp.open(&body).unwrap_err(), "LAN_DEVICE_SDP_ORDER");
    }

    #[tokio::test]
    async fn wrong_pinned_peer_wrong_endpoint_or_temporary_key_stop_before_sdp() {
        let (host, pin_viewer, viewer, pin_host) = pins();
        let stranger = Identity::generate_ephemeral().unwrap();
        let pin_stranger = pin(&host, &stranger);
        let (host_scope, viewer_scope) = scopes();
        for (pinned, destination_port, same_key) in [
            (&pin_stranger, PORT, true),
            (&pin_viewer, PORT + 1, true),
            (&pin_viewer, PORT, false),
        ] {
            let key = Arc::new(Key::generate());
            let viewer_key = if same_key {
                key.clone()
            } else {
                Arc::new(Key::generate())
            };
            let (mut host_io, mut viewer_io) = duplex(4096);
            let (host_result, viewer_result) = tokio::join!(
                async {
                    let result =
                        authenticate_host(&mut host_io, key, &host, pinned, host_scope, PORT).await;
                    drop(host_io);
                    result
                },
                async {
                    let result = authenticate_viewer(
                        &mut viewer_io,
                        viewer_key,
                        &viewer,
                        &pin_host,
                        viewer_scope,
                        destination_port,
                    )
                    .await;
                    drop(viewer_io);
                    result
                },
            );
            let Err(code) = host_result else {
                panic!("host accepted wrong pairing context");
            };
            assert_eq!(
                code,
                if same_key {
                    "SESSION_PROOF_SIGNATURE"
                } else {
                    "AUTH_REJECTED"
                }
            );
            assert!(viewer_result.is_err());
        }
    }

    #[tokio::test]
    async fn pre_auth_rejects_legacy_offer_wrong_role_and_oversized_prefix() {
        let (host, pin_viewer, _, _) = pins();
        let (host_scope, _) = scopes();
        let key = Arc::new(Key::generate());
        for (kind, expected) in [
            (Some("offer"), "LAN_DEVICE_HELLO_REQUIRED"),
            (Some(HOST_PROOF), "LAN_DEVICE_HELLO_REQUIRED"),
            (None, "LAN_DEVICE_FRAME_SIZE"),
        ] {
            let (mut host_io, mut sender) = duplex(4096);
            if let Some(kind) = kind {
                let mut viewer =
                    Codec::new(key.clone(), Uuid::new_v4().to_string(), false).unwrap();
                send(&mut sender, &mut viewer, kind, OFFER.into())
                    .await
                    .unwrap();
            } else {
                sender.write_u32((MAX_AUTH_FRAME + 1) as u32).await.unwrap();
            }
            let Err(code) = authenticate_host(
                &mut host_io,
                key.clone(),
                &host,
                &pin_viewer,
                host_scope,
                PORT,
            )
            .await
            else {
                panic!("invalid prefix or initial message passed authentication");
            };
            assert_eq!(code, expected);
        }
    }

    #[tokio::test]
    async fn even_valid_postauth_hmac_cannot_modify_or_reuse_signed_descriptions() {
        let (host, pin_viewer, viewer, pin_host) = pins();
        let ((mut host_codec, mut host_sdp), (mut viewer_codec, mut viewer_sdp)) =
            connect(&host, &pin_viewer, &viewer, &pin_host).await;
        let envelope = viewer_sdp.seal(OFFER.into()).unwrap();
        let tampered = envelope.replace("SDP offer", "SDP changed");
        // A party with the existing HMAC state can re-MAC the JSON, but cannot
        // repair the independent device signature after editing the SDP.
        let frame = viewer_codec.encode("offer", tampered).unwrap();
        let (kind, body) = host_codec.accept(frame).unwrap();
        assert_eq!(kind, "offer");
        assert_eq!(host_sdp.open(&body).unwrap_err(), "SESSION_PROOF_SIGNATURE");
        assert_eq!(
            host_sdp.open(&envelope).unwrap_err(),
            "LAN_DEVICE_SDP_ORDER"
        );

        let ((_, mut host_sdp), (_, mut viewer_sdp)) =
            connect(&host, &pin_viewer, &viewer, &pin_host).await;
        assert_eq!(
            host_sdp.open(&envelope).unwrap_err(),
            "SESSION_PROOF_SIGNATURE"
        );
        assert_eq!(
            viewer_sdp.seal("x".repeat(MAX_SDP + 1)).unwrap_err(),
            "LAN_DEVICE_SDP_SIZE"
        );
    }
}
