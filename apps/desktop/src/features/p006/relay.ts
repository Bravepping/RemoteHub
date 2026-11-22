import { invoke } from '@tauri-apps/api/core';
import { ControlClient } from '../p004/control';
import { receiveStats } from '../p004/webrtc';
import type { LanStatus, LanReceiver } from '../p004/lan';
import { relayOptions, type IceConfig } from './ice';
import {
  evidenceCode,
  evidenceBounded,
  type ReceiverEvidence,
} from './evidence';
import {
  directOptions,
  directSdp,
  directAnswerCandidates,
  selectedDirect,
  type DirectScope,
  type Candidate,
} from './direct';

interface Ready {
  mode?: 'relay_only' | 'direct_preferred';
  directScope?: DirectScope;
  controlSession: string;
  serverIp: string;
  expiresAt: number;
  ice: IceConfig;
}
export function relaySdp(sdp: string, ip: string): string {
  let count = 0;
  const result = sdp
    .split(/\r?\n/)
    .filter((line) => {
      if (!line.startsWith('a=candidate:')) return true;
      const p = line.slice(12).trim().split(/\s+/);
      const accepted =
        p[1] === '1' &&
        p[2]?.toLowerCase() === 'udp' &&
        p[4] === ip &&
        p[7] === 'relay' &&
        Number(p[5]) >= 49160 &&
        Number(p[5]) <= 49259;
      if (accepted) count++;
      return accepted;
    })
    .map((line) => (line.startsWith('c=') ? 'c=IN IP4 0.0.0.0' : line))
    .join('\r\n');
  if (!count) throw new Error('BROWSER_RELAY_CANDIDATE_MISSING');
  return result;
}
export function selectedRelay(all: RTCStatsReport, ip: string): boolean | null {
  let result: boolean | null = null;
  all.forEach((v) => {
    if (v.type !== 'transport' || !v.selectedCandidatePairId) return;
    const pair = all.get(v.selectedCandidatePairId);
    const a = pair && all.get(pair.localCandidateId),
      b = pair && all.get(pair.remoteCandidateId);
    if (a && b)
      result = [a, b].every(
        (c) =>
          c.candidateType === 'relay' &&
          c.protocol?.toLowerCase() === 'udp' &&
          (c.address ?? c.ip) === ip &&
          c.port >= 49160 &&
          c.port <= 49259,
      );
  });
  return result;
}
export class RelayProbe {
  private directScope?: DirectScope;
  private localCandidates: Candidate[] = [];
  private remoteCandidates: Candidate[] = [];
  private id?: string;
  private pc?: RTCPeerConnection;
  private control?: ControlClient;
  private stopped = false;
  private stopping?: Promise<void>;
  private timer?: ReturnType<typeof setTimeout>;
  private started = performance.now();
  private firstFrame: number | null = null;
  private lastMetric = 0;
  private server = '';
  private lastReceiver?: { sample: LanReceiver; at: number };
  private failureCodes: string[] = [];
  private receiverRole = false;
  constructor(
    private video: HTMLVideoElement,
    private update: (
      state: string,
      status?: LanStatus,
      receiver?: LanReceiver,
    ) => void,
    private fail: (error: string) => void,
    private code: (code: string) => void,
    private saved: (path: string) => void,
    private directFeatureEnabled = false,
    private call: typeof invoke = invoke,
    private createPeer: (options: RTCConfiguration) => RTCPeerConnection = (
      options,
    ) => new RTCPeerConnection(options),
  ) {
    const notify = this.fail;
    this.fail = (error) => {
      const code = evidenceCode(error);
      if (!this.failureCodes.includes(code) && this.failureCodes.length < 8) {
        this.failureCodes.push(code);
        notify(error);
      }
    };
  }
  async start(config: {
    role: 'sender' | 'receiver';
    consent: boolean;
    localIp: string;
    seconds: number;
    pairingCode?: string;
    mode?: 'relay_only' | 'direct_preferred';
    peerIp?: string;
  }) {
    this.receiverRole = config.role === 'receiver';
    try {
      const result = await this.call<{
        sessionId: string;
        pairingCode: string | null;
      }>('p006_start', { config });
      config.pairingCode = undefined;
      this.id = result.sessionId;
      if (this.stopped) {
        await this.call('p006_stop', { sessionId: this.id });
        return;
      }
      this.code(result.pairingCode ?? '');
      this.update('waiting');
      const ready = await this.call<Ready>('p006_ready', {
        sessionId: this.id,
      });
      if (this.stopped) return;
      this.server = ready.serverIp;
      if (config.role === 'receiver') await this.connect(ready);
      if (!this.stopped) await this.poll();
    } catch (e) {
      if (!this.stopped && !(await this.completedNormally(e)))
        this.fail(String(e));
      await this.stop();
    } finally {
      config.pairingCode = undefined;
    }
  }
  private async connect(ready: Ready) {
    const direct = ready.mode === 'direct_preferred';
    if (direct && (!ready.directScope || !this.directFeatureEnabled))
      throw new Error('DIRECT_MODE_NOT_ENABLED');
    this.directScope = direct ? ready.directScope : undefined;
    const options = (direct ? directOptions : relayOptions)(
      ready.ice,
      {
        bothApproved: true,
        sessionId: ready.controlSession,
        expiresAt: ready.expiresAt,
        serverIp: ready.serverIp,
        serverPort: 3478,
      },
      Math.floor(Date.now() / 1000),
      direct ? this.directFeatureEnabled : true,
    );
    const pc = this.createPeer(options);
    this.pc = pc;
    // Credentials are now owned by the PC; do not retain the IPC result.
    ready.ice.ice_servers = [];
    this.control = new ControlClient(pc, ready.controlSession, (e) => {
      this.fail(e);
      void this.stop();
    });
    pc.ontrack = (event) => {
      if (this.stopped) return;
      this.video.srcObject = event.streams[0] ?? new MediaStream([event.track]);
      void this.video.play().catch((e) => {
        if (!this.stopped) {
          this.fail(String(e));
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
    if (!codecs?.length) throw new Error('H264_BASELINE_UNAVAILABLE');
    transceiver.setCodecPreferences(codecs);
    this.update('negotiating');
    await pc.setLocalDescription(await pc.createOffer());
    await new Promise<void>((resolve, reject) => {
      if (pc.iceGatheringState === 'complete') return resolve();
      const timer = setTimeout(() => {
        pc.onicegatheringstatechange = null;
        if (direct) resolve();
        else reject(new Error('BROWSER_RELAY_GATHER_TIMEOUT'));
      }, 10000);
      pc.onicegatheringstatechange = () => {
        if (pc.iceGatheringState === 'complete') {
          clearTimeout(timer);
          pc.onicegatheringstatechange = null;
          resolve();
        }
      };
    });
    if (this.stopped) return;
    const offer = this.directScope
      ? directSdp(pc.localDescription!.sdp, this.directScope)
      : null;
    if (offer) this.localCandidates = offer.candidates;
    const answer = await this.call<string>('p006_signal', {
      sessionId: this.id,
      offer: offer?.sdp ?? relaySdp(pc.localDescription!.sdp, this.server),
    });
    if (!this.stopped) {
      if (this.directScope)
        this.remoteCandidates = directAnswerCandidates(
          answer,
          this.directScope,
        );
      await pc.setRemoteDescription({ type: 'answer', sdp: answer });
    }
  }
  private async completedNormally(error: unknown): Promise<boolean> {
    const code = String(error).replace(/^Error:\s*/, '');
    if (
      !this.id ||
      !['P006_STOPPED', 'P006_RECEIVER_STOPPED', 'P006_SIGNAL_CLOSED'].includes(
        code,
      )
    )
      return false;
    try {
      const s = await this.call<LanStatus>('p006_stats', {
        sessionId: this.id,
      });
      return s.resources_released && !s.error;
    } catch {
      return false;
    }
  }
  private async poll() {
    if (this.stopped || !this.id) return;
    try {
      const status = await this.call<LanStatus>('p006_stats', {
        sessionId: this.id,
      });
      if (this.stopped) return;
      let receiver = status.receiver_samples.at(-1);
      if (this.pc) {
        receiver = await this.receiverStats();
        if (this.stopped) return;
        this.lastReceiver = { sample: receiver, at: Date.now() };
        const playing = (receiver.framesDecoded ?? 0) > 0 && !this.video.paused;
        if (!status.resources_released)
          this.control?.beginBatch(playing && receiver.peerMatches === true);
        if (
          !status.resources_released &&
          performance.now() - this.lastMetric > 700
        ) {
          await this.call('p006_metrics', {
            sessionId: this.id,
            data: receiver,
          });
          this.lastMetric = performance.now();
        }
      }
      if (this.stopped) return;
      if (status.error) this.fail(status.error);
      if (status.resources_released) {
        await this.stop();
        return;
      }
      this.update(
        (receiver?.framesDecoded ?? 0) > 0 && !status.resources_released
          ? 'playing'
          : status.state,
        status,
        receiver,
      );
      this.timer = setTimeout(() => void this.poll(), 350);
    } catch (e) {
      if (!this.stopped && !(await this.completedNormally(e)))
        this.fail(String(e));
      await this.stop();
    }
  }
  private async receiverStats(strict = true): Promise<LanReceiver> {
    const pc = this.pc;
    if (!pc) throw new Error('BROWSER_PC_MISSING');
    const stats = await pc.getStats();
    const data = await receiveStats(
      pc,
      this.video,
      this.started,
      this.firstFrame,
      stats,
    );
    const directPath = this.directScope
      ? selectedDirect(stats, this.localCandidates, this.remoteCandidates)
      : null;
    const peerMatches = directPath
      ? directPath.verified
      : selectedRelay(stats, this.server);
    const invalid =
      peerMatches === false ||
      ((data.framesDecoded ?? 0) > 0 && peerMatches !== true);
    if (invalid && strict)
      throw new Error(
        this.directScope
          ? 'BROWSER_SELECTED_DIRECT_PATH_UNVERIFIED'
          : 'BROWSER_SELECTED_RELAY_UNVERIFIED',
      );
    if (this.firstFrame === null && (data.framesDecoded ?? 0) > 0)
      this.firstFrame = data.elapsedMs;
    const result: LanReceiver = {
      ...(directPath ? { connectionPath: directPath.path } : {}),
      ...data,
      firstFrameMs: this.firstFrame,
      peerMatches,
      decoderImplementation: null,
      control: this.control?.snapshot(),
    };
    return result;
  }
  stop(): Promise<void> {
    if (this.stopping) return this.stopping;
    this.stopped = true;
    clearTimeout(this.timer);
    this.code('');
    this.stopping = this.finish();
    return this.stopping;
  }
  private async finish() {
    this.update('stopping');
    this.control?.revoke();
    const nativeStop = this.id
      ? Promise.resolve().then(() =>
          this.call('p006_stop', { sessionId: this.id }),
        )
      : undefined;
    void nativeStop?.catch(() => {});
    const errors = this.failureCodes;
    const attempt = async <T>(
      step: () => Promise<T> | T,
    ): Promise<T | undefined> => {
      try {
        return await step();
      } catch (e) {
        const code = evidenceCode(e);
        this.fail(code);
        return undefined;
      }
    };
    let capture: ReceiverEvidence['capture'] = 'unavailable';
    const captureNow = async () => {
      const current = await attempt(() =>
        evidenceBounded(this.receiverStats(false)),
      );
      if (current) {
        this.lastReceiver = { sample: current, at: Date.now() };
        capture = 'live';
        if (
          current.peerMatches === false ||
          ((current.framesDecoded ?? 0) > 0 && current.peerMatches !== true)
        )
          this.fail(
            `Error: ${this.directScope ? 'BROWSER_SELECTED_DIRECT_PATH_UNVERIFIED' : 'BROWSER_SELECTED_RELAY_UNVERIFIED'}`,
          );
      } else if (this.lastReceiver) capture = 'last_known';
    };
    // A real final read happens while this browser PC and tracks still exist.
    if (this.pc) {
      await captureNow();
    }
    if (!this.pc && this.lastReceiver) capture = 'last_known';
    let status: LanStatus | undefined;
    const cleanup: ReceiverEvidence['cleanup'] = {
      controlClosed: null,
      peerClosed: this.pc ? this.pc.connectionState === 'closed' : null,
      tracksStopped: null,
      nativeResourcesReleased: null,
    };
    const save = async (stage: ReceiverEvidence['stage']) => {
      if (!this.id || !this.receiverRole) return;
      const sample = this.lastReceiver;
      const receiverEvidence: ReceiverEvidence = {
        version: 1,
        stage,
        capture,
        sampledAtMs: sample?.at ?? null,
        recordedAtMs: Date.now(),
        snapshot: sample?.sample ?? null,
        errors: [...errors].slice(0, 8),
        cleanup: { ...cleanup },
      };
      const result = await attempt(async () => {
        const path = await evidenceBounded(
          this.call<string>('p006_save', {
            sessionId: this.id,
            receiverEvidence,
          }),
        );
        this.saved(path);
        return true;
      });
      return result === true;
    };
    const preSaved = await save('pre_close');
    await attempt(async () => {
      await evidenceBounded(this.control?.close() ?? Promise.resolve());
      cleanup.controlClosed = this.control
        ? ['closed', 'failed'].includes(this.control.snapshot().state)
        : null;
    });
    if (nativeStop) await attempt(() => evidenceBounded(nativeStop, 15000));
    // Capture drained frames too, before close. Preserve the earlier durable file.
    if (this.pc) {
      await captureNow();
    }
    if (this.receiverRole && !preSaved) await save('pre_close');
    await attempt(() => {
      this.pc?.getReceivers().forEach((r) => r.track.stop());
      cleanup.tracksStopped = this.pc ? true : null;
    });
    await attempt(() => {
      this.pc?.close();
      cleanup.peerClosed = this.pc
        ? this.pc.connectionState === 'closed'
        : null;
    });
    this.video.srcObject = null;
    if (this.id)
      status = await attempt(() =>
        evidenceBounded(
          this.call<LanStatus>('p006_stats', { sessionId: this.id }),
        ),
      );
    if (status?.error && !errors.includes(evidenceCode(status.error)))
      this.fail(status.error);
    cleanup.nativeResourcesReleased = status?.resources_released ?? null;
    await save('cleanup');
    if (this.id)
      await attempt(async () => {
        this.saved(
          await evidenceBounded(
            this.call<string>('p006_save', {
              sessionId: this.id,
              browserClosed: !this.pc || this.pc.connectionState === 'closed',
              control: this.control?.snapshot(),
            }),
          ),
        );
      });
    if (status?.error && !errors.includes(status.error))
      errors.push(evidenceCode(status.error));
    this.update(
      errors.length || status?.error ? 'failed' : 'stopped',
      status,
      this.lastReceiver?.sample ?? status?.receiver_samples.at(-1),
    );
    this.pc = undefined;
  }
}
