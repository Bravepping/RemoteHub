import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { VideoDisplayFailure } from './canvasVideoAdapter';
import {
  parsePublicVideoFrame,
  PublicVideoSession,
  type PublicVideoBridge,
} from './publicVideoSession';

const first = '0'.repeat(32);
const second = '1'.repeat(32);

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((fulfill, fail) => {
    resolve = fulfill;
    reject = fail;
  });
  return { promise, resolve, reject };
}

function raw(sequence = 0, timestampUs = 0, key = true): ArrayBuffer {
  const payload = [0, 0, 1, 0x65, 3];
  const frame = new ArrayBuffer(24 + payload.length);
  const header = new DataView(frame);
  header.setBigUint64(0, BigInt(sequence), true);
  header.setBigUint64(8, BigInt(timestampUs), true);
  header.setUint32(16, payload.length, true);
  header.setUint8(20, Number(key));
  new Uint8Array(frame, 24).set(payload);
  return frame;
}

describe('native raw video frame', () => {
  it('parses the little-endian layout and treats only an empty buffer as pending', () => {
    expect(parsePublicVideoFrame(new ArrayBuffer(0))).toBeNull();
    expect(parsePublicVideoFrame(raw(260, 4000, false))).toEqual({
      sequence: 260,
      timestampUs: 4000,
      keyframe: false,
      annexB: new Uint8Array([0, 0, 1, 0x65, 3]),
    });
  });

  it.each([
    ['truncated', () => new ArrayBuffer(23)],
    ['oversized', () => new ArrayBuffer(24 + 1024 * 1024 + 1)],
    ['JSON bytes', () => [...new Uint8Array(raw())]],
    [
      'reserved flag',
      () => {
        const frame = raw();
        new DataView(frame).setUint8(20, 3);
        return frame;
      },
    ],
    [
      'nonzero padding',
      () => {
        const frame = raw();
        new DataView(frame).setUint8(23, 1);
        return frame;
      },
    ],
    [
      'payload length mismatch',
      () => {
        const frame = raw();
        new DataView(frame).setUint32(16, 15, true);
        return frame;
      },
    ],
    [
      'unsafe timestamp',
      () => {
        const frame = raw();
        new DataView(frame).setBigUint64(8, 1n << 53n, true);
        return frame;
      },
    ],
  ] as const)('rejects %s', (_name, mutate) => {
    expect(() => parsePublicVideoFrame(mutate())).toThrow();
  });
});

