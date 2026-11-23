'use strict';

const WIDTH = 320;
const HEIGHT = 180;
const TIMEOUT_MS = 6000;
const SAMPLE_AFTER_SEC = 0.35;
const TOLERANCE = 28;
const centers = [
  [80, 45, [255, 0, 0]],
  [240, 45, [0, 255, 0]],
  [80, 135, [0, 0, 255]],
  [240, 135, [255, 255, 0]],
];
const video = document.querySelector('#pattern');
const play = document.querySelector('#play');
const stop = document.querySelector('#stop');
const reset = document.querySelector('#reset');
const output = document.querySelector('#result-code');
const detail = document.querySelector('#result-text');
let active;
let state = 'DISPLAY_READY';

function show(code, message) {
  state = code;
  output.textContent = code;
  detail.textContent = message;
  play.disabled = !!active;
  stop.disabled = !active;
  reset.disabled = code === 'DISPLAY_READY';
}

function release(run) {
  if (active !== run) return false;
  active = undefined;
  clearTimeout(run.timer);
  if (run.callback !== undefined) video.cancelVideoFrameCallback(run.callback);
  video.pause();
  return true;
}

function finish(run, code, message) {
  if (release(run)) show(code, message);
}

function rewind() {
  video.pause();
  if (video.readyState > 0) video.currentTime = 0;
}

function samplePixels() {
  const canvas = document.createElement('canvas');
  canvas.width = WIDTH;
  canvas.height = HEIGHT;
  const context = canvas.getContext('2d', { willReadFrequently: true });
  if (!context) throw new Error();
  context.drawImage(video, 0, 0, WIDTH, HEIGHT);
  return centers.every(([x, y, expected]) => {
    const pixels = context.getImageData(x - 2, y - 2, 5, 5).data;
    const channels = [0, 0, 0];
    for (let i = 0; i < pixels.length; i += 4) {
      if (pixels[i + 3] < 250) return false;
      for (let j = 0; j < 3; j++) channels[j] += pixels[i + j];
    }
    return channels.every(
      (sum, j) => Math.abs(sum / (pixels.length / 4) - expected[j]) <= TOLERANCE,
    );
  });
}

function isVisible() {
  const rect = video.getBoundingClientRect();
  return document.visibilityState === 'visible' &&
    rect.width > 0 && rect.height > 0 && rect.left < innerWidth &&
    rect.right > 0 && rect.top < innerHeight && rect.bottom > 0;
}

function start() {
  if (active) return;
  if (typeof video.requestVideoFrameCallback !== 'function' ||
      typeof video.cancelVideoFrameCallback !== 'function') {
    show('DISPLAY_FRAME_CALLBACK_UNAVAILABLE', '当前浏览器不提供可验证的视频帧呈现回调。');
    return;
  }
  rewind();
  const run = {};
  active = run;
  show('DISPLAY_RUNNING', '正在等待视频帧并采样四色图案。');
  run.timer = setTimeout(() => {
    finish(run, 'DISPLAY_FRAME_TIMEOUT', '未在 6 秒内获得连续的可见视频帧。');
  }, TIMEOUT_MS);
  function nextFrame(_now, metadata) {
    run.callback = undefined;
    if (active !== run) return;
    if (!isVisible()) {
      run.callback = video.requestVideoFrameCallback(nextFrame);
      return;
    }
    if (video.videoWidth !== WIDTH || video.videoHeight !== HEIGHT ||
        metadata.width !== WIDTH || metadata.height !== HEIGHT) {
      finish(run, 'DISPLAY_DIMENSIONS_MISMATCH', '实际视频尺寸不是 320 × 180。');
      return;
    }
    const frames = metadata.presentedFrames;
    const time = metadata.mediaTime;
    if (Number.isSafeInteger(frames) && frames > 0 && Number.isFinite(time)) {
      if (run.firstFrame === undefined) {
        run.firstFrame = frames;
        run.firstTime = time;
      } else if (frames > run.firstFrame && time > run.firstTime && time >= SAMPLE_AFTER_SEC) {
        try {
          const matches = samplePixels();
          finish(run, matches ? 'DISPLAY_PASS' : 'DISPLAY_PIXELS_MISMATCH',
            matches
              ? '已收到连续呈现回调，320 × 180 视频的四色像素符合预期。'
              : '已呈现视频，但至少一个采样区的颜色不符合预期。');
        } catch {
          finish(run, 'DISPLAY_PIXELS_UNAVAILABLE', '无法读取本地视频像素。');
        }
        return;
      }
    }
    run.callback = video.requestVideoFrameCallback(nextFrame);
  }
  run.callback = video.requestVideoFrameCallback(nextFrame);
  try {
    void video.play().catch(() => {
      finish(run, 'DISPLAY_PLAY_FAILED', '无法播放内置合成视频。');
    });
  } catch {
    finish(run, 'DISPLAY_PLAY_FAILED', '无法播放内置合成视频。');
  }
}

play.addEventListener('click', start);
stop.addEventListener('click', () => {
  if (active) finish(active, 'DISPLAY_STOPPED', '手动停止，未判定为通过。');
});
reset.addEventListener('click', () => {
  if (state === 'DISPLAY_READY') return;
  if (active) release(active);
  rewind();
  show('DISPLAY_READY', '等待手动播放。');
});
video.addEventListener('error', () => {
  if (active) finish(active, 'DISPLAY_PLAY_FAILED', '内置合成视频读取或解码失败。');
});
window.addEventListener('pagehide', () => {
  if (active) release(active);
});
