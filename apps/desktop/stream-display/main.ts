import { invoke, isTauri } from '@tauri-apps/api/core';
import { checkQuadrants } from './pixels';
import {
  CanvasVideoAdapter,
  MAX_PENDING_VIDEO_FRAMES,
  type NativeVideoFrame,
} from '../src/features/public-video/canvasVideoAdapter';

const NATIVE_TIMEOUT_MS = 18_000;
const STOP_TIMEOUT_MS = 5_000;
const DECODE_TIMEOUT_MS = 6_000;
const MAX_AU_BYTES = 1024 * 1024;
const MAX_BATCH_BYTES = 2 * MAX_AU_BYTES;
const SIZE = 32;

const canvas = document.querySelector<HTMLCanvasElement>('#display')!;
// This small diagnostics page reads every frame. Request a CPU-backed canvas
// before the adapter acquires it, without changing product rendering settings.
const readback = canvas.getContext('2d', {
  alpha: false,
  willReadFrequently: true,
});
const startButton = document.querySelector<HTMLButtonElement>('#start')!;
const stopButton = document.querySelector<HTMLButtonElement>('#stop')!;
const resetButton = document.querySelector<HTMLButtonElement>('#reset')!;
const output = document.querySelector<HTMLOutputElement>('#result-code')!;
const detail = document.querySelector<HTMLElement>('#result-text')!;

interface Run {
  nativePending: boolean;
  expectedPaints: number;
  painted: number;
  timer?: ReturnType<typeof setTimeout>;
  decoder?: VideoDecoder;
  adapter?: CanvasVideoAdapter;
}

let current: Run | undefined;
let cancelling = false;
let stopUnconfirmed = false;
let code = 'STREAM_DISPLAY_READY';

function show(next: string, message: string) {
  code = next;
  output.textContent = next;
  detail.textContent = message;
  startButton.disabled = !!current || cancelling || stopUnconfirmed;
  stopButton.disabled = !current;
  resetButton.disabled =
    cancelling || stopUnconfirmed || next === 'STREAM_DISPLAY_READY';
}

function clear(run = current) {
  if (current !== run) return false;
  current = undefined;
  clearTimeout(run?.timer);
  if (run?.adapter) run.adapter.stop();
  else canvas.width = canvas.width;
  return true;
}

function fail(run: Run, reason: string, message: string) {
  if (current !== run) return;
  void cancel(reason, message);
}

async function cancel(next: string, message: string) {
  const nativePending = current?.nativePending === true;
  // Invalidate the JS generation and clear the backing store before IPC.
  clear();
  if (!nativePending) {
    show(next, message);
    return;
  }
  cancelling = true;
  show(next, message);
  let deadline: ReturnType<typeof setTimeout> | undefined;
  try {
    await Promise.race([
      invoke('stop_signed_stream'),
      new Promise<never>((_, reject) => {
        deadline = setTimeout(reject, STOP_TIMEOUT_MS);
      }),
    ]);
  } catch {
    stopUnconfirmed = true;
  } finally {
    clearTimeout(deadline);
    cancelling = false;
    if (stopUnconfirmed)
      show(
        'STREAM_DISPLAY_STOP_UNCONFIRMED',
        '画布已清空。未确认原生会话停止，请关闭此窗口。',
      );
    else if (code === next) show(next, message);
  }
}

function readFrames(value: unknown): NativeVideoFrame[] | undefined {
  if (!value || typeof value !== 'object' || !Object.hasOwn(value, 'frames'))
    return;
  const candidate = (value as { frames: unknown }).frames;
  if (
    !Array.isArray(candidate) ||
    candidate.length < 1 ||
    candidate.length > MAX_PENDING_VIDEO_FRAMES
  )
    return;
  const frames: NativeVideoFrame[] = [];
  let total = 0;
  let lastTimestamp = -1;
  for (const item of candidate) {
    if (
      !item ||
      typeof item !== 'object' ||
      Object.keys(item).sort().join(',') !==
        'annexB,keyframe,sequence,timestampUs' ||
      item.sequence !== frames.length ||
      !Number.isSafeInteger(item.timestampUs) ||
      item.timestampUs <= lastTimestamp ||
      typeof item.keyframe !== 'boolean' ||
      (frames.length === 0 && !item.keyframe) ||
      !Array.isArray(item.annexB) ||
      item.annexB.length < 1 ||
      item.annexB.length > MAX_AU_BYTES
    )
      return;
    total += item.annexB.length;
    if (
      total > MAX_BATCH_BYTES ||
      !item.annexB.every(
        (byte: unknown) =>
          Number.isInteger(byte) &&
          (byte as number) >= 0 &&
          (byte as number) <= 255,
      )
    )
      return;
    frames.push({
      sequence: item.sequence,
      timestampUs: item.timestampUs,
      keyframe: item.keyframe,
      annexB: Uint8Array.from(item.annexB as number[]),
    });
    lastTimestamp = item.timestampUs;
  }
  return frames;
}

