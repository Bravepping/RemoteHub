//! Product input on the exact sustained transport. It owns a single visual
//! native consent dialog and one non-renewable grant. A declined, expired or
//! revoked grant returns to passive viewing; physical transport loss still
//! ends the whole session.
use super::*;

const PRODUCT_VERSION: u8 = 3;
const PROMPT_WINDOW: Duration = Duration::from_secs(20);
const INPUT_WINDOW: Duration =
    Duration::from_secs(hub_webrtc::public_video::SUSTAINED_CONTROL_SESSION_SECONDS);

fn input_cutoff(now: Instant, media_end: Instant) -> Option<Instant> {
    (media_end > now).then(|| (now + INPUT_WINDOW).min(media_end))
}

fn grant_id_matches(value: &str, expected: &str) -> bool {
    valid_grant(value) && value == expected
}

enum GrantPoll<T> {
    Live(T),
    Ended,
}

// If the grant expires mid-I/O, release keys first but preserve the already
// polled transport future. Dropping that future would revoke the media
// route by design. Transport/physical Stop still consumes the session.
async fn within_host_grant<T, Sink: SessionSink<Event = Event>>(
    stop: &mut watch::Receiver<bool>,
    input_end: Instant,
    revoke: &mut watch::Receiver<bool>,
    owner: &mut PublicControlInput<Sink>,
    work: impl Future<Output = Result<T, &'static str>>,
) -> Flow<GrantPoll<T>> {
    let mut work = std::pin::pin!(work);
    tokio::select! {
        biased;
        halt = async {
            tokio::select! {
                biased;
                end = wait_halt(stop, input_end, true) => end,
                _ = revoke.wait_for(|&v| v) => Halt::Emergency,
            }
        } => {
            owner.advance(halt.cleanup()).map_err(|e| e.code())?;
            if matches!(halt, Halt::Stop) {
                return Err(Exit::Halt(halt));
            }
            work.await.map_err(Exit::Failed)?;
            Ok(GrantPoll::Ended)
        },
        result = &mut work => Ok(GrantPoll::Live(result?)),
    }
}

async fn idle(
    transport: &mut PublicControlTransport,
    stop: &mut watch::Receiver<bool>,
    end: Instant,
) -> Flow<()> {
    loop {
        // Bounded transport polling. No input bytes are interpreted when
        // the host has no live grant, including delayed old-grant messages.
        let _ = live(stop, end, false, transport.recv_text()).await?;
    }
}

async fn decline(
    transport: &mut PublicControlTransport,
    stop: &mut watch::Receiver<bool>,
    end: Instant,
    status: &watch::Sender<InputState>,
) -> Flow<()> {
    status.send_replace(InputState::Denied);
    let wire = encode(&Reply::InputDenied { v: PRODUCT_VERSION })?;
    live(stop, end, false, transport.send_text(&wire)).await?;
    idle(transport, stop, end).await
}

