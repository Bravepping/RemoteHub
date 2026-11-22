import { afterEach, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { LanProbe, lanOffer, selectedPeer, type LanStatus } from './lan';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
afterEach(() => {
  vi.useRealTimers();
  vi.resetAllMocks();
  vi.unstubAllGlobals();
});

function activeLan() {
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
  const browserStats = new Map([
    ['video', { type: 'inbound-rtp', kind: 'video', framesDecoded: 12 }],
    ['t', { type: 'transport', selectedCandidatePairId: 'p' }],
    ['p', { remoteCandidateId: 'r' }],
    ['r', { address: '10.0.0.3', protocol: 'udp' }],
  ]);
  const pc = {
    iceGatheringState: 'complete',
    connectionState: 'connected',
    localDescription: {
      sdp: ['v=0', 'a=candidate:1 1 UDP 1 10.0.0.2 48604 typ host'].join(
        '\r\n',
      ),
    },
    createDataChannel: () => dc,
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
        return 'answer';
      case 'p004_lan_stats':
        return status;
      case 'p004_lan_metrics':
        metrics();
        return undefined;
      case 'p004_lan_stop':
        order.push('native-stop');
        status = { ...status, state: 'stopped', resources_released: true };
        return undefined;
      case 'p004_lan_save':
        return 'fixture';
      default:
        return undefined;
    }
  });
  const video = document.createElement('video');
  Object.defineProperties(video, {
    videoWidth: { value: 1280 },
    paused: { value: false },
  });
  const update = vi.fn();
  const failed = vi.fn();
  const probe = new LanProbe(video, update, failed, vi.fn(), vi.fn());
  const start = () =>
    probe.start({
      role: 'receiver',
      consent: true,
      realControl: true,
      localIp: '10.0.0.2',
      peerIp: '10.0.0.3',
      seconds: 30,
      port: 48604,
      pairingCode: 'fixture',
    });
  return {
    probe,
    dc,
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
