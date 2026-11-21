const loopback = '127.0.0.1';
const iceWaitMs = 5_000;
const maxSdpLength = 16_384;
const profile = /^42[0-9a-f]{2}1f$/i;

function isLoopbackCandidate(line: string): boolean {
  const fields = line.slice('a=candidate:'.length).split(/[ \t]+/);
  const port = Number(fields[5]);
  return (
    fields.length >= 8 &&
    fields[1] === '1' &&
    fields[2].toLowerCase() === 'udp' &&
    fields[4] === loopback &&
    /^\d{1,5}$/.test(fields[5]) &&
    port >= 1 &&
    port <= 65535 &&
    fields[6] === 'typ' &&
    fields[7] === 'host' &&
    !fields.slice(8).some((field) => /^raddr$|^rport$/i.test(field))
  );
}

function safeDefaultAddress(address: string | undefined): boolean {
  return address === '0.0.0.0' || address === loopback;
}

/** Offer: reject off-host addresses. Answer: do not disclose gathered LAN candidates. */
export function loopbackSdp(sdp: string, type: 'offer' | 'answer'): string {
  if (sdp.length > maxSdpLength || /[^\t\n\r\x20-\x7e]|\r(?!\n)/.test(sdp)) {
    throw new Error('SIGNAL_INVALID');
  }
  const lines = sdp.split(/\r?\n/).filter(Boolean);
  if (lines.some((line) => line !== line.trim())) {
    throw new Error('SIGNAL_INVALID');
  }
  const media = lines.filter((line) => line.startsWith('m='));
  const fmtp = lines.find((line) => line.startsWith('a=fmtp:96 '));
  const codecParams = new Map(
    fmtp
      ?.slice('a=fmtp:96 '.length)
      .split(';')
      .map(
        (parameter) =>
          parameter.trim().split('=').slice(0, 2) as [string, string],
      ),
  );
  const directions = lines.filter((line) =>
    /^a=(sendonly|recvonly|inactive|sendrecv)$/.test(line),
  );
  if (
    media.length !== 1 ||
    !/^m=video [1-9]\d{0,4} UDP\/TLS\/RTP\/SAVPF 96$/.test(media[0]) ||
    !lines.includes('a=rtcp-mux') ||
    directions.length !== 1 ||
    directions[0] !== (type === 'offer' ? 'a=sendonly' : 'a=recvonly') ||
    !lines.some((line) => /^a=rtpmap:96 H264\/90000$/i.test(line)) ||
    codecParams.get('packetization-mode') !== '1' ||
    !profile.test(codecParams.get('profile-level-id') ?? '')
  ) {
    throw new Error('SIGNAL_UNSUPPORTED_VIDEO');
  }
  const candidates: string[] = [];
  const safe = lines.filter((line) => {
    if (line.startsWith('a=candidate:')) {
      if (isLoopbackCandidate(line)) {
        candidates.push(line);
        return true;
      }
      if (type === 'offer') throw new Error('ICE_OFFER_NOT_LOOPBACK');
      return false;
    }
    if (line === 'a=ice-lite' || line.startsWith('a=remote-candidates:')) {
      throw new Error('ICE_OFFER_NOT_LOOPBACK');
    }
    if (line.startsWith('a=ice-options:') || line === 'a=end-of-candidates') {
      // Exchange complete descriptions over IPC without trickle or ICE restarts.
      return false;
    }
    const fields = line.split(' ');
    if (type === 'answer') return true;
    if (
      (line.startsWith('c=') && !safeDefaultAddress(fields[2])) ||
      (line.startsWith('a=rtcp:') &&
        fields.length > 1 &&
        !safeDefaultAddress(fields[3]))
    ) {
      throw new Error('ICE_DEFAULT_NOT_LOOPBACK');
    }
    return true;
  });
  if (candidates.length === 0) throw new Error('ICE_LOOPBACK_UNAVAILABLE');
  if (type === 'answer') {
    // The complete WebView2 description may pick a LAN default even if it
    // gathered a loopback candidate. Advertise only the verified local socket.
    const port = candidates[0].split(/[ \t]+/)[5];
    for (const [index, line] of safe.entries()) {
      if (line.startsWith('m=video ')) {
        safe[index] = line.replace(/^m=video \d+ /, `m=video ${port} `);
        if (!safe.some((value) => value.startsWith('c='))) {
          safe.splice(index + 1, 0, `c=IN IP4 ${loopback}`);
        }
      } else if (line.startsWith('c=')) {
        safe[index] = `c=IN IP4 ${loopback}`;
      } else if (line.startsWith('a=rtcp:')) {
        safe[index] = `a=rtcp:${port} IN IP4 ${loopback}`;
      }
    }
  }
  return [...safe, 'a=end-of-candidates', ''].join('\r\n');
}

