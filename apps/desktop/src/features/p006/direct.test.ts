import { expect, it } from 'vitest';
import {
  directOptions,
  directSdp,
  directAnswerCandidates,
  selectedDirect,
  type DirectScope,
} from './direct';
import { relayOptions } from './ice';
const session = '00000000-0000-4000-8000-000000000001';
const approval = {
  bothApproved: true,
  sessionId: session,
  expiresAt: 1300,
  serverIp: '8.8.8.8',
  serverPort: 3478,
};
const config = {
  mode: 'direct_preferred' as const,
  session_id: session,
  expires_at: 1300,
  ice_servers: [
    {
      urls: ['turn:8.8.8.8:3478?transport=udp'],
      username: `1300:${session}`,
      credential: 'AAAAAAAAAAAAAAAAAAAAAAAAAAA=',
    },
  ],
};
const scope: DirectScope = {
  serverIp: '8.8.8.8',
  localIp: '192.168.1.2',
  privatePeerIps: ['192.168.1.3'],
  publishPrivateLocal: true,
  expiresAt: Math.floor(Date.now() / 1000) + 180,
};
const candidate = (ip: string, kind = 'host', port = 50000) =>
  `a=candidate:x 1 udp 1 ${ip} ${port} typ ${kind}`;
const answer = () =>
  [
    'v=0',
    'a=group:BUNDLE 0 1',
    `a=fingerprint:sha-256 ${Array(32).fill('AB').join(':')}`,
    'm=video 9 UDP/TLS/RTP/SAVPF 102',
    'a=mid:0',
    'a=sendonly',
    'a=rtcp-mux',
    candidate('192.168.1.3'),
    'm=application 9 UDP/DTLS/SCTP webrtc-datachannel',
    'a=mid:1',
    'a=sctp-port:5000',
    'a=max-message-size:16384',
  ].join('\r\n');
it('enables All only through explicit direct factory and preserves old relay refusal', () => {
  expect(() => relayOptions(config, approval, 1000, true)).toThrow(
    'ICE_MODE_NOT_ENABLED',
  );
  expect(() => directOptions(config, approval, 1000, false)).toThrow();
  const native = directOptions(config, approval, 1000, true);
  expect(native.iceTransportPolicy).toBe('all');
  expect(native.iceServers).toHaveLength(2);
  expect(() =>
    directOptions({ ...config, expires_at: 999 }, approval, 1000, true),
  ).toThrow();
  expect(() =>
    directOptions(config, { ...approval, bothApproved: false }, 1000, true),
  ).toThrow();
});
it('publishes exactly selected private interface only with counterpart approval', () => {
  const sdp = [
    'v=0',
    candidate(scope.localIp),
    candidate('192.168.1.5'),
    candidate('198.18.0.1'),
    candidate('8.8.8.8', 'relay', 49160),
    'a=candidate:m 1 udp 1 hidden.local 50000 typ host',
    candidate(scope.localIp).replace(' udp ', ' tcp '),
  ].join('\r\n');
  expect(directSdp(sdp, scope).candidates.map((c) => c.type)).toEqual([
    'host',
    'relay',
  ]);
  expect(
    directSdp(sdp, { ...scope, publishPrivateLocal: false }).candidates.map(
      (c) => c.type,
    ),
  ).toEqual(['relay']);
  expect(() =>
    directSdp(candidate(scope.localIp), {
      ...scope,
      publishPrivateLocal: false,
    }),
  ).toThrow('DIRECT_LOCAL_CANDIDATE_MISSING');
});
it('strictly checks authenticated answer candidate scope identity and budgets', () => {
  expect(directAnswerCandidates(answer(), scope)).toHaveLength(1);
  for (const bad of [
    answer().replace('192.168.1.3', '192.168.1.4'),
    answer().replace('192.168.1.3', '169.254.169.254'),
    answer().replace('192.168.1.3', '100.64.0.1'),
    answer().replace('udp', 'tcp'),
    answer().replace('typ host', 'typ prflx'),
    answer().replace('AB:AB', 'AB:ZZ'),
    answer() + '\r\nm=audio 9 UDP/TLS/RTP/SAVPF 0',
    answer() + '\r\n' + Array(16).fill(candidate('192.168.1.3')).join('\r\n'),
  ])
    expect(() => directAnswerCandidates(bad, scope)).toThrow();
  expect(() =>
    directAnswerCandidates(answer(), { ...scope, expiresAt: 1 }),
  ).toThrow();
});
it('publishes only R06 approved routes and scrubs related addresses and network metadata', () => {
  const raw = answer()
    .replace('v=0', 'v=0\r\no=private-fixture 11 22 IN IP4 192.168.1.8')
    .replace(
      candidate('192.168.1.3'),
      [
        'c=IN IP4 192.168.1.8',
        'a=rtcp:5000 IN IP4 192.168.1.8',
        `${candidate(scope.localIp)} network-id 5`,
        candidate('192.168.1.8'),
        `${candidate('1.1.1.1', 'srflx').replace('udp', 'UDP')} raddr 192.168.1.8 rport 5000 private-extension fixture`,
        `${candidate('8.8.8.8', 'relay', 49160)} raddr 10.0.0.9 rport 56789 generation 0`,
      ].join('\r\n'),
    );
  const diagnostic = directSdp(raw, scope, true);
  expect(diagnostic.candidates).toEqual([
    { address: scope.localIp, port: 50000, type: 'host' },
    { address: '1.1.1.1', port: 50000, type: 'srflx' },
    { address: '8.8.8.8', port: 49160, type: 'relay' },
  ]);
  expect(diagnostic.sdp).toContain('o=- 11 22 IN IP4 0.0.0.0');
  expect(diagnostic.sdp).toContain('c=IN IP4 0.0.0.0');
  expect(diagnostic.sdp).toContain('a=rtcp:9 IN IP4 0.0.0.0');
  expect(diagnostic.sdp).toContain(`${candidate(scope.localIp)}\r\n`);
  for (const c of [
    candidate('1.1.1.1', 'srflx'),
    candidate('8.8.8.8', 'relay', 49160),
  ])
    expect(diagnostic.sdp).toContain(`${c} raddr 0.0.0.0 rport 0`);
  expect(diagnostic.sdp).not.toMatch(
    /192\.168\.1\.8|10\.0\.0\.9|network-id|generation|fixture/,
  );
  expect(
    directSdp(raw, { ...scope, publishPrivateLocal: false }, true).sdp,
  ).not.toContain(` ${scope.localIp} `);
  expect(directSdp(raw, scope).sdp).toContain('raddr 192.168.1.8');
});

