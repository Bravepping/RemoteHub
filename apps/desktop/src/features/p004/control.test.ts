import { afterEach, expect, it, vi } from 'vitest';
import { ControlClient } from './control';
const id = '00000000-0000-4000-8000-000000000001';
function fixture() {
  const dc = {
    readyState: 'open',
    bufferedAmount: 0,
    send: vi.fn(),
    close: vi.fn(),
    onopen: null,
    onmessage: null,
    onerror: null,
    onclose: null,
  } as unknown as RTCDataChannel;
  const pc = {
    createDataChannel: vi.fn(() => dc),
  } as unknown as RTCPeerConnection;
  const failed = vi.fn();
  const client = new ControlClient(pc, id, failed);
  return { client, dc, pc, failed };
}
afterEach(() => vi.useRealTimers());
it('creates one reliable ordered channel on the supplied video PC, then counts real event callbacks', async () => {
  vi.useFakeTimers();
  const { client, dc, pc, failed } = fixture();
  expect(pc.createDataChannel).toHaveBeenCalledWith('control.v1', {
    ordered: true,
    protocol: 'control.v1',
  });
  dc.onopen?.(new Event('open'));
  client.beginBatch(false);
  expect(dc.send).not.toHaveBeenCalled();
  client.beginBatch(true);
  await vi.advanceTimersByTimeAsync(5050);
  expect(dc.send).toHaveBeenCalledTimes(101);
  vi.mocked(dc.send).mock.calls.forEach(([text], index) => {
    const request = JSON.parse(String(text));
    expect(request.seq).toBe(index + 1);
    dc.onmessage?.(
      new MessageEvent('message', {
        data: JSON.stringify({
          v: 1,
          kind: request.kind === 'ping' ? 'pong' : 'ack',
          seq: index + 1,
          session_id: id,
          ...(request.kind === 'ping'
            ? { reply_to: request.seq }
            : { ack_seq: request.seq }),
        }),
      }),
    );
  });
  expect(client.snapshot()).toMatchObject({
    sentMoves: 100,
    ackReceived: 100,
    pingsSent: 1,
    pongsReceived: 1,
    pending: 0,
    lastTxSeq: 101,
    lastRxSeq: 101,
    openEvents: 1,
    messageEvents: 101,
    anomalies: 0,
  });
  Object.defineProperty(dc, 'readyState', { value: 'closed' });
  dc.onclose?.(new Event('close'));
  await client.close();
  client.beginBatch(true);
  await vi.advanceTimersByTimeAsync(1000);
  expect(dc.send).toHaveBeenCalledTimes(101);
  expect(client.snapshot()).toMatchObject({ closeEvents: 1, sendClosed: true });
  expect(failed).not.toHaveBeenCalled();
});
it('rejects replay and cross-session replies and handles error without continuing to send', async () => {
  vi.useFakeTimers();
  const { client, dc, failed } = fixture();
  dc.onopen?.(new Event('open'));
  client.beginBatch(true);
  dc.onmessage?.(
    new MessageEvent('message', {
      data: JSON.stringify({
        v: 1,
        kind: 'pong',
        seq: 1,
        session_id: 'other',
        reply_to: 1,
      }),
    }),
  );
  await vi.advanceTimersByTimeAsync(1000);
  expect(failed).toHaveBeenCalledWith('CONTROL_RESPONSE_INVALID');
  expect(dc.send).toHaveBeenCalledTimes(1);
  const second = fixture();
  second.dc.onerror?.(new Event('error') as RTCErrorEvent);
  expect(second.failed).toHaveBeenCalledWith('CONTROL_CHANNEL_ERROR');
  expect(second.client.snapshot().errorEvents).toBe(1);
});
it('stopping before open prevents a late grant from sending events', async () => {
  const { client, dc } = fixture();
  Object.defineProperty(dc, 'readyState', { value: 'closed' });
  await client.close();
  dc.onopen?.(new Event('open'));
  client.beginBatch(true);
  expect(dc.send).not.toHaveBeenCalled();
});
