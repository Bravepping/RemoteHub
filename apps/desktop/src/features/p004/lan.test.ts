import { afterEach, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import {
  LanProbe,
  lanOffer,
  selectedPeer,
  type LanConfig,
  type LanStatus,
} from './lan';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
afterEach(() => {
  vi.useRealTimers();
  vi.resetAllMocks();
  vi.unstubAllGlobals();
});

function activeLan(nativeStop?: Promise<void>) {
  vi.useFakeTimers();
  const order: string[] = [];
  const metrics = vi.fn<() => void>();
  let status: LanStatus = {
    role: 'receiver',
    state: 'connected',
    elapsed_ms: 2,
    authenticated: true,
    messages_received: 1,
    source: null,
    receiver_samples: [],
    resources_released: false,
    error: null,
  };
  const dc = {
    readyState: 'open',
    bufferedAmount: 0,
    send: vi.fn(),
    close: vi.fn(() => {
      order.push('browser-close');
      Object.defineProperty(dc, 'readyState', { value: 'closed' });
    }),
  } as unknown as RTCDataChannel;
  const browserStats = new Map<string, Record<string, unknown>>([
    ['video', { type: 'inbound-rtp', kind: 'video', framesDecoded: 12 }],
    ['t', { type: 'transport', selectedCandidatePairId: 'p' }],
    ['p', { remoteCandidateId: 'r' }],
    [
      'r',
      {
        address: '10.0.0.3',
        port: 48604,
        candidateType: 'host',
        protocol: 'udp',
      },
    ],
  ]);
  const pc = {
    ontrack: null as ((event: RTCTrackEvent) => void) | null,
    iceGatheringState: 'complete',
    connectionState: 'connected',
    localDescription: {
      sdp: ['v=0', 'a=candidate:1 1 UDP 1 10.0.0.2 48604 typ host'].join(
        '\r\n',
      ),
    },
    createDataChannel: vi.fn(() => dc),
    addTransceiver: () => ({ setCodecPreferences: vi.fn() }),
    setLocalDescription: vi.fn(),
    createOffer: vi.fn(),
    setRemoteDescription: vi.fn(),
    getStats: async () => browserStats,
    getReceivers: () => [],
    close: () => {
      pc.connectionState = 'closed';
    },
  };
  vi.stubGlobal(
    'RTCPeerConnection',
    vi.fn(function () {
      return pc;
    }),
  );
  vi.stubGlobal('RTCRtpReceiver', {
    getCapabilities: () => ({
      codecs: [
        {
          mimeType: 'video/H264',
          sdpFmtpLine: 'profile-level-id=42001f;packetization-mode=1',
        },
      ],
    }),
  });
  vi.mocked(invoke).mockImplementation(async (command) => {
    switch (command) {
      case 'p004_lan_start':
        return {
          sessionId: 'approved',
          pairingCode: null,
          controlSession: 'control',
        };
      case 'p004_lan_signal':
        return 'v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\na=candidate:1 1 UDP 1 10.0.0.3 48604 typ host\r\n';
      case 'p004_lan_stats':
        return status;
      case 'p004_lan_metrics':
        metrics();
        return undefined;
      case 'p004_lan_stop':
        order.push('native-stop');
        if (nativeStop) await nativeStop;
        status = { ...status, state: 'stopped', resources_released: true };
        return undefined;
      case 'p004_lan_save':
        return 'fixture';
      default:
        return undefined;
    }
  });
  const video = document.createElement('video');
  const play = vi.fn(async () => {});
  Object.defineProperties(video, {
    videoWidth: { value: 1280 },
    paused: { value: false },
    srcObject: { value: null, writable: true },
    play: { value: play },
    pause: { value: vi.fn() },
  });
  const update = vi.fn();
  const failed = vi.fn();
  const probe = new LanProbe(video, update, failed, vi.fn(), vi.fn());
  const start = (overrides: Partial<LanConfig> = {}) =>
    probe.start({
      role: 'receiver',
      consent: true,
      realControl: true,
      localIp: '10.0.0.2',
      peerIp: '10.0.0.3',
      seconds: 30,
      port: 48604,
      pairingCode: 'fixture',
      ...overrides,
    });
  return {
    probe,
    pc,
    dc,
    video,
    play,
    browserStats,
    failed,
    update,
    order,
    metrics,
    start,
    state: (changes: Partial<LanStatus>) => {
      status = { ...status, ...changes };
    },
  };
}

function incomingTrack() {
  const track = { stop: vi.fn(), kind: 'video' } as unknown as MediaStreamTrack;
  const stream = { getTracks: () => [track] } as unknown as MediaStream;
  const event = { streams: [stream], track } as unknown as RTCTrackEvent;
  return { event, stream };
}

const verifiedMode = {
  verifiedPeer: true,
  realControl: false,
  controlSim: false,
};

it.each([false, true])(
  'admits verified video only after both native authentication and a matching selected pair (WSS=%s)',
  async (verifiedWss) => {
    const { probe, pc, video, play, start, state, browserStats, failed } =
      activeLan();
    const remote = browserStats.get('r')!;
    browserStats.delete('r');
    state({ authenticated: false, state: 'negotiating' });
    await start({ ...verifiedMode, verifiedWss });
    const { event, stream } = incomingTrack();
    expect(pc.ontrack).toBeTypeOf('function');
    pc.ontrack?.(event);
    expect(video.srcObject).toBeNull();
    expect(play).not.toHaveBeenCalled();

    browserStats.set('r', remote);
    await vi.advanceTimersByTimeAsync(350);
    expect(video.srcObject).toBeNull();
    expect(play).not.toHaveBeenCalled();

    browserStats.delete('r');
    state({ authenticated: true, state: 'connected' });
    await vi.advanceTimersByTimeAsync(350);
    expect(video.srcObject).toBeNull();
    expect(play).not.toHaveBeenCalled();
    expect(failed).not.toHaveBeenCalled();

    browserStats.set('r', remote);
    await vi.advanceTimersByTimeAsync(350);
    expect(video.srcObject).toBe(stream);
    expect(play).toHaveBeenCalledTimes(1);
    await probe.stop();
  },
);

it.each([
  { changes: { port: 48605 }, reason: 'unsigned port' },
  { changes: { address: '10.0.0.4' }, reason: 'off-peer address' },
  { changes: { candidateType: 'srflx' }, reason: 'non-host candidate' },
  { changes: { protocol: 'tcp' }, reason: 'non-UDP candidate' },
])('rejects an actual selected route mismatch: $reason', async ({ changes }) => {
  const { pc, video, play, start, state, browserStats, failed } = activeLan();
  state({ authenticated: false, state: 'negotiating' });
  await start(verifiedMode);
  const { event } = incomingTrack();
  pc.ontrack?.(event);
  browserStats.set('r', { ...browserStats.get('r'), ...changes });
  state({ authenticated: true, state: 'connected' });
  await vi.advanceTimersByTimeAsync(350);
  expect(failed).toHaveBeenCalledTimes(1);
  expect(video.srcObject).toBeNull();
  expect(play).not.toHaveBeenCalled();
  expect(pc.connectionState).toBe('closed');
});

it('clears a verified viewer before native stop resolves and ignores a queued track callback', async () => {
  let resolve!: () => void;
  const promise = new Promise<void>((resolveStop) => {
    resolve = resolveStop;
  });
  const { probe, pc, video, play, start, order } = activeLan(promise);
  await start(verifiedMode);
  const ontrack = pc.ontrack!;
  const { event, stream } = incomingTrack();
  ontrack(event);
  await vi.advanceTimersByTimeAsync(350);
  expect(video.srcObject).toBe(stream);

  const stopping = probe.stop();
  expect(order).toEqual(['native-stop']);
  expect(pc.connectionState).toBe('connected');
  expect(video.srcObject).toBeNull();
  ontrack(incomingTrack().event);
  expect(video.srcObject).toBeNull();
  expect(play).toHaveBeenCalledTimes(1);

  resolve();
  await stopping;
  expect(video.srcObject).toBeNull();
  expect(pc.connectionState).toBe('closed');
});

it('retires an admitted verified video when the actual selected pair disappears', async () => {
  const { pc, video, play, start, browserStats, failed } = activeLan();
  await start(verifiedMode);
  const ontrack = pc.ontrack!;
  const { event, stream } = incomingTrack();
  ontrack(event);
  await vi.advanceTimersByTimeAsync(350);
  expect(video.srcObject).toBe(stream);

  browserStats.delete('r');
  await vi.advanceTimersByTimeAsync(350);
  expect(video.srcObject).toBeNull();
  expect(failed).toHaveBeenCalledTimes(1);
  ontrack(incomingTrack().event);
  expect(video.srcObject).toBeNull();
  expect(play).toHaveBeenCalledTimes(1);
});

it('retires admitted verified video as soon as native reports authenticated finishing', async () => {
  const { probe, pc, video, play, start, state, failed } = activeLan();
  await start(verifiedMode);
  const ontrack = pc.ontrack!;
  const { event, stream } = incomingTrack();
  ontrack(event);
  await vi.advanceTimersByTimeAsync(350);
  expect(video.srcObject).toBe(stream);

  state({ state: 'finishing' });
  await vi.advanceTimersByTimeAsync(350);
  expect(video.srcObject).toBeNull();
  expect(pc.connectionState).toBe('connected');
  expect(failed).not.toHaveBeenCalled();
  ontrack(incomingTrack().event);
  await vi.advanceTimersByTimeAsync(350);
  expect(video.srcObject).toBeNull();
  expect(play).toHaveBeenCalledTimes(1);
  await probe.stop();
});

it('requests authenticated native stop before closing the browser DataChannel', async () => {
  const { probe, dc, order, start } = activeLan();
  await start();
  expect(probe.enableInput(true)).toBe(true);
  const stopping = probe.stop();
  expect(probe.enableInput(true)).toBe(false);
  expect(dc.close).not.toHaveBeenCalled();
  await stopping;
  expect(order).toEqual(['native-stop', 'browser-close']);
});

it('uses only scoped video, no control DataChannel, and tolerates a verified stop during stats', async () => {
  const { probe, pc, failed, start, state, metrics } = activeLan();
  pc.localDescription.sdp =
    'v=0\r\na=candidate:1 1 UDP 1 12345678-abcd-abcd-abcd-123456789012.local 48604 typ host';
  await start({ verifiedPeer: true, realControl: false, controlSim: false });
  expect(pc.createDataChannel).not.toHaveBeenCalled();
  expect(probe.enableInput(true)).toBe(false);
  expect(invoke).toHaveBeenCalledWith('p004_lan_signal', {
    sessionId: 'approved',
    offer: 'v=0\r\na=candidate:1 1 UDP 1 10.0.0.2 48604 typ host',
  });
  metrics.mockImplementationOnce(() => {
    state({ state: 'stopped', resources_released: true });
    throw 'RECEIVER_STOPPED';
  });
  await vi.advanceTimersByTimeAsync(1050);
  expect(metrics).toHaveBeenCalledTimes(1);
  expect(pc.connectionState).toBe('closed');
  expect(failed).not.toHaveBeenCalled();
});

it('revokes immediately and retains a raw error event during authenticated host finish', async () => {
  const { probe, dc, failed, start, state, update } = activeLan();
  await start();
  expect(probe.enableInput(true)).toBe(true);
  dc.onerror?.(new Event('error') as RTCErrorEvent);
  expect(probe.enableInput(true)).toBe(false);
  state({ state: 'finishing' });
  await vi.advanceTimersByTimeAsync(350);
  expect(update).toHaveBeenLastCalledWith(
    'finishing',
    expect.anything(),
    expect.objectContaining({
      control: expect.objectContaining({
        sendClosed: true,
        errorEvents: 1,
        lastError: null,
      }),
    }),
  );
  state({ state: 'stopped', resources_released: true });
  await vi.advanceTimersByTimeAsync(350);
  expect(failed).not.toHaveBeenCalled();
  expect(dc.close).toHaveBeenCalledTimes(1);
});

it('does not dismiss a channel error when authenticated finish is missing', async () => {
  const { dc, failed, start } = activeLan();
  await start();
  dc.onerror?.(new Event('error') as RTCErrorEvent);
  await vi.advanceTimersByTimeAsync(1050);
  expect(failed).toHaveBeenCalledWith('CONTROL_CHANNEL_ERROR');
  expect(dc.close).toHaveBeenCalledTimes(1);
});

it('does not report a failure if native stop beats a browser metrics sample', async () => {
  const { dc, failed, start, state, metrics } = activeLan();
  await start();
  metrics.mockImplementationOnce(() => {
    state({ state: 'stopped', resources_released: true });
    throw 'RECEIVER_STOPPED';
  });
  await vi.advanceTimersByTimeAsync(1050);
  expect(metrics).toHaveBeenCalledTimes(1);
  expect(dc.close).toHaveBeenCalledTimes(1);
  expect(failed).not.toHaveBeenCalled();
});
it('removes every candidate except the explicitly selected local UDP interface', () => {
  const sdp = [
    'v=0',
    'c=IN IP4 10.0.0.2',
    'a=candidate:1 1 UDP 1 10.0.0.2 9 typ host',
    'a=candidate:2 1 UDP 1 10.0.0.3 9 typ host',
    'a=candidate:3 1 UDP 1 device.local 9 typ host',
    'a=candidate:4 1 TCP 1 10.0.0.2 9 typ host',
  ].join('\r\n');
  const filtered = lanOffer(sdp, '10.0.0.2');
  expect(filtered).toContain('10.0.0.2 9 typ host');
  expect(filtered).not.toMatch(/10\.0\.0\.3|device.local|TCP/);
  expect(filtered).toContain('c=IN IP4 0.0.0.0');
});
it('checks the actual selected remote candidate rather than connection state', () => {
  const all = new Map([
    ['t', { type: 'transport', selectedCandidatePairId: 'p' }],
    ['p', { remoteCandidateId: 'r' }],
    ['r', { address: '10.0.0.2', protocol: 'udp' }],
  ]) as unknown as RTCStatsReport;
  expect(selectedPeer(all, '10.0.0.2')).toBe(true);
  expect(selectedPeer(all, '10.0.0.3')).toBe(false);
  expect(
    selectedPeer(new Map() as unknown as RTCStatsReport, '10.0.0.2'),
  ).toBeNull();
});
it('revokes native authorization that arrives after the receiver was stopped', async () => {
  let approve!: (value: { sessionId: string; pairingCode: null }) => void;
  vi.mocked(invoke)
    .mockReturnValueOnce(
      new Promise((resolve) => {
        approve = resolve;
      }),
    )
    .mockResolvedValue(undefined);
  const pc = vi.fn();
  vi.stubGlobal('RTCPeerConnection', pc);
  const failed = vi.fn();
  const probe = new LanProbe(
    document.createElement('video'),
    vi.fn(),
    failed,
    vi.fn(),
    vi.fn(),
  );
  const starting = probe.start({
    role: 'receiver',
    consent: true,
    localIp: '10.0.0.2',
    peerIp: '10.0.0.3',
    seconds: 30,
    port: 48604,
    pairingCode: 'test',
  });
  await probe.stop();
  approve({ sessionId: 'late', pairingCode: null });
  await starting;
  expect(pc).not.toHaveBeenCalled();
  expect(invoke).toHaveBeenLastCalledWith('p004_lan_stop', {
    sessionId: 'late',
  });
  expect(failed).not.toHaveBeenCalled();
});
