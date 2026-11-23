import { invoke } from '@tauri-apps/api/core';

export interface NativeStats {
  input_pair_approved?: boolean | null;
  direct_pair_diagnostic?: {
    approved: boolean;
    local: { reason: string };
    remote: { reason: string };
  };
  native_path?: {
    path: 'direct' | 'relay';
    local_type: string;
    remote_type: string;
    protocol: string;
  };
  diagnostic_transport?: {
    drop_direct: boolean;
    dropped_packets: number;
    dropped_direct_checks?: number;
  } | null;
  source_kind?: string;
  state: string;
  elapsed_ms: number;
  frames_sent: number;
  rtp_packets_sent?: number;
  rtp_payload_bytes: number;
  rtp_packets_written: number;
  resources_released: boolean;
  error?: string;
  stream: {
    encoder: string;
    hardware: boolean;
    frames_input: number;
    frames_encoded: number;
    missed_ticks: number;
  };
}
export interface ReceiverSnapshot {
  elapsedMs: number;
  framesDecoded: number | null;
  framesReceived: number | null;
  packetsReceived: number | null;
  packetsLost: number | null;
  bytesReceived: number | null;
  videoWidth: number;
  videoHeight: number;
  framesRendered: number | null;
  currentTime: number;
  firstFrameMs: number | null;
  connectionState: string;
  iceState: string;
  codec: string | null;
  fmtp: string | null;
  selectedRemoteLoopback: boolean | null;
  localCandidateType: string | null;
  remoteCandidateType: string | null;
  freezeCount: number | null;
  totalFreezesDuration: number | null;
}

export function loopbackOffer(sdp: string): string {
  // Chromium may hide local host addresses behind mDNS. They are not resolved or forwarded by Rust.
  // Its 127-only answer is reached by the browser's ICE checks; peer-reflexive discovery stays on-host.
  return sdp
    .split(/\r?\n/)
    .filter((line) => {
      if (!line.startsWith('a=candidate:')) return true;
      const parts = line.slice(12).trim().split(/\s+/);
      return (
        parts[2]?.toLowerCase() === 'udp' &&
        parts[4] === '127.0.0.1' &&
        parts[7] === 'host'
      );
    })
    .map((line) => (line.startsWith('c=IN IP') ? 'c=IN IP4 0.0.0.0' : line))
    .join('\r\n');
}

export async function receiveStats(
  pc: RTCPeerConnection,
  video: HTMLVideoElement,
  started: number,
  firstFrameMs: number | null,
  provided?: RTCStatsReport,
): Promise<ReceiverSnapshot> {
  const all = provided ?? (await pc.getStats());
  let inbound: RTCInboundRtpStreamStats | undefined;
  let pair: RTCIceCandidatePairStats | undefined;
  let codec: { mimeType?: string; sdpFmtpLine?: string } | undefined;
  all.forEach((value) => {
    if (
      value.type === 'inbound-rtp' &&
      (value.kind === 'video' || value.mediaType === 'video')
    )
      inbound = value;
    if (value.type === 'transport' && value.selectedCandidatePairId)
      pair = all.get(value.selectedCandidatePairId);
  });
  if (inbound?.codecId) codec = all.get(inbound.codecId);
  const remote = pair && all.get(pair.remoteCandidateId);
  const local = pair && all.get(pair.localCandidateId);
  const extras = inbound as
    | (RTCInboundRtpStreamStats & {
        freezeCount?: number;
        totalFreezesDuration?: number;
      })
    | undefined;
  return {
    elapsedMs: performance.now() - started,
    framesDecoded: inbound?.framesDecoded ?? null,
    framesReceived: inbound?.framesReceived ?? null,
    packetsReceived: inbound?.packetsReceived ?? null,
    packetsLost: inbound?.packetsLost ?? null,
    bytesReceived: inbound?.bytesReceived ?? null,
    videoWidth: video.videoWidth,
    videoHeight: video.videoHeight,
    framesRendered: video.getVideoPlaybackQuality?.().totalVideoFrames ?? null,
    currentTime: video.currentTime,
    firstFrameMs,
    connectionState: pc.connectionState,
    iceState: pc.iceConnectionState,
    codec: codec?.mimeType ?? null,
    fmtp: codec?.sdpFmtpLine ?? null,
    selectedRemoteLoopback: remote
      ? remote.address === '127.0.0.1' || remote.ip === '127.0.0.1'
      : null,
    localCandidateType: local?.candidateType ?? null,
    remoteCandidateType: remote?.candidateType ?? null,
    freezeCount: extras?.freezeCount ?? null,
    totalFreezesDuration: extras?.totalFreezesDuration ?? null,
  };
}

export class LoopbackProbe {
  private pc?: RTCPeerConnection;
  private sessionId?: string;
  private timer?: ReturnType<typeof setTimeout>;
  private started = 0;
  private firstFrame: number | null = null;
  private history: ReceiverSnapshot[] = [];
  private stopping?: Promise<void>;
  private stopped = false;
  private lastHistory = 0;

  constructor(
    private video: HTMLVideoElement,
    private update: (
      state: string,
      native?: NativeStats,
      receiver?: ReceiverSnapshot,
    ) => void,
    private failed: (error: string) => void,
    private saved: (path: string) => void,
  ) {}

