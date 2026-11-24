import { expect, it, vi } from 'vitest';
import {
  BASELINE_AVC_CODEC,
  CanvasVideoAdapter,
  MAX_PENDING_VIDEO_FRAMES,
  type NativeVideoFrame,
} from './canvasVideoAdapter';

const nal = (type: number, data: number[]) => [0, 0, 0, 1, type, ...data];
const keyAu = () =>
  Uint8Array.from([
    ...nal(0x67, [0x42, 0xe0, 0x1f, 0x80]),
    ...nal(0x68, [0x80]),
    ...nal(0x65, [0x80]),
  ]);
const deltaAu = () => Uint8Array.from([0, 0, 1, 0x41, 0x80]);
const au = (sequence = 0, keyframe = sequence === 0): NativeVideoFrame => ({
  sequence,
  timestampUs: sequence * 33_333,
  keyframe,
  annexB: keyframe ? keyAu() : deltaAu(),
});
const decoded = (timestamp: number, displayWidth = 1280, displayHeight = 720) =>
  ({
    timestamp,
    displayWidth,
    displayHeight,
    close: vi.fn(),
  }) as unknown as VideoFrame;

function fixture() {
  let visible = false;
  let width = 320;
  let height = 180;
  const drawImage = vi.fn(() => {
    visible = true;
  });
  const clear = vi.fn(() => {
    visible = false;
  });
  const canvas = {
    get width() {
      return width;
    },
    set width(value) {
      width = value;
      clear();
    },
    get height() {
      return height;
    },
    set height(value) {
      height = value;
      clear();
    },
    getContext: vi.fn(() => ({ drawImage })),
  } as unknown as HTMLCanvasElement;
  const decoders: ReturnType<typeof createDecoder>[] = [];
  function createDecoder(init: VideoDecoderInit) {
    return {
      init,
      decodeQueueSize: 0,
      configure: vi.fn(),
      decode: vi.fn(),
      close: vi.fn(),
    };
  }
  const factory = vi.fn((init: VideoDecoderInit) => {
    const decoder = createDecoder(init);
    decoders.push(decoder);
    return decoder;
  });
  const chunks = vi.fn(
    (init: EncodedVideoChunkInit) => init as unknown as EncodedVideoChunk,
  );
  const failed = vi.fn();
  const adapter = new CanvasVideoAdapter(canvas, factory, failed, chunks);
  return {
    adapter,
    canvas,
    decoders,
    factory,
    chunks,
    failed,
    clear,
    drawImage,
    visible: () => visible,
  };
}

it('configures baseline Annex B, snapshots bytes and draws only real decoder outputs', () => {
  const f = fixture();
  const ingest = f.adapter.start()!;
  expect(ingest).toBeTypeOf('function');
  expect(f.decoders[0].configure).toHaveBeenCalledWith({
    codec: BASELINE_AVC_CODEC,
    optimizeForLatency: true,
  });
  const frame = au();
  expect(ingest(frame)).toBe(true);
  expect(f.visible()).toBe(false);
  frame.annexB.fill(0);
  expect(f.chunks.mock.lastCall?.[0]).toMatchObject({
    type: 'key',
    timestamp: 0,
    data: keyAu(),
  });

  const output = decoded(0);
  f.decoders[0].init.output(output);
  expect(f.drawImage).toHaveBeenCalledWith(output, 0, 0, 1280, 720);
  expect(output.close).toHaveBeenCalledTimes(1);
  expect(f.visible()).toBe(true);
  expect(f.canvas.width).toBe(1280);

  f.adapter.stop();
  expect(f.visible()).toBe(false);
  expect(f.decoders[0].close).toHaveBeenCalledTimes(1);
  expect(ingest(au(1))).toBe(false);
});

it('ignores stale frames and errors after stop/restart and closes every stale frame', () => {
  const f = fixture();
  const oldIngress = f.adapter.start()!;
  const first = f.decoders[0];
  oldIngress(au());
  f.adapter.stop();
  const stoppedFrame = decoded(0);
  first.init.output(stoppedFrame);
  expect(stoppedFrame.close).toHaveBeenCalledTimes(1);
  expect(f.drawImage).not.toHaveBeenCalled();

  const currentIngress = f.adapter.start()!;
  expect(oldIngress(au())).toBe(false);
  expect(f.decoders[1].decode).not.toHaveBeenCalled();
  expect(currentIngress(au())).toBe(true);
  const stale = decoded(0);
  first.init.output(stale);
  first.init.error(new DOMException('private diagnostic fixture'));
  expect(stale.close).toHaveBeenCalledTimes(1);
  expect(f.drawImage).not.toHaveBeenCalled();
  expect(f.failed).not.toHaveBeenCalled();

  const fresh = decoded(0);
  f.decoders[1].init.output(fresh);
  expect(f.drawImage).toHaveBeenCalledExactlyOnceWith(fresh, 0, 0, 1280, 720);
  expect(fresh.close).toHaveBeenCalledTimes(1);
});

