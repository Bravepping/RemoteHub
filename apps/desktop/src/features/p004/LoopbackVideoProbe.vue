<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from 'vue';
import {
  getLoopbackStats,
  probeErrorCode,
  signalLoopback,
  startLoopback,
  stopLoopback,
  type ProbeState,
  type ProbeStats,
} from './bridge';
import { LoopbackReceiver, type BrowserStats } from './webrtc';

interface Run {
  controller: AbortController;
  id?: string;
  receiver?: LoopbackReceiver;
  startup?: Promise<void>;
  timer?: ReturnType<typeof setTimeout>;
  ending: boolean;
  played?: boolean;
}

const labels: Record<ProbeState, string> = {
  idle: '待启动',
  starting: '等待原生确认或初始化',
  negotiating: '正在建立本机连接',
  connected: '已连接，等待真实视频帧',
  playing: '已收到视频帧',
  stopping: '正在停止',
  stopped: '已停止',
  failed: '实验失败',
};
const messages: Record<string, string> = {
  ICE_LOOPBACK_UNAVAILABLE:
    'WebView2 没有提供可用的回环候选，本轮已关闭连接。不会改用局域网地址。',
  ICE_GATHER_TIMEOUT: '等待本机 ICE 候选超时。',
  ICE_OFFER_NOT_LOOPBACK: '原生端提供了超出本机回环范围的候选，已拒绝。',
  ICE_DEFAULT_NOT_LOOPBACK: 'SDP 默认地址不在本机回环范围，已拒绝。',
  ICE_SELECTED_NOT_LOOPBACK: '实际选中的连接未确认为本机回环，已停止。',
  SIGNAL_UNSUPPORTED_VIDEO: '未协商出单路可接收的 H.264 视频。',
  SIGNAL_NEGOTIATION_FAILED: 'WebView2 无法建立 H.264 视频会话。',
  VIDEO_PLAY_FAILED: 'WebView2 无法播放收到的视频轨。',
  VIDEO_NO_FRAMES: '实验结束前没有确认收到可播放的视频帧。',
  PEER_FAILED: 'WebView2 的本机 WebRTC 连接已失败。',
  PROBE_STOP_FAILED: '未收到停止确认；请关闭实验窗口。',
  BUSY: '已有实验正在启动或运行。',
  CONSENT_REQUIRED: '没有取得本轮原生授权，未采集主屏。',
};

const video = ref<HTMLVideoElement>();
const seconds = ref(15);
const consent = ref(false);
const state = ref<ProbeState>('idle');
const current = ref<ProbeStats>();
const browser = ref<BrowserStats>();
const failureCode = ref('');
const active = computed(
  () => !['idle', 'stopped', 'failed'].includes(state.value),
);
let run: Run | undefined;

async function terminalStats(id: string): Promise<ProbeStats> {
  const expires = performance.now() + 8_000;
  do {
    const snapshot = await getLoopbackStats(id);
    if (snapshot.state === 'stopped' || snapshot.state === 'failed') {
      return snapshot;
    }
    await new Promise<void>((resolve) => setTimeout(resolve, 200));
  } while (performance.now() < expires);
  throw new Error('PROBE_STOP_FAILED');
}

async function end(
  expected: Run,
  outcome: 'stopped' | 'failed',
  error?: string,
  stopNative = true,
) {
  if (run !== expected || expected.ending) return;
  expected.ending = true;
  expected.controller.abort();
  state.value = 'stopping';
  clearTimeout(expected.timer);
  expected.receiver?.close();
  const idAtStop = expected.id;
  let stopFailed = false;

  // An explicit user stop can cancel native work before its ID is returned.
  if (stopNative && (idAtStop || outcome === 'stopped')) {
    try {
      await stopLoopback(idAtStop);
    } catch {
      stopFailed = true;
    }
  }

  await expected.startup;
  if (stopNative && expected.id && expected.id !== idAtStop) {
    // A receipt may race the cancellation. Release that exact session before
    // enabling another start, so an old completion cannot stop a new run.
    try {
      await stopLoopback(expected.id);
      stopFailed = false;
    } catch {
      stopFailed = true;
    }
  }

  if (!stopFailed && stopNative && expected.id) {
    try {
      const final = await terminalStats(expected.id);
      current.value = final;
      if (final.state === 'failed' && outcome !== 'failed') {
        outcome = 'failed';
        error = probeErrorCode(final.errorCode);
      }
    } catch {
      stopFailed = true;
    }
  }
  if (run !== expected) return;
  if (stopFailed) {
    failureCode.value = 'PROBE_STOP_FAILED';
    state.value = 'failed';
  } else {
    failureCode.value = error ?? '';
    state.value = outcome;
  }
  run = undefined;
}

