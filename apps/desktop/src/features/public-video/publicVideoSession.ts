import type {
  NativeVideoFrame,
  VideoDisplayFailure,
  VideoFrameIngress,
} from './canvasVideoAdapter';

const FRAME_HEADER_BYTES = 24;
const MAX_AU_BYTES = 1024 * 1024;
const MAX_SAFE_BIGINT = BigInt(Number.MAX_SAFE_INTEGER);
const STATUS_INTERVAL_MS = 350;
const EMPTY_FRAME_DELAY_MS = 33;
const IPC_DEADLINE_MS = 3500;

export type PublicVideoRole = 'host' | 'viewer';
export type PublicVideoState =
  | 'idle'
  | 'starting'
  | 'waiting'
  | 'sharing'
  | 'watching'
  | 'stopping'
  | 'stopped'
  | 'failed'
  | 'cleanup_unconfirmed';

type NativePhase = 'starting' | 'ready' | 'stopping' | 'closed';
// Fixed native diagnostics only. Never render arbitrary IPC text, even when
// it starts with a recognized prefix.
const NATIVE_DIAGNOSTICS = [
  'NATIVE_CONSENT_DENIED',
  'PUBLIC_VIDEO_BUSY',
  'PUBLIC_VIDEO_CAPTURE_BACKPRESSURE',
  'PUBLIC_VIDEO_CAPTURE_ENCODER_FAILED',
  'PUBLIC_VIDEO_CAPTURE_INVALID_AU',
  'PUBLIC_VIDEO_CAPTURE_PATH_TIMEOUT',
  'PUBLIC_VIDEO_CAPTURE_WORKER_FAILED',
  'PUBLIC_VIDEO_CLOSED_UNEXPECTEDLY',
  'PUBLIC_VIDEO_FRAME_SIZE',
  'PUBLIC_VIDEO_ICE_PAIR_REJECTED',
  'PUBLIC_VIDEO_ICE_PAIR_UNAVAILABLE',
  'PUBLIC_VIDEO_MEDIA_BACKPRESSURE',
  'PUBLIC_VIDEO_MEDIA_INVALID_AU',
  'PUBLIC_VIDEO_PEER_MISMATCH',
  'PUBLIC_VIDEO_PROFILE_CA_PIN',
  'PUBLIC_VIDEO_PROFILE_EXPIRED',
  'PUBLIC_VIDEO_PROFILE_FILE',
  'PUBLIC_VIDEO_PROFILE_INVALID',
  'PUBLIC_VIDEO_PROFILE_UNAVAILABLE',
  'PUBLIC_VIDEO_PULL_BUSY',
  'PUBLIC_VIDEO_ROLE_OR_ROUTE',
  'PUBLIC_VIDEO_RTP_UNAVAILABLE',
  'PUBLIC_VIDEO_VERIFY_PEER_FIRST',
  'PUBLIC_VIDEO_WSS_ADMISSION',
  'PUBLIC_VIDEO_WSS_CLOSED',
  'PUBLIC_VIDEO_WSS_INVITATION',
  'PUBLIC_VIDEO_WSS_PROTOCOL',
  'PUBLIC_VIDEO_WSS_TIMEOUT',
  'PUBLIC_VIDEO_WSS_TLS',
  'PUBLIC_VIDEO_WSS_TRANSPORT',
] as const;
type NativeDiagnostic = (typeof NATIVE_DIAGNOSTICS)[number];
const nativeDiagnostics: ReadonlySet<string> = new Set(NATIVE_DIAGNOSTICS);
type PublicVideoError =
  | 'PUBLIC_VIDEO_OP_FAILED'
  | 'PUBLIC_VIDEO_RESPONSE_INVALID'
  | 'PUBLIC_VIDEO_IPC_TIMEOUT'
  | NativeDiagnostic
  | VideoDisplayFailure;

export interface PublicVideoBridge {
  start(role: PublicVideoRole): Promise<unknown>;
  status(generation: string): Promise<unknown>;
  nextFrame(generation: string): Promise<unknown>;
  stop(generation: string): Promise<unknown>;
}

export interface PublicVideoRenderer {
  start(): VideoFrameIngress | undefined;
  stop(): void;
}

