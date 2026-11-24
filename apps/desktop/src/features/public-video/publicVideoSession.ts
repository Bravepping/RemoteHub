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
export type PublicVideoEndReason =
  'local_stop' | 'peer_left' | 'capture_limit' | 'capture_ended' | 'failed';
export interface PublicVideoEndDetail {
  readonly reason?: PublicVideoEndReason;
  readonly playbackMs?: number;
  readonly captureMs?: number;
}
export type PublicVideoProfileDisposition =
  'available' | 'consumed' | 'unconfirmed';
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
  'PUBLIC_VIDEO_ICE_PAIR_REJECTED',
  'PUBLIC_VIDEO_ICE_PAIR_UNAVAILABLE',
  'PUBLIC_VIDEO_MEDIA_BACKPRESSURE',
  'PUBLIC_VIDEO_MEDIA_CLOSED',
  'PUBLIC_VIDEO_MEDIA_INVALID_AU',
  'PUBLIC_VIDEO_PEER_MISMATCH',
  'PUBLIC_VIDEO_PROFILE_CA_PIN',
  'PUBLIC_VIDEO_PROFILE_CONSUMED',
  'PUBLIC_VIDEO_PROFILE_EXPIRED',
  'PUBLIC_VIDEO_PROFILE_FILE',
  'PUBLIC_VIDEO_PROFILE_INVALID',
  'PUBLIC_VIDEO_PROFILE_UNAVAILABLE',
  'PUBLIC_VIDEO_PULL_BUSY',
  'PUBLIC_VIDEO_ROLE_OR_ROUTE',
  'PUBLIC_VIDEO_RTP_GAP',
  'PUBLIC_VIDEO_RTP_INVALID',
  'PUBLIC_VIDEO_RTP_SOURCE',
  'PUBLIC_VIDEO_RTP_TIMESTAMP',
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
  | 'PUBLIC_VIDEO_STATUS_IPC_FAILED'
  | 'PUBLIC_VIDEO_FRAME_IPC_FAILED'
  | 'PUBLIC_VIDEO_DISPLAY_FAILED'
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
  firstPresentedAt?: number;
  profileConsumed?: boolean;
  closedProfile?: { consumed?: boolean };
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
function parseGeneration(value: unknown): string {
  if (typeof value !== 'string' || !/^[0-9a-f]{32}$/.test(value))
    throw new InvalidResponse();
  return value;
}

interface NativeStatus {
  phase: NativePhase;
  error?: string;
  end?: PublicVideoEndDetail;
  profileConsumed?: boolean;
}

function parseStatus(value: unknown, role: PublicVideoRole): NativeStatus {
  if (typeof value !== 'object' || !value) throw new InvalidResponse();
  const parsed = value as {
    phase?: unknown;
    error?: unknown;
    end_reason?: unknown;
    elapsed_ms?: unknown;
    profile_consumed?: unknown;
  };
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
  const result: NativeStatus = {
    phase: parsed.phase,
    profileConsumed:
      Object.hasOwn(parsed, 'profile_consumed') &&
      typeof parsed.profile_consumed === 'boolean'
        ? parsed.profile_consumed
        : undefined,
  };
  if (typeof parsed.error === 'string') result.error = parsed.error;
  if (
    result.error !== undefined ||
    parsed.phase === 'stopping' ||
    parsed.phase === 'closed'
  ) {
    const reason = Object.hasOwn(parsed, 'end_reason')
      ? parsed.end_reason
      : undefined;
    const elapsed = Object.hasOwn(parsed, 'elapsed_ms')
      ? parsed.elapsed_ms
      : undefined;
    const end: PublicVideoEndDetail = {
      reason:
        reason === 'local_stop' ||
        reason === 'peer_left' ||
        reason === 'failed' ||
        (role === 'host' &&
          (reason === 'capture_limit' || reason === 'capture_ended'))
          ? reason
          : undefined,
      captureMs:
        role === 'host' &&
        typeof elapsed === 'number' &&
        Number.isSafeInteger(elapsed) &&
        elapsed >= 0
          ? elapsed
          : undefined,
    };
    if (end.reason !== undefined || end.captureMs !== undefined)
      result.end = end;
  }
  return result;
}

type DiagnosticPhase = 'start' | 'status' | 'frame' | 'display';

