//! Loopback-only transport fixture. Fixed text markers drive a fake input sink.
use super::{DataChannel, DataChannelEvent, Peer, opened};
use hub_webrtc::{
    input_approval::{End, Error as ApprovalError, InputScope, SessionSink, SinkError, State},
    input_queue::{Control, Dispatch, Error, InputQueue},
};
use rtc::peer_connection::transport::{RTCDtlsTransportState, RTCIceTransportState};
use std::{
    cell::RefCell,
    net::{Ipv4Addr, SocketAddrV4},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
use webrtc::{
    data_channel::RTCDataChannelInit,
    peer_connection::{RTCIceCandidate, RTCIceCandidateType, RTCIceProtocol},
};

pub(super) const LABEL: &str = "remotehub-loopback-fake-input-v1";
const PROTOCOL: &str = "remotehub.fixture";
const DENIED: Error = Error::Approval(ApprovalError::NotAllowed);

pub(super) fn channel_init() -> RTCDataChannelInit {
    RTCDataChannelInit {
        protocol: PROTOCOL.into(),
        ..Default::default()
    }
}

pub(super) fn signed_candidate(sdp: &str) -> SocketAddrV4 {
    let mut candidates = sdp
        .lines()
        .filter_map(|line| line.strip_prefix("a=candidate:"));
    let fields: Vec<_> = candidates.next().unwrap().split_whitespace().collect();
    assert!(candidates.next().is_none());
    assert_eq!(fields[1], "1");
    assert!(fields[2].eq_ignore_ascii_case("udp"));
    assert_eq!(&fields[6..8], ["typ", "host"]);
    assert_eq!(fields[4], "127.0.0.1");
    let port = fields[5].parse().unwrap();
    assert_ne!(port, 0);
    SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)
}

async fn selected(peer: &Peer, local: SocketAddrV4, remote: SocketAddrV4) {
    let dtls = peer.pc.sctp().await.unwrap().transport();
    assert_eq!(
        dtls.state().await.unwrap(),
        RTCDtlsTransportState::Connected
    );
    let ice = dtls.ice_transport();
    assert!(matches!(
        ice.state().await.unwrap(),
        RTCIceTransportState::Connected | RTCIceTransportState::Completed
    ));
    let pair = ice.get_selected_candidate_pair().await.unwrap().unwrap();
    for (candidate, expected) in [(pair.local(), local), (pair.remote(), remote)] {
        let candidate: &RTCIceCandidate = candidate;
        assert_eq!(candidate.typ, RTCIceCandidateType::Host);
        assert_eq!(candidate.protocol, RTCIceProtocol::Udp);
        assert_eq!(candidate.component, 1);
        assert_eq!(candidate.address, expected.ip().to_string());
        assert_eq!(candidate.port, expected.port());
    }
}

async fn text(channel: &Arc<dyn DataChannel>, expected: &str) {
    loop {
        match channel.poll().await {
            Some(DataChannelEvent::OnMessage(message)) => {
                assert!(message.is_string);
                assert_eq!(message.data.as_ref(), expected.as_bytes());
                return;
            }
            Some(
                DataChannelEvent::OnBufferedAmountHigh | DataChannelEvent::OnBufferedAmountLow,
            ) => {}
            _ => panic!("fixture data channel closed before expected marker"),
        }
    }
}

#[derive(Default)]
struct Fake {
    applied: usize,
    held: bool,
    releases: usize,
}

struct FakeSink(Rc<RefCell<Fake>>);

