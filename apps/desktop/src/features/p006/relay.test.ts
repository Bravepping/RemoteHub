import { expect, it, vi } from 'vitest';
import {
  relayCandidateSet,
  relaySdp,
  selectedRelay,
  RelayProbe,
} from './relay';
import { invoke } from '@tauri-apps/api/core';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

const ip = '8.8.8.8'; // Offline fixture. Tests never contact this address.
it.each([
  ['constructor', 'BROWSER_RELAY_PC_CREATE_FAILED'],
  ['offer', 'BROWSER_RELAY_OFFER_CREATE_FAILED'],
  ['local', 'BROWSER_RELAY_LOCAL_SDP_APPLY_FAILED'],
  ['answer', 'BROWSER_RELAY_ANSWER_APPLY_FAILED'],
] as const)(
  'R06 relay-only reports %s failures without raw browser details',
  async (stage, code) => {
    const until = Math.floor(Date.now() / 1000) + 120;
    const controlSession = '12345678-1234-1234-1234-123456789abc';
    const sdp = `v=0\r\na=candidate:a 1 udp 1 ${ip} 49160 typ relay raddr 0.0.0.0 rport 0\r\n`;
    const raw = new DOMException(
      'private SDP and token fixture',
      'OperationError',
    );
    const failAt = (name: string) => {
      if (stage === name) throw raw;
    };
    const pc = {
      connectionState: 'new',
      iceGatheringState: 'complete',
      createDataChannel: () => ({ readyState: 'closed', close: vi.fn() }),
      addTransceiver: () => ({ setCodecPreferences: vi.fn() }),
      createOffer: async () => {
        failAt('offer');
        return { type: 'offer', sdp };
      },
      setLocalDescription: async () => {
        failAt('local');
      },
      localDescription: { sdp },
      setRemoteDescription: async () => {
        failAt('answer');
      },
      getStats: async () => new Map(),
      getReceivers: () => [],
      close: vi.fn(),
    };
    const call = vi.fn(async (command: string) => {
      if (command === 'p006_start')
        return { sessionId: 'fixed', pairingCode: null };
      if (command === 'p006_ready')
        return {
          mode: 'relay_only',
          serverIp: ip,
          expiresAt: until,
          controlSession,
          ice: {
            mode: 'relay_only',
            expires_at: until,
            session_id: controlSession,
            ice_servers: [
              {
                urls: [`turn:${ip}:3478?transport=udp`],
                username: `${until}:${controlSession}`,
                credential: `${'A'.repeat(27)}=`,
              },
            ],
          },
        };
      if (command === 'p006_signal') return sdp;
      if (command === 'p006_stats')
        return { resources_released: true, error: null, receiver_samples: [] };
      if (command === 'p006_save') return 'fixture';
      return undefined;
    });
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
    try {
      const failed = vi.fn();
      const probe = new RelayProbe(
        document.createElement('video'),
        vi.fn(),
        failed,
        vi.fn(),
        vi.fn(),
        true,
        call as unknown as typeof invoke,
        () => {
          failAt('constructor');
          return pc as unknown as RTCPeerConnection;
        },
      );
      await probe.start(
        {
          role: 'receiver',
          consent: true,
          localIp: '192.168.1.2',
          seconds: 8,
          mode: 'relay_only',
        },
        true,
      );
      expect(failed).toHaveBeenCalledExactlyOnceWith(`Error: ${code}`);
      expect(JSON.stringify(call.mock.calls)).not.toContain('private SDP');
      expect(call).toHaveBeenCalledWith('p006_stop', { sessionId: 'fixed' });
    } finally {
      vi.unstubAllGlobals();
    }
  },
);
it('forwards only approved UDP relay candidates and requires at least one', () => {
  const lines = [
    'v=0',
    'c=IN IP4 192.168.1.2',
    `a=candidate:a 1 udp 1 ${ip} 49160 typ relay`,
    `a=candidate:b 1 udp 1 ${ip} 49260 typ relay`,
    `a=candidate:c 2 udp 1 ${ip} 49161 typ relay`,
    `a=candidate:d 1 tcp 1 ${ip} 49162 typ relay`,
    `a=candidate:e 1 udp 1 8.8.4.4 49163 typ relay`,
    'a=candidate:f 1 udp 1 192.168.1.2 5555 typ host',
  ].join('\r\n');
  const result = relaySdp(lines, ip);
  expect(result.match(/a=candidate:/g)).toHaveLength(1);
  expect(result).toContain('c=IN IP4 0.0.0.0');
  expect(() => relaySdp('v=0', ip)).toThrow('BROWSER_RELAY_CANDIDATE_MISSING');
});
it('requires the actual selected transport pair and both approved relay endpoints', () => {
  const all = new Map<string, Record<string, unknown>>([
    ['transport', { type: 'transport', selectedCandidatePairId: 'pair' }],
    [
      'pair',
      {
        type: 'candidate-pair',
        localCandidateId: 'local',
        remoteCandidateId: 'remote',
      },
    ],
    [
      'local',
      { candidateType: 'relay', protocol: 'udp', address: ip, port: 49160 },
    ],
    [
      'remote',
      { candidateType: 'relay', protocol: 'udp', address: ip, port: 49259 },
    ],
  ]);
  expect(selectedRelay(new Map() as unknown as RTCStatsReport, ip)).toBeNull();
  expect(selectedRelay(all as unknown as RTCStatsReport, ip)).toBe(true);
  for (const wrong of [
    { candidateType: 'host' },
    { protocol: 'tcp' },
    { address: '8.8.4.4' },
    { port: 49260 },
    { port: NaN },
  ]) {
    const altered = new Map(all);
    altered.set('remote', { ...all.get('remote'), ...wrong });
    expect(selectedRelay(altered as unknown as RTCStatsReport, ip)).toBe(false);
  }
});
it('redacts diagnostic TURN SDP origin, related address, RTCP and extensions', () => {
  const sdp = [
    'v=0',
    'o=private-name 123 456 IN IP4 192.168.1.6',
    'c=IN IP4 192.168.1.6',
    'a=rtcp:5444 IN IP4 192.168.1.6',
    'a=rtcp-mux',
    `a=candidate:a 1 UDP 1 ${ip} 49160 typ relay raddr 192.168.1.6 rport 5444 network-id 2 arbitrary private-fixture`,
  ].join('\r\n');
  const result = relaySdp(sdp, ip, true);
  expect(result).toBe(
    [
      'v=0',
      'o=- 123 456 IN IP4 0.0.0.0',
      'c=IN IP4 0.0.0.0',
      'a=rtcp:9 IN IP4 0.0.0.0',
      'a=rtcp-mux',
      `a=candidate:a 1 udp 1 ${ip} 49160 typ relay raddr 0.0.0.0 rport 0`,
    ].join('\r\n'),
  );
  expect(relayCandidateSet(result, ip)).toEqual([
    { address: ip, port: 49160, type: 'relay' },
  ]);
  for (const noncanonical of [
    result.replace(' 1 udp ', ' 1 UDP '),
    result.replace('raddr 0.0.0.0 rport 0', 'raddr 10.0.0.2 rport 5000'),
    result.replace('raddr 0.0.0.0 rport 0', 'generation 0'),
    result.replace('a=candidate:', ' a=candidate:'),
    result.replace('o=- ', 'o=unredacted '),
    result.replace('c=IN IP4 0.0.0.0', 'c=IN IP4 10.0.0.2'),
    result.replace('a=rtcp:9 IN IP4 0.0.0.0', 'a=rtcp:1234 IN IP4 10.0.0.2'),
    `${result}\na=remote-candidates:1 10.0.0.2 5000`,
  ])
    expect(() => relayCandidateSet(noncanonical, ip)).toThrow();
  expect(() =>
    relaySdp(`${sdp}\r\na=remote-candidates:1 192.168.1.6 5444`, ip, true),
  ).toThrow('BROWSER_RELAY_CANDIDATE_UNVERIFIED');
  expect(() =>
    relayCandidateSet(
      `${result}\r\na=candidate:x 1 udp 1 ${ip} 49161 typ host`,
      ip,
    ),
  ).toThrow('BROWSER_RELAY_CANDIDATE_UNVERIFIED');
});