it('requires canonical R06 answers before ICE and rejects ambiguous offer layout', () => {
  const canonical = answer()
    .replace('v=0', 'v=0\r\no=- 1 2 IN IP4 0.0.0.0')
    .replace(
      candidate('192.168.1.3'),
      [
        'c=IN IP4 0.0.0.0',
        'a=rtcp:9 IN IP4 0.0.0.0',
        candidate('192.168.1.3'),
        `${candidate('1.1.1.1', 'srflx')} raddr 0.0.0.0 rport 0`,
      ].join('\r\n'),
    );
  expect(directAnswerCandidates(canonical, scope, true)).toEqual([
    { address: '192.168.1.3', port: 50000, type: 'host' },
    { address: '1.1.1.1', port: 50000, type: 'srflx' },
  ]);
  for (const noncanonical of [
    canonical.replace('o=-', 'o=private'),
    canonical.replace('c=IN IP4 0.0.0.0', 'c=IN IP4 10.0.0.9'),
    canonical.replace('a=rtcp:9', 'a=rtcp:10000'),
    canonical.replace(
      candidate('192.168.1.3'),
      `${candidate('192.168.1.3')} network-id 1`,
    ),
    canonical.replace('raddr 0.0.0.0 rport 0', 'raddr 10.0.0.9 rport 5000'),
    canonical.replace('raddr 0.0.0.0 rport 0', ''),
  ])
    expect(() => directAnswerCandidates(noncanonical, scope, true)).toThrow();
  for (const ambiguous of [
    canonical.replace('a=candidate:', ' a=candidate:'),
    canonical + '\r\na=remote-candidates:1 10.0.0.9 50000',
    canonical
      .replace('a=mid:0', 'a=mid:0\r\na=mid:1')
      .replace(/\r\na=mid:1(?=\r\na=sctp)/, ''),
    canonical.replace('v=0', `v=0\r\n${candidate(scope.localIp)}`),
    canonical.replace('v=0', 'v=0\r\no=- 3 4 IN IP4 0.0.0.0'),
  ]) {
    expect(() => directSdp(ambiguous, scope, true)).toThrow(
      'R06_DIRECT_SDP_INVALID',
    );
    expect(() => directAnswerCandidates(ambiguous, scope, true)).toThrow(
      'R06_DIRECT_SDP_INVALID',
    );
  }
});
it('requires both actual selected endpoints and labels a mixed pair relay', () => {
  const local = directSdp(candidate(scope.localIp), scope).candidates;
  const remote = directAnswerCandidates(answer(), scope);
  const stats = new Map([
    ['t', { type: 'transport', selectedCandidatePairId: 'p' }],
    ['p', { localCandidateId: 'a', remoteCandidateId: 'b' }],
    [
      'a',
      {
        protocol: 'udp',
        candidateType: 'host',
        address: scope.localIp,
        port: 50000,
      },
    ],
    [
      'b',
      {
        protocol: 'udp',
        candidateType: 'host',
        address: '192.168.1.3',
        port: 50000,
      },
    ],
  ]) as unknown as RTCStatsReport;
  expect(
    selectedDirect(new Map() as unknown as RTCStatsReport, local, remote),
  ).toEqual({ verified: null, path: 'unknown' });
  expect(selectedDirect(stats, local, remote)).toEqual({
    verified: true,
    path: 'direct',
  });
  expect(selectedDirect(stats, [], remote)).toEqual({
    verified: false,
    path: 'unknown',
  });
  const mixed = new Map(stats);
  mixed.set('b', {
    protocol: 'udp',
    candidateType: 'relay',
    address: '8.8.8.8',
    port: 49160,
  });
  expect(
    selectedDirect(mixed as unknown as RTCStatsReport, local, [
      { address: '8.8.8.8', port: 49160, type: 'relay' },
    ]),
  ).toEqual({ verified: true, path: 'relay' });
});
