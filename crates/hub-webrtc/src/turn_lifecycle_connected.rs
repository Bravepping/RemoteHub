//! Operator-only data-channel lifecycle diagnostic. No capture, media tracks or input APIs.
use super::*;
use std::io::{BufRead, Read, Write};
use webrtc::{data_channel::DataChannelEvent, peer_connection::RTCSessionDescription};

fn data_only_relay_sdp(sdp: &str, grant: &RelayGrant) -> Result<(), String> {
    if sdp.len() > 65536 || sdp.contains('\0') {
        return Err("PROBE_SDP_SIZE".into());
    }
    let lines: Vec<_> = sdp.lines().map(str::trim).collect();
    let media: Vec<_> = lines.iter().filter(|s| s.starts_with("m=")).collect();
    let mid: Vec<_> = lines
        .iter()
        .filter_map(|s| s.strip_prefix("a=mid:"))
        .collect();
    let bundle: Vec<_> = lines
        .iter()
        .filter_map(|s| s.strip_prefix("a=group:BUNDLE "))
        .collect();
    if media.len() != 1
        || !media[0].contains(" UDP/DTLS/SCTP webrtc-datachannel")
        || !media[0].starts_with("m=application ")
        || mid.len() != 1
        || bundle.len() != 1
        || bundle[0] != mid[0]
    {
        return Err("PROBE_SINGLE_DATA_MEDIA_REQUIRED".into());
    }
    let hashes: Vec<_> = lines
        .iter()
        .filter_map(|s| s.strip_prefix("a=fingerprint:sha-256 "))
        .collect();
    if hashes.is_empty()
        || hashes.iter().any(|h| {
            h.split(':').count() != 32
                || h.split(':')
                    .any(|v| v.len() != 2 || u8::from_str_radix(v, 16).is_err())
        })
    {
        return Err("PROBE_DTLS_FINGERPRINT_REQUIRED".into());
    }
    let mut candidates = 0;
    for line in lines {
        if let Some(c) = line.strip_prefix("a=candidate:") {
            let fields: Vec<_> = c.split_whitespace().collect();
            if fields.len() < 8
                || fields[1] != "1"
                || !fields[2].eq_ignore_ascii_case("udp")
                || fields[6] != "typ"
                || fields[7] != "relay"
                || !fields[4]
                    .parse()
                    .ok()
                    .zip(fields[5].parse().ok())
                    .is_some_and(|(ip, port)| grant.permits_candidate(ip, port))
            {
                return Err("PROBE_RELAY_CANDIDATE_REJECTED".into());
            }
            candidates += 1;
        }
    }
    if !(1..=2).contains(&candidates) {
        return Err("PROBE_CANDIDATE_BUDGET".into());
    }
    Ok(())
}

async fn input() -> Result<String, String> {
    tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .take(96 * 1024)
            .read_line(&mut line)
            .map_err(|_| "PROBE_STDIN")?;
        if line.len() > 65536 {
            return Err("PROBE_INPUT_LIMIT".into());
        }
        Ok(line)
    })
    .await
    .map_err(|_| "PROBE_STDIN_TASK")?
}
fn emit(event: &str, extras: serde_json::Value) {
    println!(
        "{}",
        serde_json::json!({"case":"native-connected","event":event,"timeNs":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0,|x|x.as_nanos()),"data":extras})
    );
    let _ = std::io::stdout().flush();
}

impl TurnProbe {
    pub async fn connected_lifecycle(self) -> Result<Diagnostic, String> {
        let outcome = tokio::time::timeout(Duration::from_secs(35), async {
            let raw = input().await?;
            let offer: serde_json::Value =
                serde_json::from_str(&raw).map_err(|_| "PROBE_OFFER_SCHEMA")?;
            let offer = offer
                .get("offer")
                .and_then(|x| x.as_str())
                .ok_or("PROBE_OFFER_REQUIRED")?;
            data_only_relay_sdp(offer, &self.grant)?;
            self.pc
                .set_remote_description(
                    RTCSessionDescription::offer(offer.to_string()).map_err(|_| "PROBE_OFFER")?,
                )
                .await
                .map_err(|_| "PROBE_SET_OFFER")?;
            let answer = self
                .pc
                .create_answer(None)
                .await
                .map_err(|_| "PROBE_ANSWER")?;
            self.pc
                .set_local_description(answer)
                .await
                .map_err(|_| "PROBE_SET_ANSWER")?;
            while !self.complete.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let answer = self
                .pc
                .local_description()
                .await
                .ok_or("PROBE_LOCAL_DESCRIPTION")?
                .sdp;
            // This single line is consumed privately by the local test broker, never exported.
            println!(
                "{}",
                serde_json::json!({"event":"private_answer","sdp":answer})
            );
            let _ = std::io::stdout().flush();
            let channel = self
                .incoming
                .lock()
                .await
                .recv()
                .await
                .ok_or("PROBE_CHANNEL_REQUIRED")?;
            if channel.label().await.map_err(|_| "PROBE_LABEL")? != "allocation.probe" {
                return Err("PROBE_CHANNEL_LABEL".into());
            }
            loop {
                if let Some(DataChannelEvent::OnMessage(m)) = channel.poll().await {
                    if !m.is_string || m.data.as_ref() != b"lifecycle-ping" {
                        return Err("PROBE_PAYLOAD".into());
                    }
                    channel
                        .try_send_text("lifecycle-pong")
                        .await
                        .map_err(|_| "PROBE_SEND")?;
                    break;
                }
            }
            let diagnostic = self.diagnostic().await?;
            if !diagnostic
                .selected_pair
                .as_ref()
                .is_some_and(|p| p.matches_relay_policy)
            {
                return Err("PROBE_SELECTED_RELAY_REQUIRED".into());
            }
            emit(
                "connected_ping_pong",
                serde_json::to_value(diagnostic.selected_pair).map_err(|_| "PROBE_REPORT")?,
            );
            if input().await?.trim() != "close" {
                return Err("PROBE_CLOSE_COMMAND".into());
            }
            Ok::<(), String>(())
        })
        .await
        .unwrap_or_else(|_| Err("PROBE_CONNECTED_TIMEOUT".into()));
        emit("pc_close_call", serde_json::Value::Null);
        let closed = self.stop().await?;
        emit(
            "pc_close_returned",
            serde_json::json!({"localPeerConnectionReleased":closed.resources_released}),
        );
        outcome?;
        Ok(closed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostic_data_sdp_cannot_accept_host_or_media() {
        let (g, _) = crate::ice_policy::tests::fixture();
        let good=format!("v=0\r\na=group:BUNDLE 0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\na=mid:0\r\na=fingerprint:sha-256 {}\r\na=candidate:r 1 udp 1 8.8.8.8 49160 typ relay\r\n",["AB";32].join(":"));
        assert!(data_only_relay_sdp(&good, &g).is_ok());
        for bad in [
            good.replace("typ relay", "typ host"),
            good.replace("8.8.8.8", "8.8.4.4"),
            good.clone() + "m=video 9 UDP/TLS/RTP/SAVPF 102\r\n",
            good.replace("AB:AB", "AB:ZZ"),
        ] {
            assert!(data_only_relay_sdp(&bad, &g).is_err());
        }
    }
}