  async start(consent: boolean, seconds: number) {
    this.started = performance.now();
    this.stopped = false;
    this.update('starting');
    try {
      const result = await invoke<{ sessionId: string }>(
        'p004_start_loopback_probe',
        {
          config: {
            consent,
            seconds,
            width: 1280,
            height: 720,
            fps: 30,
            bitrateBps: 4000000,
          },
        },
      );
      this.sessionId = result.sessionId;
      if (this.stopped) {
        await invoke('p004_stop_loopback_probe', {
          sessionId: result.sessionId,
        });
        return;
      }
      this.pc = new RTCPeerConnection({
        iceServers: [],
        bundlePolicy: 'max-bundle',
      });
      const pc = this.pc;
      pc.ontrack = (event) => {
        const stream = event.streams[0] ?? new MediaStream([event.track]);
        this.video.srcObject = stream;
        void this.video.play().catch((error) => {
          if (this.stopped || this.video.srcObject !== stream) return;
          this.failed(String(error));
          void this.stop();
        });
      };
      const receiver = pc.addTransceiver('video', { direction: 'recvonly' });
      const codecs = RTCRtpReceiver.getCapabilities('video')?.codecs.filter(
        (codec) =>
          codec.mimeType.toLowerCase() === 'video/h264' &&
          codec.sdpFmtpLine
            ?.toLowerCase()
            .includes('profile-level-id=42001f') &&
          codec.sdpFmtpLine.includes('packetization-mode=1'),
      );
      if (!codecs?.length)
        throw new Error('WEBVIEW_H264_BASELINE_MODE1_UNAVAILABLE');
      receiver.setCodecPreferences(codecs);
      this.update('negotiating');
      await pc.setLocalDescription(await pc.createOffer());
      await new Promise<void>((resolve, reject) => {
        if (pc.iceGatheringState === 'complete') return resolve();
        const timer = setTimeout(() => {
          pc.onicegatheringstatechange = null;
          reject(new Error('BROWSER_ICE_GATHER_TIMEOUT'));
        }, 4000);
        pc.onicegatheringstatechange = () => {
          if (pc.iceGatheringState === 'complete') {
            clearTimeout(timer);
            pc.onicegatheringstatechange = null;
            resolve();
          }
        };
      });
      if (this.stopped) return;
      const answer = await invoke<string>('p004_signal', {
        sessionId: this.sessionId,
        offer: loopbackOffer(pc.localDescription!.sdp),
      });
      if (this.stopped) return;
      await pc.setRemoteDescription({ type: 'answer', sdp: answer });
      await this.poll();
    } catch (error) {
      if (!this.stopped) this.failed(String(error));
      await this.stop();
    }
  }

  private async poll() {
    if (this.stopped || !this.pc || !this.sessionId) return;
    const pc = this.pc;
    try {
      const native = await invoke<NativeStats>('p004_get_stats', {
        sessionId: this.sessionId,
      });
      if (this.stopped || this.pc !== pc) return;
      const receiver = await receiveStats(
        pc,
        this.video,
        this.started,
        this.firstFrame,
      );
      if (this.stopped || this.pc !== pc) return;
      if (
        this.firstFrame === null &&
        (receiver.framesDecoded ?? 0) > 0 &&
        receiver.videoWidth > 0 &&
        receiver.currentTime > 0
      )
        this.firstFrame = receiver.elapsedMs;
      receiver.firstFrameMs = this.firstFrame;
      if (
        receiver.elapsedMs - this.lastHistory >= 900 ||
        native.resources_released
      ) {
        this.history.push(receiver);
        this.history = this.history.slice(-60);
        this.lastHistory = receiver.elapsedMs;
      }
      const playing =
        (receiver.framesDecoded ?? 0) > 0 &&
        receiver.videoWidth > 0 &&
        !this.video.paused;
      this.update(
        playing && !native.resources_released ? 'playing' : native.state,
        native,
        receiver,
      );
      if (native.error) this.failed(native.error);
      if (native.resources_released) {
        await this.stop();
        return;
      }
      this.timer = setTimeout(() => {
        void this.poll();
      }, 350);
    } catch (error) {
      if (this.stopped) return;
      this.failed(String(error));
      await this.stop();
    }
  }

  stop(): Promise<void> {
    if (this.stopping) return this.stopping;
    this.stopped = true;
    clearTimeout(this.timer);
    this.stopping = this.finish();
    return this.stopping;
  }
  private async finish() {
    this.update('stopping');
    try {
      if (this.sessionId) {
        await invoke('p004_stop_loopback_probe', { sessionId: this.sessionId });
        if (this.pc && this.pc.connectionState !== 'closed')
          this.history.push(
            await receiveStats(
              this.pc,
              this.video,
              this.started,
              this.firstFrame,
            ),
          );
        const native = await invoke<NativeStats>('p004_get_stats', {
          sessionId: this.sessionId,
        });
        const pc = this.pc;
        const tracks =
          pc?.getReceivers().map((receiver) => receiver.track) ?? [];
        tracks.forEach((track) => track.stop());
        pc?.close();
        const browserClosed =
          (!pc || pc.connectionState === 'closed') &&
          tracks.every((track) => track.readyState === 'ended');
        this.pc = undefined;
        this.video.srcObject = null;
        const path = await invoke<string>('p004_save_report', {
          sessionId: this.sessionId,
          receiver: this.history.slice(-60),
          browserClosed,
        });
        this.saved(path);
        this.update(
          native.error ? 'failed' : 'stopped',
          native,
          this.history.at(-1),
        );
      } else this.update('stopped');
    } catch (error) {
      this.failed(String(error));
      this.update('failed');
    } finally {
      this.pc?.getReceivers().forEach((receiver) => receiver.track.stop());
      this.pc?.close();
      this.pc = undefined;
      this.video.srcObject = null;
    }
  }
}