async function poll(expected: Run) {
  if (run !== expected || expected.ending || !expected.receiver || !expected.id)
    return;
  try {
    const [native, received] = await Promise.all([
      getLoopbackStats(expected.id),
      expected.receiver.sample(),
    ]);
    if (expected.ending) return;
    current.value = native;
    browser.value = received;

    if (native.state === 'failed') {
      void end(expected, 'failed', probeErrorCode(native.errorCode), false);
      return;
    }
    if (received.candidateScope === 'off-host') {
      void end(expected, 'failed', 'ICE_SELECTED_NOT_LOOPBACK');
      return;
    }
    const dimensions = received.width && received.height;
    const observedFrames =
      (received.framesRendered ?? received.framesDecoded ?? 0) > 0;
    if (dimensions && observedFrames) expected.played = true;
    if (native.state === 'stopped') {
      void end(
        expected,
        expected.played ? 'stopped' : 'failed',
        expected.played ? undefined : 'VIDEO_NO_FRAMES',
        false,
      );
      return;
    }
    if (
      native.state !== 'stopping' &&
      ['failed', 'closed'].includes(received.peerState)
    ) {
      void end(expected, 'failed', 'PEER_FAILED');
      return;
    }
    state.value =
      native.state === 'stopping'
        ? 'stopping'
        : native.state === 'connected' && received.peerState === 'connected'
          ? dimensions && observedFrames
            ? 'playing'
            : 'connected'
          : 'negotiating';
    expected.timer = setTimeout(() => void poll(expected), 350);
  } catch (error) {
    void end(expected, 'failed', probeErrorCode(error));
  }
}

async function begin(expected: Run) {
  try {
    const receipt = await startLoopback(seconds.value);
    expected.id = receipt.sessionId;
    if (expected.ending) return;
    if (!video.value) throw new Error('VIDEO_UNAVAILABLE');
    state.value = 'negotiating';
    expected.receiver = new LoopbackReceiver(
      video.value,
      (code) => void end(expected, 'failed', code),
    );
    const answer = await expected.receiver.negotiate(
      receipt.offer,
      expected.controller.signal,
    );
    if (expected.ending) return;
    await signalLoopback(receipt.sessionId, answer);
    if (!expected.ending) void poll(expected);
  } catch (error) {
    void end(expected, 'failed', probeErrorCode(error));
  }
}

function start() {
  if (!consent.value || run) return;
  consent.value = false;
  current.value = undefined;
  browser.value = undefined;
  failureCode.value = '';
  state.value = 'starting';
  const expected: Run = { controller: new AbortController(), ending: false };
  run = expected;
  expected.startup = begin(expected);
}

function stop() {
  if (run) void end(run, 'stopped');
}

onBeforeUnmount(stop);

const value = (input?: number | string | null, unit = '') =>
  input == null ? '未测' : `${input}${unit}`;
</script>

