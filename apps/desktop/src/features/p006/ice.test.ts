import { expect, it } from 'vitest';
import {
  relayOptions,
  relayPairSummary,
  type IceConfig,
  type RelayApproval,
} from './ice';
const id = '00000000-0000-4000-8000-000000000001';
function fixture(): [IceConfig, RelayApproval] {
  return [
    {
      mode: 'relay_only',
      session_id: id,
      expires_at: 1300,
      ice_servers: [
        {
          urls: ['turn:8.8.8.8:3478?transport=udp'],
          username: `1300:${id}`,
          credential: 'AAAAAAAAAAAAAAAAAAAAAAAAAAA=',
        },
      ],
    },
    {
      bothApproved: true,
      sessionId: id,
      expiresAt: 1600,
      serverIp: '8.8.8.8',
      serverPort: 3478,
    },
  ];
}
it('uses the actual browser relay policy without opening any connection', () => {
  const [c, a] = fixture();
  const options = relayOptions(c, a, 1000, true);
  expect(options.iceTransportPolicy).toBe('relay');
  expect(options.iceServers).toHaveLength(1);
  expect(relayPairSummary(new Map() as unknown as RTCStatsReport)).toBeNull();
});
it('rejects disabled approval, expiry, public endpoint substitution, TCP TLS and unsupported mode', () => {
  const [c, a] = fixture();
  expect(() => relayOptions(c, a, 1000, false)).toThrow();
  expect(() =>
    relayOptions(c, { ...a, bothApproved: false }, 1000, true),
  ).toThrow();
  expect(() => relayOptions(c, a, 1300, true)).toThrow();
  expect(() => relayOptions(c, { ...a, expiresAt: NaN }, 1000, true)).toThrow();
  for (const url of [
    'turns:8.8.8.8:5349',
    'turn:8.8.8.8:3478?transport=tcp',
    'turn:8.8.4.4:3478?transport=udp',
  ]) {
    const [other, approval] = fixture();
    other.ice_servers[0].urls = [url];
    expect(() => relayOptions(other, approval, 1000, true)).toThrow();
  }
  expect(() =>
    relayOptions({ ...c, mode: 'direct_preferred' }, a, 1000, true),
  ).toThrow();
});