it.each(['direct_preferred', undefined] as const)(
  'closes an R06 relay-only session without creating a PC when authenticated ready mode is %s',
  async (mode) => {
    vi.mocked(invoke)
      .mockClear()
      .mockImplementation(async (command) => {
        if (command === 'p006_start')
          return { sessionId: 'mismatch', pairingCode: null };
        if (command === 'p006_ready') return { mode };
        if (command === 'p006_stats')
          return {
            resources_released: true,
            error: null,
            receiver_samples: [],
          };
        if (command === 'p006_save') return 'fixture';
        return undefined;
      });
    const failure = vi.fn();
    const createPeer = vi.fn();
    const probe = new RelayProbe(
      document.createElement('video'),
      vi.fn(),
      failure,
      vi.fn(),
      vi.fn(),
      true,
      invoke,
      createPeer,
    );
    await probe.start(
      {
        role: 'receiver',
        consent: true,
        localIp: '192.168.1.2',
        seconds: 8,
        mode: 'relay_only',
      },
      true,
    );
    expect(failure).toHaveBeenCalledWith('Error: R06_REQUESTED_MODE_MISMATCH');
    expect(createPeer).not.toHaveBeenCalled();
    expect(invoke).not.toHaveBeenCalledWith('p006_signal', expect.anything());
    expect(invoke).toHaveBeenCalledWith('p006_stop', { sessionId: 'mismatch' });
  },
);