function afterPaint(run: Run) {
  if (current !== run) return;
  if (canvas.width !== SIZE || canvas.height !== SIZE) {
    fail(
      run,
      'STREAM_DISPLAY_DIMENSIONS_MISMATCH',
      '实际呈现尺寸不是 32 × 32。',
    );
    return;
  }
  try {
    if (!readback) throw new Error();
    const sample = checkQuadrants(readback);
    if (!sample.pass) {
      fail(
        run,
        'STREAM_DISPLAY_PIXELS_MISMATCH',
        `四个灰度区未通过。RGBA（左上、右上、左下、右下）：${sample.rgba}`,
      );
      return;
    }
  } catch {
    fail(run, 'STREAM_DISPLAY_PIXELS_UNAVAILABLE', '无法读取实际画布像素。');
    return;
  }
  run.painted++;
  if (run.painted === run.expectedPaints) {
    clearTimeout(run.timer);
    show(
      'STREAM_DISPLAY_PASS',
      `浏览器已解码并绘制 ${run.painted} 帧 32 × 32 合成图案，四个灰度区符合预期。停止可立即清屏。`,
    );
  }
}

async function start() {
  if (current || cancelling || stopUnconfirmed) return;
  if (!isTauri()) {
    show(
      'STREAM_DISPLAY_NATIVE_UNAVAILABLE',
      '请在专用 Windows 诊断应用内运行。',
    );
    return;
  }
  if (
    typeof VideoDecoder !== 'function' ||
    typeof EncodedVideoChunk !== 'function'
  ) {
    show(
      'STREAM_DISPLAY_DECODER_UNAVAILABLE',
      '此 WebView2 不支持所需的 WebCodecs 解码接口。',
    );
    return;
  }
  const run: Run = { nativePending: true, expectedPaints: 0, painted: 0 };
  current = run;
  show('STREAM_DISPLAY_RUNNING', '等待本机回环返回已核验的合成帧。');
  run.timer = setTimeout(() => {
    fail(
      run,
      'STREAM_DISPLAY_NATIVE_TIMEOUT',
      '等待本机回环超时，已清屏并请求停止。',
    );
  }, NATIVE_TIMEOUT_MS);
  try {
    const value = await invoke<unknown>('run_signed_stream');
    if (current !== run) return;
    run.nativePending = false;
    clearTimeout(run.timer);
    const frames = readFrames(value);
    if (!frames) {
      fail(
        run,
        'STREAM_DISPLAY_RESPONSE_INVALID',
        '本机核心未返回有效的合成帧。',
      );
      return;
    }
    run.expectedPaints = frames.length;
    run.timer = setTimeout(() => {
      fail(
        run,
        'STREAM_DISPLAY_DECODE_TIMEOUT',
        '未在规定时间内绘制全部合成帧。',
      );
    }, DECODE_TIMEOUT_MS);
    const adapter = new CanvasVideoAdapter(
      canvas,
      (init) => {
        const decoder = new VideoDecoder({
          ...init,
          output: (frame) => {
            init.output(frame);
            afterPaint(run);
          },
        });
        run.decoder = decoder;
        // Prefer the software decoder for this two-frame diagnostic to
        // avoid relying on hardware-surface import into the canvas.
        return {
          get decodeQueueSize() {
            return decoder.decodeQueueSize;
          },
          configure: (config) =>
            decoder.configure({
              ...config,
              hardwareAcceleration: 'prefer-software',
            }),
          decode: (chunk) => decoder.decode(chunk),
          close: () => decoder.close(),
        };
      },
      () =>
        fail(
          run,
          'STREAM_DISPLAY_DECODE_FAILED',
          'WebView2 无法解码或绘制本机合成帧。',
        ),
    );
    run.adapter = adapter;
    const ingest = adapter.start();
    if (!ingest || current !== run) return;
    for (const frame of frames) {
      if (current !== run || !ingest(frame)) return;
    }
    // Flush only after submitting the bounded batch. Some decoders buffer
    // their first output until drain, even with optimizeForLatency enabled.
    await run.decoder?.flush();
    if (current === run && run.painted !== run.expectedPaints) {
      fail(
        run,
        'STREAM_DISPLAY_DECODE_FAILED',
        '解码结束，但未绘制全部合成帧。',
      );
    }
  } catch {
    if (current !== run) return;
    if (run.nativePending) {
      run.nativePending = false;
      fail(
        run,
        'STREAM_DISPLAY_NATIVE_FAILED',
        '本机签名视频诊断未完成，未显示视频。',
      );
    } else
      fail(
        run,
        'STREAM_DISPLAY_DECODE_FAILED',
        'WebView2 无法完成合成帧解码。',
      );
  }
}

startButton.addEventListener('click', () => void start());
stopButton.addEventListener(
  'click',
  () => void cancel('STREAM_DISPLAY_STOPPED', '已立即清空画布。'),
);
resetButton.addEventListener(
  'click',
  () => void cancel('STREAM_DISPLAY_READY', '等待手动开始。'),
);
window.addEventListener(
  'pagehide',
  () => void cancel('STREAM_DISPLAY_STOPPED', '已清空画布。'),
);