pub(crate) async fn run_host(
    mut transport: PublicControlTransport,
    mut stop: watch::Receiver<bool>,
    hwnd: usize,
    fingerprint: String,
    mut first_video: watch::Receiver<bool>,
    status: watch::Sender<InputState>,
    mut revoke: watch::Receiver<bool>,
) -> Result<(), &'static str> {
    status.send_replace(InputState::ViewOnly);
    let flow = async {
        if transport.role() != SessionRole::Host || !transport.is_sustained() {
            return Err(ROLE.into());
        }
        let media_end = transport.deadline();
        if !live(&mut stop, media_end, false, async {
            Ok(first_video.wait_for(|&sent| sent).await.is_ok())
        })
        .await?
        {
            return Err(Exit::Halt(Halt::Stop));
        }
        status.send_replace(InputState::Pending);
        let prompt_end = media_end.min(Instant::now() + PROMPT_WINDOW);
        // This is a separate affirmative choice, visibly disclosing the
        // longest possible permission and never extending the active media lease.
        let remaining = media_end.saturating_duration_since(Instant::now());
        let max_control_seconds = remaining
            .as_secs()
            .saturating_add(u64::from(remaining.subsec_nanos() != 0))
            .clamp(1, INPUT_WINDOW.as_secs());
        let mut response = approval_dialog_seconds(hwnd, fingerprint, max_control_seconds)?;
        let affirmative = loop {
            if local_halt(&stop, prompt_end, true).is_some() {
                break false;
            }
            match response.try_recv() {
                Ok(answer) => break answer,
                Err(oneshot::error::TryRecvError::Closed) => return Err(DIALOG.into()),
                Err(oneshot::error::TryRecvError::Empty) => (),
            }
            let _ = live(&mut stop, media_end, false, transport.recv_text()).await?;
        };
        // An expired or interrupted native prompt cannot authorize input,
        // even if its blocking Win32 dialog returns later.
        if !affirmative || local_halt(&stop, prompt_end, true).is_some() {
            return decline(&mut transport, &mut stop, media_end, &status).await;
        }

        let _ = live(&mut stop, media_end, false, transport.recv_text()).await?;
        if local_halt(&stop, prompt_end, true).is_some() {
            return decline(&mut transport, &mut stop, media_end, &status).await;
        }
        let input_end = input_cutoff(Instant::now(), media_end).ok_or(BAD_DEADLINE)?;
        let scope = transport.scope();
        let mut owner = PublicControlInput::for_sustained_host(
            &transport,
            input_end,
            DeferredInjector::default(),
        )
        .map_err(|error| error.code())?;
        let pending = owner
            .begin_local_decision(&scope, Control::Continue)
            .map_err(|e| e.code())?;
        owner
            .complete_local_decision(pending, &scope, true, Control::Continue)
            .map_err(|e| e.code())?;
        let grant_id = Uuid::new_v4().simple().to_string();
        let open = encode(&Reply::InputGranted {
            v: PRODUCT_VERSION,
            grant_id: grant_id.clone(),
        })?;

        let active = async {
            if matches!(
                within_host_grant(
                    &mut stop,
                    input_end,
                    &mut revoke,
                    &mut owner,
                    transport.send_text(&open)
                )
                .await?,
                GrantPoll::Ended
            ) {
                return Ok(());
            }
            status.send_replace(InputState::Allowed);
            loop {
                let raw = within_host_grant(
                    &mut stop,
                    input_end,
                    &mut revoke,
                    &mut owner,
                    transport.recv_text(),
                )
                .await?;
                let data = match raw {
                    GrantPoll::Live(Some(data)) => data,
                    GrantPoll::Live(None) => continue,
                    GrantPoll::Ended => return Ok(()),
                };
                if let Some(end) = local_halt(&stop, input_end, true) {
                    if matches!(end, Halt::Stop) {
                        return Err(Exit::Halt(end));
                    }
                    owner.advance(end.cleanup()).map_err(|e| e.code())?;
                    return Ok(());
                }
                let message = approved_message_version(&data, &grant_id, PRODUCT_VERSION)?;
                match owner.enqueue_v1(&scope, message.event_v1.as_bytes(), true, Control::Continue)
                {
                    Ok(()) => (),
                    Err(hub_webrtc::public_control_input::Error::NotAllowed)
                        if Instant::now() >= input_end =>
                    {
                        return Ok(())
                    }
                    Err(error) => return Err(error.code().into()),
                }
                let control = match local_halt(&stop, input_end, true) {
                    None if *revoke.borrow() => Control::Revoke,
                    Some(end) => end.cleanup(),
                    None => Control::Continue,
                };
                let Some(ack_seq) = owner.dispatch_next(&scope, control).map_err(|e| e.code())?
                else {
                    if owner.state() != State::Allowed {
                        return Ok(());
                    }
                    return Err(WIRE.into());
                };
                let ack = encode(&Reply::InputAck {
                    v: PRODUCT_VERSION,
                    ack_seq,
                })?;
                if matches!(
                    within_host_grant(
                        &mut stop,
                        input_end,
                        &mut revoke,
                        &mut owner,
                        transport.send_text(&ack)
                    )
                    .await?,
                    GrantPoll::Ended
                ) {
                    return Ok(());
                }
            }
        }
        .await;
        // Confirm native release before publishing the view-only state.
        owner
            .advance(Control::TransportLost)
            .map_err(|e| e.code())?;
        active?;
        status.send_replace(InputState::Ended);
        let ended = encode(&Reply::InputEnded {
            v: PRODUCT_VERSION,
            grant_id,
        })?;
        live(&mut stop, media_end, false, transport.send_text(&ended)).await?;
        idle(&mut transport, &mut stop, media_end).await
    }
    .await;
    status.send_replace(InputState::Closed);
    result(flow)
}