interface Run {
  readonly generation: Promise<string>;
  readonly role: PublicVideoRole;
  renderer?: PublicVideoRenderer;
  ingress?: VideoFrameIngress;
  cancelled: boolean;
  revoke?: Promise<void>;
  delay?: () => void;
}

/** Only trusted native IPC is wired to this parser; a valid frame is not authorization. */
export function parsePublicVideoFrame(value: unknown): NativeVideoFrame | null {
  if (!(value instanceof ArrayBuffer)) throw new InvalidResponse();
  if (value.byteLength === 0) return null;
  if (
    value.byteLength <= FRAME_HEADER_BYTES ||
    value.byteLength > FRAME_HEADER_BYTES + MAX_AU_BYTES
  )
    throw new InvalidResponse();
  const header = new DataView(value);
  const sequence = header.getBigUint64(0, true);
  const timestampUs = header.getBigUint64(8, true);
  const bytes = header.getUint32(16, true);
  const flags = header.getUint8(20);
  if (
    sequence > MAX_SAFE_BIGINT ||
    timestampUs > MAX_SAFE_BIGINT ||
    bytes !== value.byteLength - FRAME_HEADER_BYTES ||
    flags > 1 ||
    header.getUint8(21) !== 0 ||
    header.getUint8(22) !== 0 ||
    header.getUint8(23) !== 0
  )
    throw new InvalidResponse();
  return {
    sequence: Number(sequence),
    timestampUs: Number(timestampUs),
    keyframe: flags === 1,
    annexB: new Uint8Array(value, FRAME_HEADER_BYTES, bytes),
  };
}

class InvalidResponse extends Error {}
class IpcTimeout extends Error {}
class NativeFailure {
  constructor(readonly value: string) {}
}

function parseGeneration(value: unknown): string {
  if (typeof value !== 'string' || !/^[0-9a-f]{32}$/.test(value))
    throw new InvalidResponse();
  return value;
}

function phase(value: unknown): NativePhase {
  if (typeof value !== 'object' || !value) throw new InvalidResponse();
  const parsed = value as { phase?: unknown; error?: unknown };
  if (
    !Object.hasOwn(parsed, 'phase') ||
    (parsed.phase !== 'starting' &&
      parsed.phase !== 'ready' &&
      parsed.phase !== 'stopping' &&
      parsed.phase !== 'closed') ||
    (parsed.error !== undefined &&
      parsed.error !== null &&
      typeof parsed.error !== 'string')
  )
    throw new InvalidResponse();
  if (typeof parsed.error === 'string') throw new NativeFailure(parsed.error);
  return parsed.phase;
}

function errorCode(cause: unknown): PublicVideoError {
  if (cause instanceof InvalidResponse) return 'PUBLIC_VIDEO_RESPONSE_INVALID';
  if (cause instanceof IpcTimeout) return 'PUBLIC_VIDEO_IPC_TIMEOUT';
  // Native Result<_, String> rejections and the typed status wrapper use
  // the same exact allowlist; transport/JS exception messages never pass.
  const native = cause instanceof NativeFailure ? cause.value : cause;
  if (typeof native === 'string' && nativeDiagnostics.has(native))
    return native as NativeDiagnostic;
  // Never put raw native IPC failures, profile values, or transport data in UI.
  return 'PUBLIC_VIDEO_OP_FAILED';
}

async function withDeadline<T>(job: Promise<T>): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      job,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new IpcTimeout()), IPC_DEADLINE_MS);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

export class PublicVideoSession {
  private run?: Run;
  private outstandingFrame?: Promise<unknown>;
  private disposed = false;
  private cleanupUnconfirmed = false;

  constructor(
    private readonly bridge: PublicVideoBridge,
    private readonly createRenderer: (
      failed: (code: VideoDisplayFailure) => void,
      presented: () => void,
    ) => PublicVideoRenderer,
    private readonly update: (
      state: PublicVideoState,
      error?: PublicVideoError,
    ) => void,
  ) {}