impl SessionSink for FakeSink {
    type Event = ();
    fn apply(&mut self, _: &()) -> Result<(), SinkError> {
        let mut state = self.0.borrow_mut();
        state.applied += 1;
        state.held = true;
        Ok(())
    }
    fn release_owned(&mut self) -> Result<(), SinkError> {
        let mut state = self.0.borrow_mut();
        assert!(state.held);
        state.held = false;
        state.releases += 1;
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub(super) enum Consent {
    Mock(bool),
    #[cfg(windows)]
    Native,
}

pub(super) enum Outcome {
    AppliedAndRevoked,
    Declined,
    Expired,
}

impl Consent {
    pub(super) fn extra_budget(self) -> Duration {
        match self {
            Self::Mock(_) => Duration::ZERO,
            #[cfg(windows)]
            Self::Native => Duration::from_secs(25),
        }
    }

    fn decision_window(self) -> Duration {
        match self {
            Self::Mock(_) => Duration::from_secs(5),
            #[cfg(windows)]
            Self::Native => self.extra_budget(),
        }
    }

    async fn decide(self, _deadline: Instant) -> Option<bool> {
        match self {
            // This is an explicit test-owner decision, not a channel message.
            Self::Mock(approved) => Some(approved),
            #[cfg(windows)]
            Self::Native => windows_decision(_deadline).await,
        }
    }

    pub(super) fn report(self, outcome: Outcome) {
        match self {
            Self::Mock(approved) => assert!(matches!(
                (approved, outcome),
                (true, Outcome::AppliedAndRevoked) | (false, Outcome::Declined)
            )),
            #[cfg(windows)]
            Self::Native => println!(
                "{}",
                match outcome {
                    Outcome::AppliedAndRevoked => "WSS_NATIVE_CONSENT_REVOKE_PASS",
                    Outcome::Declined => "WSS_NATIVE_CONSENT_DECLINED_PASS",
                    Outcome::Expired => "WSS_NATIVE_CONSENT_EXPIRED_PASS",
                }
            ),
        }
    }
}

#[cfg(windows)]
async fn windows_decision(deadline: Instant) -> Option<bool> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IDNO, IDYES, MB_DEFBUTTON2, MB_ICONQUESTION, MB_YESNO, MessageBoxW,
    };

    // The native modal must not block the async transport or its deadline. If
    // left open, it can no longer authorize at the deadline and the exact-test
    // process exits after teardown. Do not simulate a user response.
    let (send, receive) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("remotehub-fixture-consent".into())
        .spawn(move || {
            fn wide(text: &str) -> Vec<u16> {
                text.encode_utf16().chain(std::iter::once(0)).collect()
            }
            let caption = wide("RemoteHub 原生同意回环自检");
            let text = wide(
                "使用临时测试身份，仅验证本机 127.0.0.1 的 WebRTC 回环连接。不读取屏幕，不操作真实键鼠。

是否批准一次模拟输入？默认「否」，决定窗口约 25 秒。批准后也会立即测试撤销。",
            );
            // SAFETY: Both UTF-16 buffers are NUL-terminated and live until
            // MessageBoxW returns; the test uses no owner window.
            let response = unsafe {
                MessageBoxW(
                    std::ptr::null_mut(),
                    text.as_ptr(),
                    caption.as_ptr(),
                    MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION,
                )
            };
            let _ = send.send(response);
        })
        .expect("could not start native consent dialog");

    match tokio::time::timeout_at(deadline.into(), receive).await {
        Err(_) => None,
        Ok(result) => match result.expect("native consent dialog thread exited unexpectedly") {
            IDNO => Some(false),
            IDYES => Some(true),
            _ => panic!("native consent dialog failed"),
        },
    }
}