pub(crate) async fn run_viewer(
    mut transport: PublicControlTransport,
    mut stop: watch::Receiver<bool>,
    mut events: mpsc::Receiver<String>,
    status: watch::Sender<InputState>,
) -> Result<(), &'static str> {
    status.send_replace(InputState::ViewOnly);
    let flow = async {
        if transport.role() != SessionRole::Viewer || !transport.is_sustained() {
            return Err(ROLE.into());
        }
        let media_end = transport.deadline();
        let room = Uuid::from_bytes(transport.scope().room);
        status.send_replace(InputState::Pending);
        let grant = loop {
            discard_pending(&mut events)?;
            let Some(frame) = live(&mut stop, media_end, false, transport.recv_text()).await?
            else {
                continue;
            };
            match serde_json::from_slice::<Reply>(&frame).map_err(|_| WIRE)? {
                Reply::InputGranted {
                    v: PRODUCT_VERSION,
                    grant_id,
                } if valid_grant(&grant_id) => break grant_id,
                Reply::InputDenied { v: PRODUCT_VERSION } => {
                    status.send_replace(InputState::Denied);
                    return idle(&mut transport, &mut stop, media_end).await;
                }
                _ => return Err(WIRE.into()),
            }
        };
        let input_end = input_cutoff(Instant::now(), media_end).ok_or(BAD_DEADLINE)?;
        discard_pending(&mut events)?;
        status.send_replace(InputState::Allowed);
        let mut seq = 1;
        'grant: loop {
            require_current(&stop, media_end, false)?;
            if Instant::now() >= input_end {
                break;
            }
            let raw = match events.try_recv() {
                Ok(raw) => raw,
                Err(mpsc::error::TryRecvError::Empty) => {
                    if let Some(frame) =
                        live(&mut stop, media_end, false, transport.recv_text()).await?
                    {
                        match serde_json::from_slice::<Reply>(&frame).map_err(|_| WIRE)? {
                            Reply::InputEnded {
                                v: PRODUCT_VERSION,
                                grant_id,
                            } if grant_id_matches(&grant_id, &grant) => break,
                            _ => return Err(WIRE.into()),
                        }
                    }
                    continue;
                }
                Err(mpsc::error::TryRecvError::Disconnected) => return Err(Exit::Halt(Halt::Stop)),
            };
            let packet = wrap_event_version(&raw, room, seq, &grant, PRODUCT_VERSION, true)?;
            // Do not cancel a transport future on input-only timeout.
            live(&mut stop, media_end, false, transport.send_text(&packet)).await?;
            let ack_end = Instant::now() + ACK_WAIT;
            loop {
                if Instant::now() >= input_end {
                    break 'grant;
                }
                if Instant::now() >= ack_end {
                    return Err(ACK_TIMEOUT.into());
                }
                if let Some(frame) =
                    live(&mut stop, media_end, false, transport.recv_text()).await?
                {
                    match serde_json::from_slice::<Reply>(&frame).map_err(|_| WIRE)? {
                        Reply::InputAck {
                            v: PRODUCT_VERSION,
                            ack_seq,
                        } if ack_seq == seq => break,
                        Reply::InputEnded {
                            v: PRODUCT_VERSION,
                            grant_id,
                        } if grant_id_matches(&grant_id, &grant) => break 'grant,
                        _ => return Err(WIRE.into()),
                    }
                }
            }
            seq += 1;
        }
        status.send_replace(InputState::Ended);
        discard_pending(&mut events)?;
        idle(&mut transport, &mut stop, media_end).await
    }
    .await;
    status.send_replace(InputState::Closed);
    result(flow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sustained_keyboard_lease_is_not_the_media_lease() {
        let now = Instant::now();
        let media = now + Duration::from_secs(1800);
        assert_eq!(input_cutoff(now, media), Some(now + INPUT_WINDOW));
        assert_eq!(
            input_cutoff(media - Duration::from_secs(3), media),
            Some(media)
        );
        assert_eq!(input_cutoff(media, media), None);
        assert_eq!(input_cutoff(media + Duration::from_secs(1), media), None);
    }

    #[tokio::test]
    async fn expiry_releases_fake_input_before_existing_media_io_finishes() {
        use hub_webrtc::input_approval::InputScope;
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        struct FakeSink(Arc<AtomicBool>);
        impl SessionSink for FakeSink {
            type Event = Event;
            fn apply(&mut self, _: &Event) -> Result<(), SinkError> {
                Ok(())
            }
            fn release_owned(&mut self) -> Result<(), SinkError> {
                self.0.store(true, Ordering::SeqCst);
                Ok(())
            }
        }

        let released = Arc::new(AtomicBool::new(false));
        let scope = InputScope {
            room: *Uuid::new_v4().as_bytes(),
            generation: [1; 32],
            viewer_key: [2; 32],
        };
        let end = Instant::now() + Duration::from_millis(40);
        let mut owner =
            PublicControlInput::view_only(scope, end, FakeSink(released.clone())).unwrap();
        let pending = owner
            .begin_local_decision(&scope, Control::Continue)
            .unwrap();
        owner
            .complete_local_decision(pending, &scope, true, Control::Continue)
            .unwrap();
        let (_signal, mut stop) = watch::channel(false);
        let (_revoke_tx, mut revoke) = watch::channel(false);
        let io = async {
            tokio::time::sleep(Duration::from_millis(75)).await;
            assert!(released.load(Ordering::SeqCst));
            Ok(())
        };
        let result = within_host_grant(&mut stop, end, &mut revoke, &mut owner, io).await;
        assert!(matches!(result, Ok(GrantPoll::Ended)));
        assert_ne!(owner.state(), State::Allowed);
    }

    #[test]
    fn wheel_uses_product_wire_version_and_cannot_use_legacy_or_foreign_grant() {
        let room = Uuid::new_v4();
        let a = Uuid::new_v4().simple().to_string();
        let b = Uuid::new_v4().simple().to_string();
        let valid = r#"{"kind":"wheel","delta":-120}"#;
        let wrong_delta = r#"{"kind":"wheel","delta":-180}"#;
        assert!(wrap_event(valid, room, 1, &a).is_err());
        assert!(wrap_event_version(wrong_delta, room, 1, &a, PRODUCT_VERSION, true).is_err());
        let packet = wrap_event_version(valid, room, 1, &a, PRODUCT_VERSION, true).unwrap();
        assert!(approved_message(packet.as_bytes(), &a).is_err());
        assert!(approved_message_version(packet.as_bytes(), &b, PRODUCT_VERSION).is_err());
        assert!(approved_message_version(packet.as_bytes(), &a, PRODUCT_VERSION).is_ok());
        assert!(grant_id_matches(&a, &a));
        assert!(!grant_id_matches(&a, &b));
    }
}
