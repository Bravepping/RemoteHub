export const BASELINE_AVC_CODEC = 'avc1.42001f';
export const MAX_PENDING_VIDEO_FRAMES = 8;
const MAX_AU_BYTES = 1024 * 1024;
const MAX_NALS = 128;

export interface NativeVideoFrame {
  sequence: number;
  timestampUs: number;
  keyframe: boolean;
  annexB: Uint8Array;
}

export interface CanvasDecoder {
  readonly decodeQueueSize: number;
  configure(config: VideoDecoderConfig): void;
  decode(chunk: EncodedVideoChunk): void;
  close(): void;
}

export type DecoderFactory = (init: VideoDecoderInit) => CanvasDecoder;
export type ChunkFactory = (init: EncodedVideoChunkInit) => EncodedVideoChunk;
export type VideoFrameIngress = (frame: NativeVideoFrame) => boolean;

export type VideoDisplayFailure =
  | 'VIDEO_DISPLAY_UNAVAILABLE'
  | 'VIDEO_DECODER_CONFIG_FAILED'
  | 'VIDEO_AU_INVALID'
  | 'VIDEO_AU_GAP'
  | 'VIDEO_QUEUE_EXCEEDED'
  | 'VIDEO_DECODE_FAILED'
  | 'VIDEO_OUTPUT_INVALID'
  | 'VIDEO_PAINT_FAILED';

interface Generation {
  decoder?: CanvasDecoder;
  pending: Set<number>;
  sequence?: number;
  timestampUs?: number;
  lastPresentedUs?: number;
}

function startCode(bytes: Uint8Array, offset: number): number {
  if (bytes[offset] !== 0 || bytes[offset + 1] !== 0) return 0;
  if (bytes[offset + 2] === 1) return 3;
  return bytes[offset + 2] === 0 && bytes[offset + 3] === 1 ? 4 : 0;
}

function validAnnexB(bytes: Uint8Array, keyframe: boolean): boolean {
  let offset = 0;
  let count = 0;
  let sps = false;
  let pps = false;
  let slices = 0;
  while (offset < bytes.length) {
    const prefix = startCode(bytes, offset);
    const header = offset + prefix;
    if (!prefix || header + 1 >= bytes.length || ++count > MAX_NALS)
      return false;
    let next = header + 1;
    while (next < bytes.length && !startCode(bytes, next)) next++;
    const byte = bytes[header];
    const kind = byte & 31;
    if (next === header + 1 || byte & 0x80) return false;
    if (kind === 7) sps = true;
    else if (kind === 8) pps = true;
    else if (kind === 1 || kind === 5) {
      // A WebCodecs Annex B key AU needs both parameter sets before its IDR.
      if ((kind === 5) !== keyframe || (keyframe && (!sps || !pps)))
        return false;
      slices++;
    } else if (![6, 9, 10, 11, 12].includes(kind)) return false;
    offset = next;
  }
  return slices > 0;
}

function closeDecoder(decoder?: CanvasDecoder) {
  try {
    decoder?.close();
  } catch {
    // Native decoder errors can have closed it already.
  }
}

function closeFrame(frame: VideoFrame) {
  try {
    frame.close();
  } catch {
    // Always make a release attempt, including outputs from a stopped decoder.
  }
}

/**
 * Rendering only. The caller must already have native authorization and a
 * verified media stream. This object never acquires or grants either.
 */
export class CanvasVideoAdapter {
  private active?: Generation;
  private readonly context: CanvasRenderingContext2D | null;

  constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly decoderFactory: DecoderFactory = (init) =>
      new VideoDecoder(init),
    private readonly onFailure: (code: VideoDisplayFailure) => void = () => {},
    private readonly chunkFactory: ChunkFactory = (init) =>
      new EncodedVideoChunk(init),
    private readonly onPresented: () => void = () => {},
  ) {
    this.context = canvas.getContext('2d', { alpha: false });
  }

  // Bind the native event listener to this returned closure, not to a mutable
  // "current" ingress. Even sequence 0 from an older listener must be dropped.
  start(codec = BASELINE_AVC_CODEC): VideoFrameIngress | undefined {
    this.stop();
    if (!this.context) {
      this.onFailure('VIDEO_DISPLAY_UNAVAILABLE');
      return undefined;
    }
    if (!/^avc1\.42[0-9a-f]{4}$/i.test(codec)) {
      this.onFailure('VIDEO_DECODER_CONFIG_FAILED');
      return undefined;
    }
    const run: Generation = { pending: new Set() };
    this.active = run;
    try {
      const decoder = this.decoderFactory({
        output: (frame) => this.paint(run, frame),
        error: () => this.fail(run, 'VIDEO_DECODE_FAILED'),
      });
      if (this.active !== run) {
        closeDecoder(decoder);
        return undefined;
      }
      run.decoder = decoder;
      // With no AVCC description present, this config consumes Annex B.
      decoder.configure({ codec, optimizeForLatency: true });
    } catch {
      this.fail(run, 'VIDEO_DECODER_CONFIG_FAILED');
    }
    return this.active === run ? (frame) => this.push(run, frame) : undefined;
  }

  private push(run: Generation, frame: NativeVideoFrame): boolean {
    if (this.active !== run || !run.decoder) return false;
    const decoder = run.decoder;
    if (
      !frame ||
      !Number.isSafeInteger(frame.sequence) ||
      frame.sequence < 0 ||
      !Number.isSafeInteger(frame.timestampUs) ||
      frame.timestampUs < 0 ||
      typeof frame.keyframe !== 'boolean' ||
      (!frame.keyframe && run.sequence === undefined) ||
      !(frame.annexB instanceof Uint8Array) ||
      frame.annexB.length === 0 ||
      frame.annexB.length > MAX_AU_BYTES
    )
      return this.fail(run, 'VIDEO_AU_INVALID');
    if (
      frame.sequence !== (run.sequence === undefined ? 0 : run.sequence + 1) ||
      (run.timestampUs !== undefined && frame.timestampUs <= run.timestampUs)
    )
      return this.fail(run, 'VIDEO_AU_GAP');
    if (
      run.pending.size >= MAX_PENDING_VIDEO_FRAMES ||
      !Number.isSafeInteger(decoder.decodeQueueSize) ||
      decoder.decodeQueueSize < 0 ||
      decoder.decodeQueueSize >= MAX_PENDING_VIDEO_FRAMES
    )
      return this.fail(run, 'VIDEO_QUEUE_EXCEEDED');
    try {
      // Validate and submit the same private bytes, even if the caller's
      // Uint8Array aliases a SharedArrayBuffer or is reused by a producer.
      const bytes = new Uint8Array(frame.annexB);
      if (!validAnnexB(bytes, frame.keyframe))
        return this.fail(run, 'VIDEO_AU_INVALID');
      const chunk = this.chunkFactory({
        type: frame.keyframe ? 'key' : 'delta',
        timestamp: frame.timestampUs,
        data: bytes,
      });
      if (this.active !== run) return false;
      run.sequence = frame.sequence;
      run.timestampUs = frame.timestampUs;
      run.pending.add(frame.timestampUs);
      decoder.decode(chunk);
      return this.active === run;
    } catch {
      return this.fail(run, 'VIDEO_DECODE_FAILED');
    }
  }

  stop(): void {
    const run = this.active;
    this.active = undefined;
    run?.pending.clear();
    // Reset the backing store synchronously, including any context transforms.
    try {
      this.canvas.width = this.canvas.width;
    } finally {
      closeDecoder(run?.decoder);
    }
  }

  private fail(run: Generation, code: VideoDisplayFailure): false {
    if (this.active === run) {
      this.stop();
      this.onFailure(code);
    }
    return false;
  }

  private paint(run: Generation, frame: VideoFrame) {
    try {
      if (this.active !== run || !this.context) return;
      const { timestamp, displayWidth: width, displayHeight: height } = frame;
      if (
        !Number.isSafeInteger(timestamp) ||
        run.pending.values().next().value !== timestamp ||
        !run.pending.delete(timestamp) ||
        (run.lastPresentedUs !== undefined &&
          timestamp <= run.lastPresentedUs) ||
        !Number.isSafeInteger(width) ||
        width < 1 ||
        width > 3840 ||
        !Number.isSafeInteger(height) ||
        height < 1 ||
        height > 2160
      ) {
        this.fail(run, 'VIDEO_OUTPUT_INVALID');
        return;
      }
      if (this.canvas.width !== width) this.canvas.width = width;
      if (this.canvas.height !== height) this.canvas.height = height;
      this.context.drawImage(frame, 0, 0, width, height);
      run.lastPresentedUs = timestamp;
      this.onPresented();
    } catch {
      this.fail(run, 'VIDEO_PAINT_FAILED');
    } finally {
      closeFrame(frame);
    }
  }
}
