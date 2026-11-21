import { expect, it, vi } from 'vitest';
import { relaySdp, selectedRelay, RelayProbe } from './relay';
import { invoke } from '@tauri-apps/api/core';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

const ip = '8.8.8.8'; // Offline fixture. Tests never contact this address.
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