function waitForIce(pc: RTCPeerConnection, signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal.aborted) {
      reject(new Error('PROBE_STOPPED'));
      return;
    }
    if (pc.iceGatheringState === 'complete') {
      resolve();
      return;
    }

    function finish(error?: Error) {
      clearTimeout(timer);
      signal.removeEventListener('abort', abort);
      pc.removeEventListener('icegatheringstatechange', update);
      if (error) reject(error);
      else resolve();
    }

    function update() {
      if (pc.iceGatheringState === 'complete') finish();
    }

    function abort() {
      finish(new Error('PROBE_STOPPED'));
    }

    const timer = setTimeout(
      () => finish(new Error('ICE_GATHER_TIMEOUT')),
      iceWaitMs,
    );
    signal.addEventListener('abort', abort, { once: true });
    pc.addEventListener('icegatheringstatechange', update);
  });
}

export interface BrowserStats {
  width?: number;
  height?: number;
  framesDecoded?: number;
  framesRendered?: number;
  packetsReceived?: number;
  packetsLost?: number;
  codec?: string;
  localCandidateType?: string;
  remoteCandidateType?: string;
  candidateProtocol?: string;
  candidateScope?: 'loopback' | 'unverified' | 'off-host';
  firstFrameMs?: number;
  playbackMs?: number;
  iceState: RTCIceConnectionState;
  peerState: RTCPeerConnectionState;
}

const number = (value: unknown): number | undefined =>
  typeof value === 'number' && Number.isFinite(value) ? value : undefined;

const text = (value: unknown): string | undefined =>
  typeof value === 'string' ? value : undefined;

export function browserMeasurements(
  report: RTCStatsReport,
): Omit<BrowserStats, 'peerState' | 'iceState'> {
  let inbound: Record<string, unknown> | undefined;
  report.forEach((entry) => {
    if (
      entry.type === 'inbound-rtp' &&
      (entry.kind === 'video' || entry.mediaType === 'video') &&
      (!inbound ||
        (number(entry.packetsReceived) ?? -1) >
          (number(inbound.packetsReceived) ?? -1))
    ) {
      inbound = entry;
    }
  });

  const codec = report.get(text(inbound?.codecId) ?? '');
  let transport = report.get(text(inbound?.transportId) ?? '');
  if (!transport) {
    report.forEach((entry) => {
      if (entry.type === 'transport' && entry.selectedCandidatePairId)
        transport = entry;
    });
  }
  const pair = report.get(text(transport?.selectedCandidatePairId) ?? '');
  const local = report.get(text(pair?.localCandidateId) ?? '');
  const remote = report.get(text(pair?.remoteCandidateId) ?? '');
  const localAddress = text(local?.address);
  const remoteAddress = text(remote?.address);
  const bothLoopback = localAddress === loopback && remoteAddress === loopback;
  const knownOffHost = [localAddress, remoteAddress].some(
    (address) =>
      address !== undefined &&
      address !== '' &&
      address !== loopback &&
      !address.endsWith('.local'),
  );

  return {
    framesDecoded: number(inbound?.framesDecoded),
    packetsReceived: number(inbound?.packetsReceived),
    packetsLost: number(inbound?.packetsLost),
    codec: text(codec?.mimeType),
    localCandidateType: text(local?.candidateType),
    remoteCandidateType: text(remote?.candidateType),
    candidateProtocol: text(remote?.protocol),
    candidateScope: pair
      ? bothLoopback
        ? 'loopback'
        : knownOffHost
          ? 'off-host'
          : 'unverified'
      : undefined,
  };
}

