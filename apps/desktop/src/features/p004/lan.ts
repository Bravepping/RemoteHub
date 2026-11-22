import { invoke } from '@tauri-apps/api/core';
import { publishInputOffer } from './inputOffer';
import {
  ControlClient,
  type BrowserControlStats,
  type HostControlStats,
} from './control';
import {
  receiveStats,
  type NativeStats,
  type ReceiverSnapshot,
} from './webrtc';

export interface LanReceiver extends ReceiverSnapshot {
  connectionPath?: 'direct' | 'relay' | 'unknown';
  control?: BrowserControlStats;
  peerMatches: boolean | null;
  decoderImplementation: string | null;
}
export interface LanStatus {
  browser_control?: BrowserControlStats | null;
  role: string;
  state: string;
  elapsed_ms: number;
  authenticated: boolean;
  messages_received: number;
  source: (NativeStats & { control?: HostControlStats | null }) | null;
  receiver_samples: LanReceiver[];
  resources_released: boolean;
  error: string | null;
}
export interface LanConfig {
  verifiedPeer?: boolean;
  realControl?: boolean;
  controlSim?: boolean;
  role: 'sender' | 'receiver';
  consent: boolean;
  localIp: string;
  peerIp: string;
  seconds: number;
  port: number;
  pairingCode?: string;
}

// The Rust LAN path authenticates the peer's finish/stop messages. Do not
// infer graceful shutdown solely from the browser's DataChannel error event.
function authenticatedClosing(status: LanStatus): boolean {
  return (
    status.authenticated &&
    !status.error &&
    (status.state === 'finishing' ||
      (status.state === 'stopped' && status.resources_released))
  );
}
export function lanOffer(sdp: string, localIp: string): string {
  return sdp
    .split(/\r?\n/)
    .filter((line) => {
      if (!line.startsWith('a=candidate:')) return true;
      const p = line.slice(12).trim().split(/\s+/);
      return (
        p[1] === '1' &&
        p[2]?.toLowerCase() === 'udp' &&
        p[4] === localIp &&
        p[7] === 'host'
      );
    })
    .map((line) => (line.startsWith('c=') ? 'c=IN IP4 0.0.0.0' : line))
    .join('\r\n');
}
export function selectedPeer(
  all: RTCStatsReport,
  peerIp: string,
): boolean | null {
  let result: boolean | null = null;
  all.forEach((item) => {
    if (item.type !== 'transport' || !item.selectedCandidatePairId) return;
    const pair = all.get(item.selectedCandidatePairId);
    const remote = pair && all.get(pair.remoteCandidateId);
    if (remote)
      result =
        (remote.address ?? remote.ip) === peerIp &&
        remote.protocol?.toLowerCase() === 'udp';
  });
  return result;
}

export class LanProbe {
  private control?: ControlClient;
  private controlSession?: string;
  private finalReceiver?: LanReceiver;
  private candidateSummary?: {
    before: number;
    mdns: number;
    published: number;
    mdnsPublished: number;
  };
  private id?: string;
  private pc?: RTCPeerConnection;
  private timer?: ReturnType<typeof setTimeout>;
  private stopped = false;
  private stopping?: Promise<void>;
  private started = performance.now();
  private firstFrame: number | null = null;
  private lastMetric = 0;
  private controlErrorDeadline?: number;
  private config?: LanConfig;
  constructor(
    private video: HTMLVideoElement,
    private update: (
      state: string,
      status?: LanStatus,
      receiver?: LanReceiver,
    ) => void,
    private failed: (error: string) => void,
    private code: (code: string) => void,
    private saved: (path: string) => void,
  ) {}