pub(super) async fn exercise(
    host: &mut Peer,
    viewer: &Peer,
    outgoing: Arc<dyn DataChannel>,
    scope: InputScope,
    host_addr: SocketAddrV4,
    viewer_addr: SocketAddrV4,
    consent: Consent,
) -> Outcome {
    opened(&outgoing).await;
    let incoming = host.channels.recv().await.unwrap();
    opened(&incoming).await;
    assert_eq!(incoming.id(), outgoing.id());
    assert!(host.channels.try_recv().is_err());
    for channel in [&incoming, &outgoing] {
        assert_eq!(channel.label().await.unwrap(), LABEL);
        assert_eq!(channel.protocol().await.unwrap(), PROTOCOL);
        assert!(channel.ordered().await.unwrap());
        assert!(!channel.negotiated().await.unwrap());
        assert_eq!(channel.max_retransmits().await.unwrap(), None);
        assert_eq!(channel.max_packet_life_time().await.unwrap(), None);
    }
    selected(host, host_addr, viewer_addr).await;
    selected(viewer, viewer_addr, host_addr).await;

    let fake = Rc::new(RefCell::new(Fake::default()));
    let deadline = Instant::now() + consent.decision_window();
    let mut owner = InputQueue::view_only(scope, deadline, FakeSink(fake.clone()));
    let prompt = owner.begin_local_decision(&scope).unwrap();
    outgoing.send_text("press-before-approval").await.unwrap();
    text(&incoming, "press-before-approval").await;
    assert_eq!(owner.enqueue(&scope, ()), Err(DENIED));
    assert_eq!(fake.borrow().applied, 0);
    incoming.send_text("denied").await.unwrap();
    text(&outgoing, "denied").await;

    let decision = consent.decide(deadline).await;
    // Recheck transport after an affirmative native response. A lost or
    // changed route fails the fixture before the owner can accept the choice.
    #[cfg(windows)]
    if matches!(consent, Consent::Native) && decision == Some(true) && Instant::now() < deadline {
        selected(host, host_addr, viewer_addr).await;
        selected(viewer, viewer_addr, host_addr).await;
    }
    // The serialized owner independently samples its monotonic clock before
    // accepting even a Yes delivered on the deadline.
    let outcome = match owner.complete_local_decision(
        prompt,
        &scope,
        decision == Some(true),
        Control::Continue,
    ) {
        Ok(()) if decision == Some(true) => None,
        Ok(()) if decision == Some(false) => Some((End::Declined, Outcome::Declined)),
        Err(error) if error == DENIED && owner.state() == State::Ended(End::Expired) => {
            Some((End::Expired, Outcome::Expired))
        }
        result => panic!("unexpected local consent result: {result:?}"),
    };
    if let Some((reason, outcome)) = outcome {
        assert_eq!(owner.state(), State::Ended(reason));
        outgoing.send_text("press-late").await.unwrap();
        text(&incoming, "press-late").await;
        assert_eq!(owner.enqueue(&scope, ()), Err(DENIED));
        assert_eq!(
            owner.dispatch_next(&scope, Control::Continue),
            Ok(Dispatch::Closed)
        );
        assert_eq!(fake.borrow().applied, 0);
        assert_eq!(fake.borrow().releases, 0);
        assert!(!fake.borrow().held);
        incoming.send_text("closed-no-ack").await.unwrap();
        text(&outgoing, "closed-no-ack").await;
        outgoing.close().await.unwrap();
        return outcome;
    }
    outgoing.send_text("press-approved").await.unwrap();
    text(&incoming, "press-approved").await;
    owner.enqueue(&scope, ()).unwrap();
    assert_eq!(fake.borrow().applied, 0);
    assert_eq!(
        owner.dispatch_next(&scope, Control::Continue),
        Ok(Dispatch::Applied)
    );
    assert_eq!(fake.borrow().applied, 1);
    assert!(fake.borrow().held);
    incoming.send_text("applied-ack").await.unwrap();
    text(&outgoing, "applied-ack").await;

    outgoing.send_text("press-queued").await.unwrap();
    text(&incoming, "press-queued").await;
    owner.enqueue(&scope, ()).unwrap();
    assert_eq!(
        owner.dispatch_next(&scope, Control::Revoke),
        Ok(Dispatch::Closed)
    );
    assert_eq!(owner.state(), State::Ended(End::Revoked));
    // Send another marker while SCTP is still alive, then close the channel.
    outgoing.send_text("press-late").await.unwrap();
    text(&incoming, "press-late").await;
    assert_eq!(owner.enqueue(&scope, ()), Err(DENIED));
    assert_eq!(
        owner.dispatch_next(&scope, Control::Continue),
        Ok(Dispatch::Closed)
    );
    assert_eq!(fake.borrow().applied, 1);
    assert_eq!(fake.borrow().releases, 1);
    assert!(!fake.borrow().held);
    incoming.send_text("revoked-no-ack").await.unwrap();
    text(&outgoing, "revoked-no-ack").await;
    outgoing.close().await.unwrap();
    Outcome::AppliedAndRevoked
}