it.each([49161, 49162])(
  'diagnostic relay requires exact authenticated answer port, selected %s',
  async (selectedPort) => {
    const failures = vi.fn();
    const update = vi.fn();
    const probe = new RelayProbe(
      document.createElement('video'),
      update,
      failures,
      vi.fn(),
      vi.fn(),
    );
    const report = new Map([
      ['video', { type: 'inbound-rtp', kind: 'video', framesDecoded: 1 }],
      ['transport', { type: 'transport', selectedCandidatePairId: 'pair' }],
      ['pair', { localCandidateId: 'local', remoteCandidateId: 'remote' }],
      [
        'local',
        { candidateType: 'relay', protocol: 'udp', address: ip, port: 49160 },
      ],
      [
        'remote',
        {
          candidateType: 'relay',
          protocol: 'udp',
          address: ip,
          port: selectedPort,
        },
      ],
    ] as Array<[string, Record<string, unknown>]>);
    const pc = {
      connectionState: 'connected',
      getStats: async () => report,
      getReceivers: () => [],
      close: () => {
        pc.connectionState = 'closed';
      },
    };
    const candidate = (port: number) =>
      `a=candidate:a 1 udp 1 ${ip} ${port} typ relay raddr 0.0.0.0 rport 0`;
    Object.assign(probe, {
      pc,
      server: ip,
      diagnosticMode: 'relay_only',
      localCandidates: relayCandidateSet(candidate(49160), ip),
      remoteCandidates: relayCandidateSet(candidate(49161), ip),
    });
    await probe.stop();
    expect(update).toHaveBeenLastCalledWith(
      selectedPort === 49161 ? 'stopped' : 'failed',
      undefined,
      expect.objectContaining({
        peerMatches: selectedPort === 49161,
        connectionPath: selectedPort === 49161 ? 'relay' : 'unknown',
      }),
    );
    if (selectedPort === 49161) expect(failures).not.toHaveBeenCalled();
    else
      expect(failures).toHaveBeenCalledWith(
        'Error: BROWSER_SELECTED_RELAY_UNVERIFIED',
      );
  },
);
it('revokes a late native approval when stopped while the native dialog is pending', async () => {
  let approve!: (value: { sessionId: string; pairingCode: null }) => void;
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === 'p006_start')
      return await new Promise((resolve) => {
        approve = resolve;
      });
    return undefined;
  });
  const states: string[] = [];
  const probe = new RelayProbe(
    document.createElement('video'),
    (s) => states.push(s),
    () => {},
    () => {},
    () => {},
  );
  const starting = probe.start({
    role: 'sender',
    consent: true,
    localIp: '10.0.0.2',
    seconds: 30,
  });
  await probe.stop();
  approve({ sessionId: 'test-session', pairingCode: null });
  await starting;
  expect(invoke).toHaveBeenCalledWith('p006_stop', {
    sessionId: 'test-session',
  });
  expect(invoke).not.toHaveBeenCalledWith('p006_ready', expect.anything());
  expect(states).toContain('stopped');
});
it.each(['P006_STOPPED', 'BROWSER_SELECTED_DIRECT_PATH_UNVERIFIED'])(
  'only treats %s as normal when native cleanup is clean',
  async (code) => {
    const failures: string[] = [];
    const states: string[] = [];
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'p006_start')
        return { sessionId: 'clean-native', pairingCode: null };
      if (command === 'p006_ready') throw new Error(code);
      if (command === 'p006_stats')
        return { resources_released: true, error: null, receiver_samples: [] };
      if (command === 'p006_save') return 'saved';
      return undefined;
    });
    const probe = new RelayProbe(
      document.createElement('video'),
      (s) => states.push(s),
      (e) => failures.push(e),
      () => {},
      () => {},
    );
    await probe.start({
      role: 'sender',
      consent: true,
      localIp: '192.168.1.2',
      seconds: 30,
    });
    expect(states).toContain(code === 'P006_STOPPED' ? 'stopped' : 'failed');
    if (code === 'P006_STOPPED') expect(failures).toEqual([]);
    else expect(failures).toEqual([`Error: ${code}`]);
  },
);
it.each([true, false])(
  'reads final live counters before closing, preserving selected-pair validation: %s',
  async (permitted) => {
    const order: string[] = [];
    const failures: string[] = [];
    const snapshots: Array<number | null | undefined> = [];
    const video = document.createElement('video');
    const report = new Map([
      [
        'inbound',
        {
          type: 'inbound-rtp',
          kind: 'video',
          framesReceived: 300,
          framesDecoded: 300,
        },
      ],
      ['transport', { type: 'transport', selectedCandidatePairId: 'pair' }],
      ['pair', { localCandidateId: 'local', remoteCandidateId: 'remote' }],
      [
        'local',
        { candidateType: 'relay', protocol: 'udp', address: ip, port: 49160 },
      ],
      [
        'remote',
        {
          candidateType: 'relay',
          protocol: 'udp',
          address: ip,
          port: permitted ? 49161 : 49260,
        },
      ],
    ] as Array<[string, Record<string, unknown>]>);
    const pc = {
      connectionState: 'connected',
      getStats: async () => {
        order.push('read');
        return report;
      },
      getReceivers: () => [{ track: { stop: () => order.push('track-stop') } }],
      close: () => {
        order.push('close');
        pc.connectionState = 'closed';
      },
    };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'p006_stats')
        return {
          resources_released: true,
          error: null,
          receiver_samples: [{ framesDecoded: 280 }],
        };
      if (command === 'p006_save') return 'saved';
      return undefined;
    });
    const probe = new RelayProbe(
      video,
      (_state, _status, receiver) => {
        if (receiver) snapshots.push(receiver.framesDecoded);
      },
      (e) => failures.push(e),
      () => {},
      () => {},
    );
    Object.assign(probe, { id: 'final-snapshot', pc, server: ip });
    await probe.stop();
    expect(order.indexOf('read')).toBeLessThan(order.indexOf('track-stop'));
    expect(pc.connectionState).toBe('closed');
    if (permitted) {
      expect(failures).toEqual([]);
      expect(snapshots).toEqual([300]);
    } else {
      expect(failures).toEqual(['Error: BROWSER_SELECTED_RELAY_UNVERIFIED']);
      // Failed-path counters are retained as diagnostic evidence, not success.
      expect(snapshots).toEqual([300]);
    }
  },
);