  async start(config: LanConfig) {
    this.config = { ...config, pairingCode: undefined };
    this.update('starting');
    try {
      const result = await invoke<{
        sessionId: string;
        pairingCode: string | null;
        controlSession?: string | null;
      }>('p004_lan_start', { config });
      config.pairingCode = undefined;
      this.id = result.sessionId;
      this.controlSession = result.controlSession ?? undefined;
      if (this.stopped) {
        await invoke('p004_lan_stop', { sessionId: this.id });
        return;
      }
      if (result.pairingCode) this.code(result.pairingCode);
      if (config.role === 'receiver') await this.connectReceiver(config);
      if (!this.stopped) await this.poll();
    } catch (error) {
      if (!this.stopped) this.failed(String(error));
      await this.stop();
    } finally {
      config.pairingCode = undefined;
    }
  }
  private async connectReceiver(config: LanConfig) {
    const pc = new RTCPeerConnection({
      iceServers: [],
      bundlePolicy: 'max-bundle',
    });
    this.pc = pc;
    if (config.controlSim || config.realControl) {
      if (!this.controlSession) throw new Error('CONTROL_CONTEXT_MISSING');
      this.control = new ControlClient(
        pc,
        this.controlSession,
        (code) => {
          if (!this.stopped) {
            this.failed(code);
            void this.stop();
          }
        },
        config.realControl ?? false,
        config.realControl
          ? () => {
              // Local input is already revoked. Give authenticated teardown a
              // short, bounded opportunity to explain this transport event.
              this.controlErrorDeadline = performance.now() + 1000;
            }
          : undefined,
      );
    }
    pc.ontrack = (event) => {
      if (this.stopped) return;
      this.video.srcObject = event.streams[0] ?? new MediaStream([event.track]);
      void this.video.play().catch((error) => {
        if (!this.stopped) {
          this.failed(String(error));
          void this.stop();
        }
      });
    };
    const transceiver = pc.addTransceiver('video', { direction: 'recvonly' });
    const codecs = RTCRtpReceiver.getCapabilities('video')?.codecs.filter(
      (c) =>
        c.mimeType.toLowerCase() === 'video/h264' &&
        c.sdpFmtpLine?.includes('profile-level-id=42001f') &&
        c.sdpFmtpLine.includes('packetization-mode=1'),
    );
    if (!codecs?.length)
      throw new Error('WEBVIEW_H264_BASELINE_MODE1_UNAVAILABLE');
    transceiver.setCodecPreferences(codecs);
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
    const publication =
      config.realControl || config.verifiedPeer
        ? publishInputOffer(pc.localDescription!.sdp, config.localIp)
        : undefined;
    this.candidateSummary = publication?.summary;
    if (publication && publication.summary.published === 0)
      throw new Error('INPUT_NO_PUBLISHED_HOST_CANDIDATE');
    const answer = await invoke<string>('p004_lan_signal', {
      sessionId: this.id,
      offer:
        publication?.sdp ?? lanOffer(pc.localDescription!.sdp, config.localIp),
    });
    if (this.stopped) return;
    await pc.setRemoteDescription({ type: 'answer', sdp: answer });
  }
  private async receiverSnapshot(): Promise<LanReceiver> {
    const pc = this.pc!;
    const data = await receiveStats(
      pc,
      this.video,
      this.started,
      this.firstFrame,
    );
    const all = await pc.getStats();
    const peerMatches = selectedPeer(all, this.config!.peerIp);
    let decoderImplementation: string | null = null;
    all.forEach((item) => {
      if (
        item.type === 'inbound-rtp' &&
        item.kind === 'video' &&
        typeof item.decoderImplementation === 'string'
      )
        decoderImplementation = item.decoderImplementation;
    });
    if (peerMatches === false) throw new Error('BROWSER_SELECTED_OFF_PEER');
    if ((data.framesDecoded ?? 0) > 0 && peerMatches !== true)
      throw new Error('BROWSER_PEER_UNVERIFIED');
    if (
      this.firstFrame === null &&
      (data.framesDecoded ?? 0) > 0 &&
      data.videoWidth > 0 &&
      data.currentTime > 0
    )
      this.firstFrame = data.elapsedMs;
    data.firstFrameMs = this.firstFrame;
    return {
      ...data,
      peerMatches,
      decoderImplementation,
      ...(this.control ? { control: this.control.snapshot() } : {}),
    };
  }
  private async poll() {
    if (this.stopped || !this.id) return;
    const productSession = !!(
      this.config?.realControl || this.config?.verifiedPeer
    );
    try {
      let status = await invoke<LanStatus>('p004_lan_stats', {
        sessionId: this.id,
      });
      if (this.stopped) return;
      if (productSession && authenticatedClosing(status)) {
        this.control?.revoke();
        this.controlErrorDeadline = undefined;
      } else if (
        this.controlErrorDeadline !== undefined &&
        performance.now() >= this.controlErrorDeadline &&
        !status.error
      ) {
        this.control?.confirmChannelError();
        return;
      }
      let receiver: LanReceiver | undefined;
      if (this.pc) {
        try {
          receiver = await this.receiverSnapshot();
        } catch (error) {
          if (
            !productSession ||
            !authenticatedClosing(status) ||
            String(error).includes('BROWSER_SELECTED_OFF_PEER')
          )
            throw error;
        }
        if (this.stopped) return;
        if (
          receiver &&
          !status.resources_released &&
          performance.now() - this.lastMetric >= 450
        ) {
          try {
            await invoke('p004_lan_metrics', {
              sessionId: this.id,
              data: receiver,
            });
          } catch (error) {
            if (!productSession || !String(error).includes('RECEIVER_STOPPED'))
              throw error;
            // Stop can land between the status sample and the metrics IPC.
            status = await invoke<LanStatus>('p004_lan_stats', {
              sessionId: this.id,
            });
            if (!authenticatedClosing(status)) throw error;
            this.control?.revoke();
          }
          if (this.stopped) return;
          this.lastMetric = performance.now();
        }
      } else receiver = status.receiver_samples.at(-1);
      const playing =
        (receiver?.framesDecoded ?? 0) > 0 && (receiver?.videoWidth ?? 0) > 0;
      const finishing = productSession && authenticatedClosing(status);
      if (!status.resources_released && !finishing)
        this.control?.beginBatch(
          playing && receiver?.peerMatches === true && !this.video.paused,
        );
      this.update(
        playing && !status.resources_released && !finishing
          ? 'playing'
          : status.state,
        status,
        receiver,
      );
      if (status.error) this.failed(status.error);
      if (status.resources_released) {
        await this.stop();
        return;
      }
      this.timer = setTimeout(() => void this.poll(), 350);
    } catch (error) {
      if (this.stopped) return;
      this.failed(String(error));
      await this.stop();
    }
  }
  stop(): Promise<void> {
    if (this.stopping) return this.stopping;
    this.stopped = true;
    this.control?.enableInput(false);
    this.control?.revoke();
    clearTimeout(this.timer);
    this.code('');
    this.stopping = this.finish();
    return this.stopping;
  }
  enableInput(value: boolean) {
    return this.control?.enableInput(value) ?? false;
  }
  input(event: Record<string, unknown>) {
    return this.control?.input(event) ?? false;
  }
  private async readFinalReceiver(): Promise<LanReceiver | undefined> {
    if ((!this.config?.realControl && !this.config?.verifiedPeer) || !this.pc)
      return undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      return await Promise.race([
        this.receiverSnapshot(),
        new Promise<undefined>((resolve) => {
          timer = setTimeout(resolve, 750);
        }),
      ]);
    } catch {
      return undefined;
    } finally {
      clearTimeout(timer);
    }
  }
  private async finish() {
    this.update('stopping');
    try {
      // Send an authenticated LAN stop before closing the browser channel. Take
      // the final browser reading while the PC is still available.
      const stopNative = this.id
        ? invoke('p004_lan_stop', { sessionId: this.id })
        : Promise.resolve();
      [, this.finalReceiver] = await Promise.all([
        stopNative,
        this.readFinalReceiver(),
      ]);
      await this.control?.close();
      const tracks = this.pc?.getReceivers().map((r) => r.track) ?? [];
      tracks.forEach((t) => t.stop());
      this.pc?.close();
      const closed =
        (!this.pc || this.pc.connectionState === 'closed') &&
        tracks.every((t) => t.readyState === 'ended');
      this.pc = undefined;
      this.video.srcObject = null;
      if (this.id) {
        const status = await invoke<LanStatus>('p004_lan_stats', {
          sessionId: this.id,
        });
        const path = await invoke<string>('p004_lan_save', {
          sessionId: this.id,
          browserClosed: closed,
          ...(this.control ? { control: this.control.snapshot() } : {}),
          ...(this.config?.realControl || this.config?.verifiedPeer
            ? {
                receiverFinal: this.finalReceiver ?? null,
                candidateSummary: this.candidateSummary ?? null,
              }
            : {}),
        });
        this.saved(path);
        this.update(
          status.error ? 'failed' : 'stopped',
          status,
          this.finalReceiver ?? status.receiver_samples.at(-1),
        );
      } else this.update('stopped');
    } catch (error) {
      this.failed(String(error));
      this.update('failed');
    } finally {
      await this.control?.close();
      this.pc?.getReceivers().forEach((r) => r.track.stop());
      this.pc?.close();
      this.pc = undefined;
      this.video.srcObject = null;
    }
  }
}
