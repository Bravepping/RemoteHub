// Offline lifecycle fixtures. Real local transport evidence is collected separately.
import { expect, it, vi } from 'vitest';
import { RelayProbe } from './relay';
import {
  evidenceBounded,
  evidenceCode,
  type ReceiverEvidence,
} from './evidence';
import type { LanReceiver } from '../p004/lan';
function fixture(
  options: {
    readFails?: boolean;
    saveFails?: boolean;
    nativeFails?: boolean;
    unverified?: boolean;
    known?: boolean;
  } = {},
) {
  const order: string[] = [];
  const records: ReceiverEvidence[] = [];
  const errors: string[] = [];
  const states: string[] = [];
  const stats = new Map<string, Record<string, unknown>>([
    [
      'inbound',
      {
        type: 'inbound-rtp',
        kind: 'video',
        framesDecoded: 300,
        framesReceived: 300,
      },
    ],
    ['transport', { type: 'transport', selectedCandidatePairId: 'pair' }],
    ['pair', { localCandidateId: 'local', remoteCandidateId: 'remote' }],
    [
      'local',
      {
        candidateType: 'relay',
        protocol: 'udp',
        address: '8.8.8.8',
        port: 49160,
      },
    ],
    [
      'remote',
      {
        candidateType: 'relay',
        protocol: 'udp',
        address: '8.8.8.8',
        port: options.unverified ? 49260 : 49161,
      },
    ],
  ]);
  const pc = {
    connectionState: 'connected',
    getStats: async () => {
      order.push('read');
      if (options.readFails) throw new Error('PRIVATE C:/file 192.168.1.2');
      return stats;
    },
    getReceivers: () => [{ track: { stop: () => order.push('track-stop') } }],
    close: () => {
      order.push('pc-close');
      pc.connectionState = 'closed';
    },
  };
  const call = vi.fn(
    async (command: string, args?: Record<string, unknown>) => {
      order.push(command);
      if (command === 'p006_stop' && options.nativeFails)
        throw new Error('P006_CLEANUP_PENDING');
      if (command === 'p006_stats')
        return {
          resources_released: !options.nativeFails,
          error: options.nativeFails ? 'P006_CLEANUP_PENDING' : null,
          receiver_samples: [{ framesDecoded: 280 }],
        };
      const e = args?.receiverEvidence as ReceiverEvidence | undefined;
      if (e) {
        if (options.saveFails) throw new Error('REPORT_WRITE');
        records.push(structuredClone(e));
        order.push(e.stage);
      }
      return 'saved';
    },
  );
  const probe = new RelayProbe(
    document.createElement('video'),
    (s) => states.push(s),
    (e) => errors.push(e),
    () => {},
    () => {},
    false,
    call as never,
  );
  Object.assign(probe, {
    id: 'native-owner',
    pc,
    server: '8.8.8.8',
    receiverRole: true,
  });
  if (options.known)
    Object.assign(probe, {
      lastReceiver: {
        at: 1000,
        sample: { framesDecoded: 91, peerMatches: true } as LanReceiver,
      },
    });
  return { probe, pc, records, errors, states, order, call };
}
it('persists independent actual counters before close and cleanup after close, only once', async () => {
  const f = fixture();
  await Promise.all([f.probe.stop(), f.probe.stop()]);
  expect(f.records.map((e) => e.stage)).toEqual(['pre_close', 'cleanup']);
  expect(f.order.indexOf('p006_stop')).toBeLessThan(
    f.order.indexOf('pre_close'),
  );
  expect(f.order.indexOf('pre_close')).toBeLessThan(
    f.order.indexOf('track-stop'),
  );
  expect(f.order.indexOf('pc-close')).toBeLessThan(f.order.indexOf('cleanup'));
  expect(f.records[0].snapshot?.framesDecoded).toBe(300);
  expect(f.records[0].cleanup.peerClosed).toBe(false);
  expect(f.records[1].cleanup).toMatchObject({
    peerClosed: true,
    tracksStopped: true,
    nativeResourcesReleased: true,
  });
  expect(f.states.at(-1)).toBe('stopped');
});
it.each([true, false])(
  'getStats failure preserves last-known/unavailable truth and still closes: %s',
  async (known) => {
    const f = fixture({ readFails: true, known });
    await f.probe.stop();
    expect(f.pc.connectionState).toBe('closed');
    expect(f.order).toContain('p006_stop');
    expect(f.records).toHaveLength(2);
    expect(f.records[0].capture).toBe(known ? 'last_known' : 'unavailable');
    expect(f.records[0].sampledAtMs).toBe(known ? 1000 : null);
    expect(f.records[0].snapshot?.framesDecoded ?? null).toBe(
      known ? 91 : null,
    );
    expect(f.records[1].errors).toContain('RECEIVER_INTERNAL_ERROR');
    expect(JSON.stringify(f.records)).not.toContain('192.168.1.2');
  },
);
it('disk/IPC failure never skips native stop, track stop or PC close', async () => {
  const f = fixture({ saveFails: true });
  await f.probe.stop();
  expect(f.errors).toContain('REPORT_WRITE');
  expect(f.pc.connectionState).toBe('closed');
  expect(f.order).toContain('track-stop');
  expect(f.order).toContain('p006_stop');
  expect(f.states.at(-1)).toBe('failed');
});
it('native cleanup failure is retained without pretending resources released', async () => {
  const f = fixture({ nativeFails: true });
  await f.probe.stop();
  expect(f.records[1].cleanup.nativeResourcesReleased).toBe(false);
  expect(f.records[1].errors).toContain('P006_CLEANUP_PENDING');
  expect(f.pc.connectionState).toBe('closed');
});
it('unverified endpoint is recorded as failed evidence, never normal success', async () => {
  const f = fixture({ unverified: true });
  await f.probe.stop();
  expect(f.records[0].snapshot?.peerMatches).toBe(false);
  expect(f.states.at(-1)).toBe('failed');
  expect(f.records[0].errors).toContain('BROWSER_SELECTED_RELAY_UNVERIFIED');
});
it('bounded collection times out and raw errors become fixed safe codes', async () => {
  expect(evidenceCode('Error: BROWSER_PC_MISSING')).toBe('BROWSER_PC_MISSING');
  expect(evidenceCode('token=secret or 192.168.1.2')).toBe(
    'RECEIVER_INTERNAL_ERROR',
  );
  await expect(evidenceBounded(new Promise(() => {}), 5)).rejects.toThrow(
    'RECEIVER_EVIDENCE_TIMEOUT',
  );
});