function errorCode(cause: unknown, phase: DiagnosticPhase): PublicVideoError {
  if (cause instanceof InvalidResponse) return 'PUBLIC_VIDEO_RESPONSE_INVALID';
  if (cause instanceof IpcTimeout) return 'PUBLIC_VIDEO_IPC_TIMEOUT';
  // Native Result<_, String> rejections and the parsed native status use
  // the same exact allowlist; transport/JS exception messages never pass.
  if (typeof cause === 'string' && nativeDiagnostics.has(cause))
    return cause as NativeDiagnostic;
  // Never put raw native IPC failures, profile values, or transport data in UI.
  if (phase === 'status') return 'PUBLIC_VIDEO_STATUS_IPC_FAILED';
  if (phase === 'frame') return 'PUBLIC_VIDEO_FRAME_IPC_FAILED';
  if (phase === 'display') return 'PUBLIC_VIDEO_DISPLAY_FAILED';
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
      detail?: PublicVideoEndDetail,
    ) => void,
    private readonly onProfileDisposition: (
      state: PublicVideoProfileDisposition,
    ) => void = () => {},
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
    return this.run
      ? this.retire(this.run, 'stopped', undefined, { reason: 'local_stop' })
      : Promise.resolve();
  }

  dispose(): void {
    this.disposed = true;
    void this.stop();
  }

  private active(run: Run) {
    return !this.disposed && this.run === run && !run.cancelled;
  }

  private async work(run: Run): Promise<void> {
    let diagnosticPhase: DiagnosticPhase = 'start';
    try {
      const generation = await withDeadline(run.generation);
      let statusAt = -Infinity;
      let ready = false;
      while (this.active(run)) {
        if (performance.now() - statusAt >= STATUS_INTERVAL_MS) {
          diagnosticPhase = 'status';
          const nativeStatus = await withDeadline(
            this.bridge.status(generation),
          );
          if (!this.active(run)) return;
          const current = parseStatus(nativeStatus, run.role);
          this.observeProfile(run, current);
          statusAt = performance.now();
          if (current.error !== undefined || current.end?.reason === 'failed') {
            await this.retire(
              run,
              'failed',
              errorCode(current.error, 'status'),
              current.end,
            );
            return;
          }
          if (current.phase === 'closed' || current.phase === 'stopping') {
            await this.retire(run, 'stopped', undefined, current.end);
            return;
          }
          if (ready && current.phase !== 'ready') throw new InvalidResponse();
          if (!ready && current.phase === 'ready') {
            ready = true;
            if (run.role === 'host') {
              this.update('sharing');
            } else {
              diagnosticPhase = 'display';
              run.renderer = this.createRenderer(
                (code) => {
                  if (this.active(run)) void this.retire(run, 'failed', code);
                },
                () => {
                  if (!this.active(run) || run.firstPresentedAt !== undefined)
                    return;
                  run.firstPresentedAt = performance.now();
                  this.update('watching');
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
        diagnosticPhase = 'frame';
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
        else {
          diagnosticPhase = 'display';
          if (!run.ingress(frame)) throw new Error('Decoder rejected frame');
        }
      }
    } catch (cause) {
      if (this.active(run)) {
        if (cause === 'PUBLIC_VIDEO_PROFILE_CONSUMED')
          this.profileConsumed(run);
        await this.retire(run, 'failed', errorCode(cause, diagnosticPhase));
      }
    }
  }

  private profileConsumed(run: Run): void {
    if (run.profileConsumed) return;
    run.profileConsumed = true;
    this.onProfileDisposition('consumed');
  }

  private observeProfile(run: Run, current: NativeStatus): void {
    if (
      current.profileConsumed ||
      current.error === 'PUBLIC_VIDEO_PROFILE_CONSUMED'
    )
      this.profileConsumed(run);
    if (current.phase === 'closed')
      run.closedProfile = { consumed: current.profileConsumed };
  }

  private async confirmClosedProfile(run: Run, generation: string) {
    if (run.profileConsumed || this.disposed) return;
    // Native stop already acknowledged shutdown. One bounded read covers Stop
    // racing profile consumption, with no further polling.
    if (!run.closedProfile) {
      try {
        const raw = await withDeadline(this.bridge.status(generation));
        if (this.disposed) return;
        this.observeProfile(run, parseStatus(raw, run.role));
      } catch {
        // No fresh terminal proof: do not permit reuse of this profile.
      }
    }
    if (!this.disposed && !run.profileConsumed) {
      this.onProfileDisposition(
        run.closedProfile?.consumed === false ? 'available' : 'unconfirmed',
      );
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
    nativeEnd?: PublicVideoEndDetail,
  ): Promise<void> {
    if (run.revoke) return run.revoke;
    // Never wait on native stop or a pending frame before clearing old pixels.
    run.cancelled = true;
    const detail =
      run.firstPresentedAt === undefined
        ? nativeEnd
        : {
            ...nativeEnd,
            playbackMs: Math.max(
              0,
              Math.floor(performance.now() - run.firstPresentedAt),
            ),
          };
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
      return generation;
    };
    // Even if this expires, cleanup remains attached to any late generation.
    run.revoke = withDeadline(cleanup()).then(
      async (generation) => {
        if (generation) await this.confirmClosedProfile(run, generation);
        if (this.run === run) this.run = undefined;
        if (!this.disposed) {
          if (detail) this.update(state, code, detail);
          else this.update(state, code);
        }
      },
      () => {
        this.cleanupUnconfirmed = true;
        if (!this.disposed) {
          if (detail) this.update('cleanup_unconfirmed', code, detail);
          else this.update('cleanup_unconfirmed', code);
        }
      },
    );
    return run.revoke;
  }
}