  start(role: PublicVideoRole): void {
    if (this.disposed || this.run || this.cleanupUnconfirmed) return;
    const run: Run = {
      role,
      cancelled: false,
      generation: Promise.resolve()
        .then(() => {
          if (!this.active(run)) throw new Error('Cancelled before dispatch');
          return this.bridge.start(role);
        })
        .then(parseGeneration),
    };
    this.run = run;
    this.update('starting');
    void this.work(run);
  }

  stop(): Promise<void> {
    return this.run ? this.retire(this.run, 'stopped') : Promise.resolve();
  }

  dispose(): void {
    this.disposed = true;
    void this.stop();
  }

  private active(run: Run) {
    return !this.disposed && this.run === run && !run.cancelled;
  }

  private async work(run: Run): Promise<void> {
    try {
      const generation = await withDeadline(run.generation);
      let statusAt = -Infinity;
      let ready = false;
      while (this.active(run)) {
        if (performance.now() - statusAt >= STATUS_INTERVAL_MS) {
          const current = phase(
            await withDeadline(this.bridge.status(generation)),
          );
          if (!this.active(run)) return;
          statusAt = performance.now();
          if (current === 'closed' || current === 'stopping') {
            await this.retire(run, 'stopped');
            return;
          }
          if (ready && current !== 'ready') throw new InvalidResponse();
          if (!ready && current === 'ready') {
            ready = true;
            if (run.role === 'host') {
              this.update('sharing');
            } else {
              run.renderer = this.createRenderer(
                (code) => {
                  if (this.active(run)) void this.retire(run, 'failed', code);
                },
                () => {
                  if (this.active(run)) this.update('watching');
                },
              );
              run.ingress = run.renderer.start();
              if (!this.active(run)) return;
              if (!run.ingress) throw new Error('Decoder unavailable');
              this.update('waiting');
            }
          }
        }
        if (!run.ingress) {
          await this.delay(run, STATUS_INTERVAL_MS);
          continue;
        }
        // A previous generation's IPC may settle after its stop acknowledgement.
        // Keep a global one-pull bound without delivering that old response.
        if (this.outstandingFrame) {
          await withDeadline(
            this.outstandingFrame.then(
              () => undefined,
              () => undefined,
            ),
          );
        }
        if (!this.active(run)) return;
        const pending = this.bridge.nextFrame(generation);
        this.outstandingFrame = pending;
        const released = () => {
          if (this.outstandingFrame === pending)
            this.outstandingFrame = undefined;
        };
        void pending.then(released, released);
        const bytes = await withDeadline(pending);
        if (!this.active(run)) return;
        const frame = parsePublicVideoFrame(bytes);
        if (!frame) await this.delay(run, EMPTY_FRAME_DELAY_MS);
        else if (!run.ingress(frame)) throw new Error('Decoder rejected frame');
      }
    } catch (cause) {
      if (this.active(run)) await this.retire(run, 'failed', errorCode(cause));
    }
  }

  private delay(run: Run, ms: number): Promise<void> {
    return new Promise((resolve) => {
      const done = () => {
        clearTimeout(timer);
        run.delay = undefined;
        resolve();
      };
      const timer = setTimeout(done, ms);
      run.delay = done;
    });
  }

  private retire(
    run: Run,
    state: 'stopped' | 'failed',
    code?: PublicVideoError,
  ): Promise<void> {
    if (run.revoke) return run.revoke;
    // Never wait on native stop or a pending frame before clearing old pixels.
    run.cancelled = true;
    run.delay?.();
    run.ingress = undefined;
    run.renderer?.stop();
    run.renderer = undefined;
    if (!this.disposed) this.update('stopping');
    const cleanup = async () => {
      const generation = await run.generation.catch((cause: unknown) => {
        if (cause instanceof InvalidResponse) throw cause;
        // A rejected start never allocated/returned a generation.
        return undefined;
      });
      if (generation) await this.bridge.stop(generation);
    };
    // Even if this expires, cleanup remains attached to any late generation.
    run.revoke = withDeadline(cleanup()).then(
      () => {
        if (this.run === run) this.run = undefined;
        if (!this.disposed) this.update(state, code);
      },
      () => {
        this.cleanupUnconfirmed = true;
        if (!this.disposed) this.update('cleanup_unconfirmed', code);
      },
    );
    return run.revoke;
  }
}
