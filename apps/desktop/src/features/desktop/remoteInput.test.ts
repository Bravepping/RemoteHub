import { expect, it, vi } from 'vitest';
import { ControlClient } from '../p004/control';
function fixture() {
  const dc = {
    readyState: 'open',
    bufferedAmount: 0,
    send: vi.fn(),
    close: vi.fn(),
  } as unknown as RTCDataChannel;
  const pc = {
    createDataChannel: vi.fn(() => dc),
  } as unknown as RTCPeerConnection;
  const client = new ControlClient(pc, 'fixture-session', vi.fn(), true);
  return { dc, pc, client };
}
it('manual input never sends a simulated batch and needs playing/path readiness plus an explicit enable', () => {
  const { client, dc, pc } = fixture();
  expect(pc.createDataChannel).toHaveBeenCalledWith('remote-input.v1', {
    ordered: true,
    protocol: 'remote-input.v1',
  });
  expect(client.enableInput(true)).toBe(false);
  expect(client.input({ kind: 'release_all' })).toBe(false);
  client.beginBatch(true);
  expect(dc.send).not.toHaveBeenCalled();
  expect(client.input({ kind: 'release_all' })).toBe(false);
  expect(client.enableInput(true)).toBe(true);
  expect(client.input({ kind: 'mouse_move', x_norm: 0.5, y_norm: 0.2 })).toBe(
    true,
  );
  expect(typeof vi.mocked(dc.send).mock.calls[0][0]).toBe('string');
  const packet = JSON.parse(String(vi.mocked(dc.send).mock.calls[0][0]));
  expect(packet.event.kind).toBe('mouse_move');
  expect(packet.kind).toBeUndefined();
  expect(packet.seq).toBe(1);
  client.beginBatch(false);
  expect(
    JSON.parse(String(vi.mocked(dc.send).mock.calls[1][0])).event.kind,
  ).toBe('release_all');
  expect(client.input({ kind: 'release_all' })).toBe(false);
});
it('revoked channel cannot send; fixed acknowledgements refer to real manual sequence', () => {
  const { client, dc } = fixture();
  client.beginBatch(true);
  client.enableInput(true);
  client.input({ kind: 'release_all' });
  dc.onmessage?.({
    data: JSON.stringify({
      v: 1,
      kind: 'ack',
      seq: 1,
      session_id: 'fixture-session',
      ack_seq: 1,
    }),
  } as MessageEvent);
  expect(client.snapshot().ackReceived).toBe(1);
  client.revoke();
  expect(client.input({ kind: 'release_all' })).toBe(false);
  expect(dc.send).toHaveBeenCalledTimes(1);
});
