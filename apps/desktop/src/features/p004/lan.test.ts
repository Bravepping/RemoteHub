import { afterEach, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { LanProbe, lanOffer, selectedPeer } from './lan';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
afterEach(() => {
  vi.resetAllMocks();
  vi.unstubAllGlobals();
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