<template>
  <section class="loopback" aria-labelledby="loopback-heading">
    <span class="step">03 / P0-04a 本机实验</span>
    <h2 id="loopback-heading">WebRTC 视频环路</h2>
    <p>
      实时采集本机主屏，经 H.264
      编码后在本窗口解码。仅允许本机回环，不保存画面、不连接其他设备。
    </p>
    <div class="actions">
      <label class="duration">
        采集上限
        <select v-model.number="seconds" :disabled="active">
          <option :value="5">5 秒</option>
          <option :value="15">15 秒</option>
          <option :value="30">30 秒</option>
        </select>
      </label>
      <label class="consent">
        <input v-model="consent" type="checkbox" :disabled="active" />
        我同意本次主屏采集与回放
      </label>
      <button :disabled="!consent || active" @click="start">
        开始视频实验
      </button>
      <button
        class="secondary"
        :disabled="!active || state === 'stopping'"
        @click="stop"
      >
        停止实验
      </button>
    </div>
    <p role="status" aria-live="polite">实验状态：{{ labels[state] }}</p>
    <p v-if="failureCode" class="error" role="alert">
      {{ messages[failureCode] ?? '实验已失败，未确认视频链路可用。' }}
      ({{ failureCode }})
    </p>
    <div class="preview">
      <video
        ref="video"
        aria-label="本机 WebRTC 视频"
        autoplay
        playsinline
        muted
      />
      <p v-if="state !== 'playing'" class="placeholder" aria-hidden="true">
        {{
          state === 'connected' ? '等待真实视频帧' : '实时视频仅在实验期间显示'
        }}
      </p>
    </div>
    <div class="measurements">
      <div>
        <strong>原生发送端</strong>
        <dl>
          <dt>输入 / 编码 / 发送帧</dt>
          <dd>
            {{ value(current?.submittedFrames) }} /
            {{ value(current?.framesEncoded) }} /
            {{ value(current?.framesSent) }}
          </dd>
          <dt>RTP 提交 / 发送包</dt>
          <dd>
            {{ value(current?.packetsSent) }} /
            {{ value(current?.rtpPacketsSent) }}
          </dd>
          <dt>RTP 提交 / 发送字节</dt>
          <dd>
            {{ value(current?.bytesSent, ' B') }} /
            {{ value(current?.rtpBytesSent, ' B') }}
          </dd>
          <dt>首 / 末 PTS (100ns)</dt>
          <dd>
            {{ value(current?.firstPts100ns) }} /
            {{ value(current?.lastPts100ns) }}
          </dd>
          <dt>背压 / 错过节拍 / IDR</dt>
          <dd>
            {{ value(current?.backpressureCount) }} /
            {{ value(current?.missedTicks) }} / {{ value(current?.idrFrames) }}
          </dd>
          <dt>原生编码器</dt>
          <dd>{{ value(current?.encoder) }}</dd>
          <dt>协商 / 实际 H.264 profile</dt>
          <dd>
            {{ value(current?.profileLevelId) }} /
            {{ value(current?.spsProfileLevelId) }}
          </dd>
          <dt>RTP 封包模式 / 载荷</dt>
          <dd>
            {{ value(current?.packetizationMode) }} /
            {{ value(current?.payloadType) }}
          </dd>
          <dt>RTCP PLI / FIR / NACK</dt>
          <dd>
            {{ value(current?.rtcpPliReceived) }} /
            {{ value(current?.rtcpFirReceived) }} /
            {{ value(current?.rtcpNackReceived) }}
          </dd>
          <dt>RTCP 报告丢包</dt>
          <dd>{{ value(current?.rtcpReportedPacketsLost) }}</dd>
          <dt>原生已核实候选类型</dt>
          <dd>{{ value(current?.candidatePairType) }}</dd>
          <dt>会话耗时</dt>
          <dd>{{ value(current?.elapsedMs, ' ms') }}</dd>
        </dl>
      </div>
      <div>
        <strong>WebView2 接收端</strong>
        <dl>
          <dt>播放尺寸</dt>
          <dd>
            {{
              browser?.width && browser?.height
                ? `${browser.width} × ${browser.height}`
                : '未测'
            }}
          </dd>
          <dt>接收包 / 丢包</dt>
          <dd>
            {{ value(browser?.packetsReceived) }} /
            {{ value(browser?.packetsLost) }}
          </dd>
          <dt>解码 / 呈现帧</dt>
          <dd>
            {{ value(browser?.framesDecoded) }} /
            {{ value(browser?.framesRendered) }}
          </dd>
          <dt>首帧 / 播放</dt>
          <dd>
            {{ value(browser?.firstFrameMs, ' ms') }} /
            {{ value(browser?.playbackMs, ' ms') }}
          </dd>
          <dt>编码 / ICE</dt>
          <dd>{{ value(browser?.codec) }} / {{ value(browser?.iceState) }}</dd>
          <dt>已选候选</dt>
          <dd>
            {{ value(browser?.localCandidateType) }} /
            {{ value(browser?.remoteCandidateType) }} ·
            {{ value(browser?.candidateProtocol) }} ·
            {{
              browser?.candidateScope === 'loopback'
                ? '回环'
                : browser?.candidateScope === 'off-host'
                  ? '非回环'
                  : '未核实'
            }}
          </dd>
        </dl>
      </div>
    </div>
    <p>首帧以 WebView2 开始 SDP 协商为起点；实际播放时长以首尾呈现帧为准。</p>
  </section>
</template>

<style scoped>
.loopback {
  margin-top: 20px;
}
.actions {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
  margin: 12px 0;
}
.actions .consent {
  margin: 0 4px;
}
.duration {
  display: flex;
  gap: 6px;
  align-items: center;
  font-size: 12px;
  color: #475569;
}
select {
  background-color: #fff;
  border: 1px solid #d5deea;
  border-radius: 6px;
  padding: 5px;
}
.preview {
  position: relative;
  overflow: hidden;
  background: #142236;
  border-radius: 8px;
  max-width: 640px;
  margin: 12px 0;
}
video {
  display: block;
  width: 100%;
  aspect-ratio: 16 / 9;
}
.placeholder {
  position: absolute;
  inset: 0;
  display: grid;
  place-items: center;
  color: #bac8db;
  background: #142236;
  pointer-events: none;
}
.measurements {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  gap: 14px;
  font-size: 12px;
}
</style>