describe('public video viewer controller', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  function harness() {
    const bridge = {
      start: vi.fn<PublicVideoBridge['start']>().mockResolvedValue(first),
      status: vi
        .fn<PublicVideoBridge['status']>()
        .mockResolvedValue({ phase: 'ready' }),
      nextFrame: vi
        .fn<PublicVideoBridge['nextFrame']>()
        .mockResolvedValue(new ArrayBuffer(0)),
      stop: vi.fn<PublicVideoBridge['stop']>().mockResolvedValue(undefined),
    };
    const ingress = vi.fn(() => true);
    const renderer = { start: vi.fn(() => ingress), stop: vi.fn() };
    let draw!: () => void;
    const create = vi.fn(
      (_failed: (code: VideoDisplayFailure) => void, presented: () => void) => {
        draw = presented;
        return renderer;
      },
    );
    const update = vi.fn();
    const session = new PublicVideoSession(bridge, create, update);
    return {
      bridge,
      renderer,
      create,
      ingress,
      update,
      session,
      draw: () => draw(),
    };
  }

  it('waits for native ready, has one frame pull, and reports watching only after a paint', async () => {
    const { bridge, create, ingress, update, session, draw } = harness();
    const frame = deferred<unknown>();
    bridge.status.mockResolvedValueOnce({ phase: 'starting' });
    bridge.nextFrame.mockReturnValueOnce(frame.promise);
    session.start('viewer');
    await vi.advanceTimersByTimeAsync(0);
    expect(create).not.toHaveBeenCalled();
    expect(bridge.nextFrame).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(700);
    expect(bridge.nextFrame).toHaveBeenCalledExactlyOnceWith(first);
    expect(update).toHaveBeenLastCalledWith('waiting');
    frame.resolve(raw());
    await vi.advanceTimersByTimeAsync(0);
    expect(ingress).toHaveBeenCalledTimes(1);
    expect(update).toHaveBeenLastCalledWith('waiting');
    draw();
    expect(update).toHaveBeenLastCalledWith('watching');
    await session.stop();
  });

  it('does not create a renderer or pull media when sharing', async () => {
    const { bridge, create, session, update } = harness();
    session.start('host');
    await vi.advanceTimersByTimeAsync(1000);
    expect(bridge.start).toHaveBeenCalledExactlyOnceWith('host');
    expect(bridge.status).toHaveBeenCalledWith(first);
    expect(create).not.toHaveBeenCalled();
    expect(bridge.nextFrame).not.toHaveBeenCalled();
    expect(update).toHaveBeenLastCalledWith('sharing');
    await session.stop();
  });

  it('clears synchronously and suppresses stale output across stop and restart', async () => {
    const { bridge, renderer, ingress, session, update, draw } = harness();
    const oldFrame = deferred<unknown>();
    const nativeStop = deferred<unknown>();
    bridge.nextFrame.mockReturnValueOnce(oldFrame.promise);
    bridge.stop.mockReturnValueOnce(nativeStop.promise);
    bridge.start.mockResolvedValueOnce(first).mockResolvedValueOnce(second);
    session.start('viewer');
    await vi.advanceTimersByTimeAsync(0);
    const stopping = session.stop();
    expect(renderer.stop).toHaveBeenCalledTimes(1);
    expect(update).toHaveBeenLastCalledWith('stopping');
    draw();
    session.start('viewer');
    expect(bridge.start).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(0);
    expect(bridge.stop).toHaveBeenCalledExactlyOnceWith(first);
    nativeStop.resolve(undefined);
    await stopping;
    session.start('viewer');
    await vi.advanceTimersByTimeAsync(0);
    expect(bridge.start).toHaveBeenCalledTimes(2);
    expect(bridge.nextFrame).toHaveBeenCalledExactlyOnceWith(first);
    oldFrame.resolve(raw());
    await vi.advanceTimersByTimeAsync(0);
    expect(ingress).not.toHaveBeenCalled();
    expect(bridge.nextFrame).toHaveBeenLastCalledWith(second);
    expect(update.mock.calls.map(([state]) => state)).not.toContain('watching');
    await session.stop();
  });

  it('revokes any generation returned after the pending start was stopped', async () => {
    const { bridge, session, update } = harness();
    const pending = deferred<unknown>();
    bridge.start.mockReturnValueOnce(pending.promise);
    session.start('viewer');
    await vi.advanceTimersByTimeAsync(0);
    const stopping = session.stop();
    expect(update).toHaveBeenLastCalledWith('stopping');
    pending.resolve(first);
    await stopping;
    expect(bridge.stop).toHaveBeenCalledExactlyOnceWith(first);
    expect(bridge.status).toHaveBeenCalledExactlyOnceWith(first);
    expect(bridge.stop).toHaveBeenCalledBefore(bridge.status);
    expect(bridge.nextFrame).not.toHaveBeenCalled();
    expect(update).toHaveBeenLastCalledWith('stopped', undefined, {
      reason: 'local_stop',
    });
  });

  it.each(['capture_limit', 'capture_ended'] as const)(
    'reports the native host ending %s and measured capture time',
    async (reason) => {
      const { bridge, session, update } = harness();
      bridge.status
        .mockResolvedValueOnce({ phase: 'ready' })
        .mockResolvedValueOnce({
          phase: 'closed',
          error: null,
          end_reason: reason,
          elapsed_ms: 12_345,
        });
      session.start('host');
      await vi.advanceTimersByTimeAsync(400);
      expect(update).toHaveBeenLastCalledWith('stopped', undefined, {
        reason,
        captureMs: 12_345,
      });
      expect(bridge.status).toHaveBeenCalledTimes(2);
      expect(bridge.stop).toHaveBeenCalledExactlyOnceWith(first);
    },
  );

  it('reports peer departure and only local observed playback on the viewer', async () => {
    const { bridge, renderer, session, update, draw } = harness();
    bridge.status
      .mockResolvedValueOnce({ phase: 'ready' })
      .mockResolvedValueOnce({
        phase: 'stopping',
        end_reason: 'peer_left',
        elapsed_ms: 98_765,
      });
    session.start('viewer');
    await vi.advanceTimersByTimeAsync(50);
    draw();
    await vi.advanceTimersByTimeAsync(350);
    expect(renderer.stop).toHaveBeenCalledBefore(bridge.stop);
    expect(update).toHaveBeenLastCalledWith(
      'stopped',
      undefined,
      expect.objectContaining({ reason: 'peer_left', playbackMs: 313 }),
    );
    expect(update.mock.lastCall?.[2].captureMs).toBeUndefined();
  });

  it('excludes pre-paint and native cleanup waits from viewer playback, without extra polling on Stop', async () => {
    const { bridge, renderer, session, update, draw } = harness();
    const nativeStop = deferred<unknown>();
    bridge.stop.mockReturnValueOnce(nativeStop.promise);
    session.start('viewer');
    await vi.advanceTimersByTimeAsync(100);
    draw();
    await vi.advanceTimersByTimeAsync(125);
    draw(); // Later paints must not replace the first.
    const stopping = session.stop();
    expect(renderer.stop).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1000);
    expect(update).toHaveBeenLastCalledWith('stopping');
    expect(bridge.status).toHaveBeenCalledExactlyOnceWith(first);
    nativeStop.resolve(undefined);
    await stopping;
    expect(update).toHaveBeenLastCalledWith('stopped', undefined, {
      reason: 'local_stop',
      playbackMs: 125,
    });
  });

  it.each([
    { end_reason: null, elapsed_ms: null },
    { end_reason: 'fixture secret', elapsed_ms: 'fixture secret' },
    { end_reason: 'capture_limit', elapsed_ms: 2000 },
  ])(
    'does not imply viewer playback or a remote capture reason without local evidence',
    async (end) => {
      const { bridge, session, update } = harness();
      bridge.status.mockResolvedValueOnce({ phase: 'closed', ...end });
      session.start('viewer');
      await vi.advanceTimersByTimeAsync(0);
      expect(update).toHaveBeenLastCalledWith('stopped', undefined);
      expect(JSON.stringify(update.mock.calls)).not.toContain('fixture secret');
    },
  );

  it('reports a native failed ending with a redacted error and ignores an invalid native duration', async () => {
    const { bridge, session, update } = harness();
    bridge.status.mockResolvedValueOnce({
      phase: 'closed',
      error: 'secret fixture',
      end_reason: 'failed',
      elapsed_ms: -2,
    });
    session.start('host');
    await vi.advanceTimersByTimeAsync(0);
    expect(update).toHaveBeenLastCalledWith(
      'failed',
      'PUBLIC_VIDEO_STATUS_IPC_FAILED',
      expect.objectContaining({ reason: 'failed' }),
    );
    expect(update.mock.lastCall?.[2].captureMs).toBeUndefined();
    expect(JSON.stringify(update.mock.calls)).not.toContain('secret fixture');
  });

  it('does not call native start if stopped before dispatch', async () => {
    const { bridge, session } = harness();
    session.start('viewer');
    await session.stop();
    expect(bridge.start).not.toHaveBeenCalled();
  });

  it('clears on terminal native status, without exposing raw failure', async () => {
    const { bridge, renderer, session, update } = harness();
    bridge.status
      .mockResolvedValueOnce({ phase: 'ready' })
      .mockResolvedValueOnce({ phase: 'closed', error: 'secret fixture' });
    session.start('viewer');
    await vi.advanceTimersByTimeAsync(500);
    expect(renderer.stop).toHaveBeenCalledTimes(1);
    expect(bridge.stop).toHaveBeenCalledExactlyOnceWith(first);
    expect(update).toHaveBeenLastCalledWith(
      'failed',
      'PUBLIC_VIDEO_STATUS_IPC_FAILED',
    );
    expect(JSON.stringify(update.mock.calls)).not.toContain('secret fixture');
  });

  it('clears and stops before reporting a fixed native terminal diagnostic', async () => {
    const { bridge, renderer, session, update } = harness();
    bridge.status
      .mockResolvedValueOnce({ phase: 'ready' })
      .mockResolvedValueOnce({
        phase: 'closed',
        error: 'PUBLIC_VIDEO_ICE_PAIR_UNAVAILABLE',
      });
    session.start('viewer');
    await vi.advanceTimersByTimeAsync(500);
    expect(renderer.stop).toHaveBeenCalledTimes(1);
    expect(bridge.stop).toHaveBeenCalledExactlyOnceWith(first);
    expect(update).toHaveBeenLastCalledWith(
      'failed',
      'PUBLIC_VIDEO_ICE_PAIR_UNAVAILABLE',
    );
  });

  it.each([
    'PUBLIC_VIDEO_H264_BOOTSTRAP',
    'PUBLIC_VIDEO_H264_CLOSED',
    'PUBLIC_VIDEO_H264_LIMIT',
    'PUBLIC_VIDEO_H264_MALFORMED',
    'PUBLIC_VIDEO_ICE_BOUNDARY_CLOSED',
    'PUBLIC_VIDEO_ICE_BOUNDARY_PEER',
    'PUBLIC_VIDEO_ICE_BOUNDARY_SETUP',
    'PUBLIC_VIDEO_ICE_CONFIG_MISMATCH',
    'PUBLIC_VIDEO_ICE_DRIVER',
    'PUBLIC_VIDEO_ICE_DRIVER_TIMEOUT',
    'PUBLIC_VIDEO_ICE_GRANT_MISMATCH',
    'PUBLIC_VIDEO_MEDIA_CLOSED',
    'PUBLIC_VIDEO_RTP_GAP',
    'PUBLIC_VIDEO_RTP_INVALID',
    'PUBLIC_VIDEO_RTP_SOURCE',
    'PUBLIC_VIDEO_RTP_TIMESTAMP',
  ])(
    'reports the fixed frame-IPC diagnostic %s only after clearing and stopping',
    async (code) => {
      const { bridge, renderer, ingress, update, session } = harness();
      const nativeStop = deferred<unknown>();
      bridge.nextFrame.mockRejectedValueOnce(code);
      bridge.stop.mockReturnValueOnce(nativeStop.promise);
      session.start('viewer');
      await vi.advanceTimersByTimeAsync(0);
      expect(ingress).not.toHaveBeenCalled();
      expect(renderer.stop).toHaveBeenCalledTimes(1);
      expect(renderer.stop).toHaveBeenCalledBefore(bridge.stop);
      expect(bridge.stop).toHaveBeenCalledExactlyOnceWith(first);
      expect(update).toHaveBeenLastCalledWith('stopping');
      nativeStop.resolve(undefined);
      await vi.advanceTimersByTimeAsync(0);
      expect(update).toHaveBeenLastCalledWith('failed', code);
    },
  );

  it.each([
    ['status', 'PUBLIC_VIDEO_STATUS_IPC_FAILED'],
    ['nextFrame', 'PUBLIC_VIDEO_FRAME_IPC_FAILED'],
  ] as const)(
    'scopes and redacts unknown %s IPC failures',
    async (method, code) => {
      const { bridge, ingress, update, session } = harness();
      bridge[method].mockRejectedValueOnce(
        'PUBLIC_VIDEO_H264_MALFORMED leaked-fixture',
      );
      session.start('viewer');
      await vi.advanceTimersByTimeAsync(0);
      expect(ingress).not.toHaveBeenCalled();
      expect(bridge.stop).toHaveBeenCalledExactlyOnceWith(first);
      expect(update).toHaveBeenLastCalledWith('failed', code);
      expect(JSON.stringify(update.mock.calls)).not.toContain('leaked-fixture');
    },
  );

  it.each([
    ['PUBLIC_VIDEO_BUSY', 'PUBLIC_VIDEO_BUSY'],
    ['PUBLIC_VIDEO_BUSY token=secret-fixture', 'PUBLIC_VIDEO_OP_FAILED'],
    [new Error('PUBLIC_VIDEO_BUSY'), 'PUBLIC_VIDEO_OP_FAILED'],
  ])(
    'allows only exact native IPC rejection codes (%s)',
    async (cause, expected) => {
      const { bridge, session, update } = harness();
      bridge.start.mockRejectedValueOnce(cause);
      session.start('host');
      await vi.advanceTimersByTimeAsync(0);
      expect(update).toHaveBeenLastCalledWith('failed', expected);
      expect(JSON.stringify(update.mock.calls)).not.toContain('secret-fixture');
    },
  );

  it('fails closed on raw buffer mismatch and never submits a malformed frame', async () => {
    const { bridge, renderer, ingress, session, update } = harness();
    bridge.nextFrame.mockResolvedValueOnce(new ArrayBuffer(3));
    session.start('viewer');
    await vi.advanceTimersByTimeAsync(0);
    expect(ingress).not.toHaveBeenCalled();
    expect(renderer.stop).toHaveBeenCalledTimes(1);
    expect(bridge.stop).toHaveBeenCalledExactlyOnceWith(first);
    expect(update).toHaveBeenLastCalledWith(
      'failed',
      'PUBLIC_VIDEO_RESPONSE_INVALID',
    );
  });

  it('separates an unexpected renderer rejection from frame IPC failure', async () => {
    const { bridge, renderer, ingress, session, update } = harness();
    bridge.nextFrame.mockResolvedValueOnce(raw());
    ingress.mockReturnValueOnce(false);
    session.start('viewer');
    await vi.advanceTimersByTimeAsync(0);
    expect(renderer.stop).toHaveBeenCalledTimes(1);
    expect(bridge.stop).toHaveBeenCalledExactlyOnceWith(first);
    expect(update).toHaveBeenLastCalledWith(
      'failed',
      'PUBLIC_VIDEO_DISPLAY_FAILED',
    );
  });

  it('keeps cleanup locked on timeout and closes the late native ID without displaying', async () => {
    const { bridge, session, update } = harness();
    const pending = deferred<unknown>();
    bridge.start.mockReturnValueOnce(pending.promise);
    session.start('viewer');
    await vi.advanceTimersByTimeAsync(7100);
    expect(update).toHaveBeenLastCalledWith(
      'cleanup_unconfirmed',
      'PUBLIC_VIDEO_IPC_TIMEOUT',
    );
    session.start('viewer');
    pending.resolve(first);
    await vi.advanceTimersByTimeAsync(0);
    expect(bridge.start).toHaveBeenCalledTimes(1);
    expect(bridge.stop).toHaveBeenCalledExactlyOnceWith(first);
    expect(bridge.nextFrame).not.toHaveBeenCalled();
  });
});