export class LoopbackReceiver {
  private readonly pc = new RTCPeerConnection({
    iceServers: [],
    bundlePolicy: 'max-bundle',
    rtcpMuxPolicy: 'require',
  });
  private frameCallback?: number;
  private firstFrame?: number;
  private lastFrame?: number;
  private framesRendered?: number;
  private connectingAt?: number;
  private videoPlaying = false;
  private closed = false;

  constructor(
    private readonly video: HTMLVideoElement,
    onFailure: (code: string) => void,
  ) {
    this.pc.ontrack = ({ track }) => {
      if (this.closed || track.kind !== 'video') return;
      // Build a stream from the negotiated video track only.
      this.video.srcObject = new MediaStream([track]);
      if (typeof this.video.requestVideoFrameCallback === 'function') {
        const observe: VideoFrameRequestCallback = (timestamp, frame) => {
          if (this.closed) return;
          this.firstFrame ??= timestamp;
          this.lastFrame = timestamp;
          this.framesRendered = frame.presentedFrames;
          this.frameCallback = this.video.requestVideoFrameCallback(observe);
        };
        this.frameCallback = this.video.requestVideoFrameCallback(observe);
      }
      void this.video.play().then(
        () => {
          this.videoPlaying = !this.closed;
        },
        () => {
          if (!this.closed) onFailure('VIDEO_PLAY_FAILED');
        },
      );
    };
  }

  async negotiate(offer: string, signal: AbortSignal): Promise<string> {
    const sdp = loopbackSdp(offer, 'offer');
    this.connectingAt = performance.now();
    try {
      await this.pc.setRemoteDescription({ type: 'offer', sdp });
      signal.throwIfAborted();
      const [transceiver] = this.pc.getTransceivers();
      if (!transceiver || this.pc.getTransceivers().length !== 1)
        throw new Error('SIGNAL_UNSUPPORTED_VIDEO');
      transceiver.direction = 'recvonly';
      const answer = await this.pc.createAnswer();
      signal.throwIfAborted();
      await this.pc.setLocalDescription(answer);
    } catch {
      throw new Error('SIGNAL_NEGOTIATION_FAILED');
    }
    await waitForIce(this.pc, signal);
    return loopbackSdp(this.pc.localDescription?.sdp ?? '', 'answer');
  }

  async sample(): Promise<BrowserStats> {
    const report = browserMeasurements(await this.pc.getStats());
    // Feature detection: without requestVideoFrameCallback, use the real decoder
    // count for state, but do not invent presentation timing.
    const presented = this.framesRendered !== undefined;
    const decoded = this.videoPlaying && (report.framesDecoded ?? 0) > 0;
    return {
      ...report,
      width:
        presented || decoded ? this.video.videoWidth || undefined : undefined,
      height:
        presented || decoded ? this.video.videoHeight || undefined : undefined,
      iceState: this.pc.iceConnectionState,
      peerState: this.pc.connectionState,
      framesRendered: this.framesRendered,
      firstFrameMs:
        this.firstFrame !== undefined && this.connectingAt !== undefined
          ? Math.round(this.firstFrame - this.connectingAt)
          : undefined,
      playbackMs:
        this.firstFrame !== undefined && this.lastFrame !== undefined
          ? Math.round(this.lastFrame - this.firstFrame)
          : undefined,
    };
  }

  close() {
    if (this.closed) return;
    this.closed = true;
    this.pc.ontrack = null;
    if (this.frameCallback !== undefined)
      this.video.cancelVideoFrameCallback(this.frameCallback);
    this.pc.close();
    this.video.srcObject = null;
  }
}