it.each([
  au(0, false),
  { ...au(), annexB: Uint8Array.from([1, 2, 3, 4]) },
  { ...au(), annexB: Uint8Array.from(nal(0x65, [0x80])) },
  { ...au(), annexB: Uint8Array.from([...keyAu(), ...nal(0x81, [0x80])]) },
  { ...au(), annexB: new Uint8Array(1024 * 1024 + 1) },
  { ...au(), timestampUs: Number.NaN },
])(
  'fails closed before sending a malformed Annex B AU to the browser',
  (frame) => {
    const f = fixture();
    const ingest = f.adapter.start()!;
    expect(ingest(frame)).toBe(false);
    expect(f.chunks).not.toHaveBeenCalled();
    expect(f.failed).toHaveBeenCalledExactlyOnceWith('VIDEO_AU_INVALID');
    expect(f.decoders[0].close).toHaveBeenCalledTimes(1);
  },
);

it.each([au(2), { ...au(1), timestampUs: 0 }])(
  'fails closed on an input gap or repeated media timestamp',
  (frame) => {
    const f = fixture();
    const ingest = f.adapter.start()!;
    ingest(au());
    f.decoders[0].init.output(decoded(0));
    expect(f.visible()).toBe(true);
    expect(ingest(frame)).toBe(false);
    expect(f.failed).toHaveBeenCalledExactlyOnceWith('VIDEO_AU_GAP');
    expect(f.visible()).toBe(false);
    expect(f.decoders[0].decode).toHaveBeenCalledTimes(1);
  },
);

it('rejects a missing initial sequence and unbounded pending decoder output', () => {
  const f = fixture();
  const first = f.adapter.start()!;
  expect(first(au(1, true))).toBe(false);
  expect(f.failed).toHaveBeenLastCalledWith('VIDEO_AU_GAP');
  const ingest = f.adapter.start()!;
  for (let i = 0; i < MAX_PENDING_VIDEO_FRAMES; i++)
    expect(ingest(au(i))).toBe(true);
  expect(ingest(au(MAX_PENDING_VIDEO_FRAMES))).toBe(false);
  expect(f.failed).toHaveBeenLastCalledWith('VIDEO_QUEUE_EXCEEDED');
  expect(f.decoders[1].decode).toHaveBeenCalledTimes(MAX_PENDING_VIDEO_FRAMES);
});

it('checks the native decode queue as a separate backpressure bound', () => {
  const f = fixture();
  const ingest = f.adapter.start()!;
  f.decoders[0].decodeQueueSize = MAX_PENDING_VIDEO_FRAMES;
  expect(ingest(au())).toBe(false);
  expect(f.failed).toHaveBeenCalledExactlyOnceWith('VIDEO_QUEUE_EXCEEDED');
  expect(f.chunks).not.toHaveBeenCalled();
});

it('closes invalid or misordered decoder output, never painting it', () => {
  const f = fixture();
  const first = f.adapter.start()!;
  first(au());
  first(au(1));
  const ahead = decoded(33_333);
  f.decoders[0].init.output(ahead);
  expect(ahead.close).toHaveBeenCalledTimes(1);
  expect(f.failed).toHaveBeenLastCalledWith('VIDEO_OUTPUT_INVALID');
  expect(f.visible()).toBe(false);

  f.adapter.start()!(au());
  const oversized = decoded(0, 4000);
  f.decoders[1].init.output(oversized);
  expect(oversized.close).toHaveBeenCalledTimes(1);
  expect(f.failed).toHaveBeenLastCalledWith('VIDEO_OUTPUT_INVALID');
  expect(f.drawImage).not.toHaveBeenCalled();
});

it('clears the canvas on a paint fault and emits only a fixed error code', () => {
  const f = fixture();
  expect(f.adapter.start('avc1.4d001f')).toBeUndefined();
  expect(f.factory).not.toHaveBeenCalled();
  expect(f.failed).toHaveBeenLastCalledWith('VIDEO_DECODER_CONFIG_FAILED');
  f.adapter.start()!(au());
  f.drawImage.mockImplementationOnce(() => {
    throw new Error('sensitive fixture');
  });
  const output = decoded(0);
  f.decoders[0].init.output(output);
  expect(output.close).toHaveBeenCalledTimes(1);
  expect(f.visible()).toBe(false);
  expect(f.failed).toHaveBeenLastCalledWith('VIDEO_PAINT_FAILED');
  expect(f.decoders[0].close).toHaveBeenCalledTimes(1);
});

it('reports only available decoder credits and returns them on valid output', () => {
  const f = fixture();
  expect(f.adapter.availableCapacity()).toBe(0);
  const ingest = f.adapter.start()!;
  expect(f.adapter.availableCapacity()).toBe(MAX_PENDING_VIDEO_FRAMES);
  expect(ingest(au())).toBe(true);
  expect(ingest(au(1))).toBe(true);
  expect(f.adapter.availableCapacity()).toBe(MAX_PENDING_VIDEO_FRAMES - 2);
  f.decoders[0].decodeQueueSize = MAX_PENDING_VIDEO_FRAMES - 1;
  expect(f.adapter.availableCapacity()).toBe(1);
  f.decoders[0].decodeQueueSize = 0;
  f.decoders[0].init.output(decoded(0));
  expect(f.adapter.availableCapacity()).toBe(MAX_PENDING_VIDEO_FRAMES - 1);
  f.adapter.stop();
  expect(f.adapter.availableCapacity()).toBe(0);
});
