<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { LanProbe, type LanStatus, type LanReceiver } from '../p004/lan';
const video = ref<HTMLVideoElement>();
const surface = ref<HTMLDivElement>();
const role = ref<'sender' | 'receiver'>('receiver');
const peer = ref('');
const local = ref('');
const code = ref('');
const invitation = ref('');
const consent = ref(false);
const busy = ref(false);
const state = ref('idle');
const error = ref('');
const native = ref<LanStatus>();
const receiver = ref<LanReceiver>();
const input = ref(false);
const emit = defineEmits<{ sessionState: [state: string] }>();
let session: LanProbe | undefined;
let lastMove = 0;
const labels: Record<string, string> = {
  idle: '尚未连接',
  starting: '等待原生授权',
  waiting: '等待另一端配对',
  negotiating: '正在协商连接',
  connected: '已连接，等待画面',
  playing: '视频正在播放',
  finishing: '正在收集最终统计',
  stopping: '正在停止和回收',
  stopped: '会话已结束',
  failed: '会话失败',
};
function safeError(value: unknown) {
  const text = String(value);
  const match = text.match(
    /\b(?:INPUT|CONTROL|LAN|ICE|OFF_HOST|NATIVE|SIGNAL|CONSENT|INVALID|PRIVATE|ROUTE|CAPTURE|WEBVIEW|H264|SDP|ANSWER|OFFER|RECEIVER|BUSY|SESSION)[A-Z0-9_]*\b/,
  );
  return match?.[0] ?? 'CONNECTION_FAILED';
}
async function detect() {
  try {
    local.value = await invoke<string>('p004_lan_local_ip', {
      peerIp: peer.value.trim(),
    });
  } catch (e) {
    error.value = safeError(e);
  }
}
async function start() {
  if (busy.value || !video.value || !consent.value) return;
  busy.value = true;
  error.value = '';
  native.value = undefined;
  receiver.value = undefined;
  input.value = false;
  session = new LanProbe(
    video.value,
    (next, n, r) => {
      state.value = next;
      emit('sessionState', next);
      if (n) native.value = n;
      if (r) receiver.value = r;
      if (next === 'stopped' || next === 'failed') {
        busy.value = false;
        pause();
      }
    },
    (e) => {
      error.value = safeError(e);
    },
    (c) => {
      invitation.value = c;
    },
    () => {},
  );
  const pairing = code.value;
  code.value = '';
  consent.value = false;
  await session.start({
    role: role.value,
    consent: true,
    localIp: local.value.trim(),
    peerIp: peer.value.trim(),
    port: 48604,
    seconds: 30,
    pairingCode: pairing,
    realControl: true,
    controlSim: false,
  });
}
function pause() {
  session?.enableInput(false);
  input.value = false;
}
async function stop() {
  pause();
  await session?.stop();
}
function activate() {
  input.value = session?.enableInput(true) ?? false;
  if (input.value) surface.value?.focus();
}
function send(event: Record<string, unknown>) {
  if (input.value) session?.input(event);
}
function move(e: MouseEvent, force = false) {
  if (!input.value || !video.value) return false;
  const v = video.value;
  const box = v.getBoundingClientRect();
  const scale = Math.min(box.width / v.videoWidth, box.height / v.videoHeight);
  const w = v.videoWidth * scale,
    h = v.videoHeight * scale;
  const x = (e.clientX - box.left - (box.width - w) / 2) / w,
    y = (e.clientY - box.top - (box.height - h) / 2) / h;
  if (
    !Number.isFinite(x) ||
    !Number.isFinite(y) ||
    x < 0 ||
    x > 1 ||
    y < 0 ||
    y > 1
  )
    return false;
  if (force || performance.now() - lastMove > 50) {
    lastMove = performance.now();
    send({ kind: 'mouse_move', x_norm: x, y_norm: y });
  }
  return true;
}
function button(e: MouseEvent, down: boolean) {
  if (!input.value) return;
  e.preventDefault();
  if (move(e, true)) send({ kind: 'mouse_button', button: e.button, down });
  else if (!down) pause();
}
function key(e: KeyboardEvent, down: boolean) {
  if (!input.value) return;
  e.preventDefault();
  if (e.code === 'Escape') {
    pause();
    return;
  }
  if (e.repeat || e.altKey || e.metaKey) return;
  const allowed =
    /^(Key[A-Z]|Digit[0-9]|Space|Enter|Backspace|Tab|Delete|Arrow(Left|Right|Up|Down)|Home|End|Shift(Left|Right)|Control(Left|Right)|Period|Comma|Slash|Semicolon|Quote|Minus|Equal)$/;
  if (allowed.test(e.code)) send({ kind: 'key', key: e.code, down });
}
function wheel(e: WheelEvent) {
  if (input.value) {
    e.preventDefault();
    send({ kind: 'wheel', delta: e.deltaY > 0 ? -120 : 120 });
  }
}
onMounted(() => {
  window.addEventListener('blur', pause);
  document.addEventListener('visibilitychange', pause);
});
onBeforeUnmount(() => {
  window.removeEventListener('blur', pause);
  document.removeEventListener('visibilitychange', pause);
  void stop();
});
</script>
<template>
  <section class="product-card remote-workspace" aria-label="局域网远程会话">
    <div class="card-header">
      <h2>局域网远程会话</h2>
      <button class="outline-button" :disabled="!busy" @click="stop">
        停止并回收
      </button>
    </div>
    <p role="status">
      {{ labels[state] ?? '正在读取状态' }} · 本次最长30秒 · 主屏720p30
    </p>
    <p v-if="busy && role === 'sender'" class="remote-warning">
      本机主屏正在共享或等待共享，并已批准本次实际键鼠控制。可点“停止并回收”、关闭窗口，或在本机按
      Ctrl+Alt+F12 紧急停止。
    </p>
    <template v-if="!busy">
      <div class="remote-fields">
        <label
          >本机角色<select v-model="role">
            <option value="receiver">控制端：观看并操作另一台电脑</option>
            <option value="sender">被控端：共享本机主屏</option>
          </select></label
        >
        <label
          >另一台的局域网IPv4<input
            v-model="peer"
            autocomplete="off"
            spellcheck="false"
            placeholder="填写另一台电脑地址"
        /></label>
        <button class="outline-button" @click="detect">检测本机地址</button>
        <label
          >本机IPv4<input v-model="local" autocomplete="off" spellcheck="false"
        /></label>
        <label v-if="role === 'receiver'"
          >被控端本次配对码<input
            v-model="code"
            type="password"
            autocomplete="off"
            spellcheck="false"
        /></label>
      </div>
      <label
        ><input
          v-model="consent"
          type="checkbox"
        />我同意本次指定电脑的主屏视频和实际鼠标、基本键盘控制；不授予无人值守或管理员权限。</label
      >
      <p>
        <button
          class="outline-button"
          :disabled="
            !consent || !local || !peer || (role === 'receiver' && !code)
          "
          @click="start"
        >
          {{ role === 'sender' ? '确认后等待配对' : '确认后连接' }}
        </button>
      </p>
    </template>
    <p v-if="invitation">
      本次配对码（只交给你操作的控制端）：<code class="remote-code">{{
        invitation
      }}</code>
    </p>
    <p v-if="error" role="alert">{{ error }}</p>
    <p v-if="busy && role === 'receiver'">
      <button
        class="outline-button"
        :disabled="state !== 'playing'"
        @click="input ? pause() : activate()"
      >
        {{ input ? '暂停键鼠控制' : '启用本次键鼠控制' }}
      </button>
      {{
        input
          ? '当前可操作画面；Esc或失焦暂停并释放按键'
          : '键鼠控制暂停。看到画面后主动启用。'
      }}
    </p>
    <div
      ref="surface"
      class="remote-surface"
      tabindex="0"
      aria-label="远程主屏操作区域"
      @blur="pause"
      @keydown="key($event, true)"
      @keyup="key($event, false)"
      @mousemove="move"
      @mousedown="button($event, true)"
      @mouseup="button($event, false)"
      @mouseleave="pause"
      @wheel="wheel"
      @contextmenu.prevent
    >
      <video
        ref="video"
        autoplay
        muted
        playsinline
        :class="{ ready: role === 'receiver' && busy }"
      />
      <span v-if="role === 'sender' || !busy">{{
        role === 'sender'
          ? '被控端不显示本机桌面预览'
          : '连接后在此显示另一台主屏'
      }}</span>
    </div>
    <p>
      视频解码 {{ receiver?.framesDecoded ?? '未取得' }} · 呈现
      {{ receiver?.framesRendered ?? '未取得' }} · 对端校验
      {{ receiver?.peerMatches === true ? '通过' : '尚未通过' }} · 输入发送/确认
      {{
        receiver?.control
          ? `${receiver.control.sentMoves}/${receiver.control.ackReceived}`
          : '未取得'
      }}
      · Rust接收/确认
      {{
        native?.source?.control
          ? `${native.source.control.accepted_moves}/${native.source.control.acks_sent}`
          : '未取得'
      }}
    </p>
    <p>
      资源回收：{{
        native?.resources_released
          ? '已完成'
          : busy
            ? '会话进行中'
            : '尚无本次结果'
      }}。未接入公网、云身份或持久信任库；每轮都要重新确认和配对。
    </p>
  </section>
</template>
<style scoped>
.remote-workspace {
  margin: 16px 0;
}
.remote-fields {
  display: flex;
  flex-wrap: wrap;
  gap: 12px;
  align-items: end;
  margin-bottom: 12px;
}
.remote-fields label {
  display: grid;
  gap: 6px;
}
.remote-fields input,
.remote-fields select {
  padding: 8px;
  border: 1px solid #ccd5d1;
  border-radius: 6px;
  background: white;
  max-width: 100%;
}
.remote-warning {
  padding: 12px;
  background: #fff0d0;
  color: #684600;
}
.remote-code {
  user-select: text;
  overflow-wrap: anywhere;
}
.remote-surface {
  position: relative;
  aspect-ratio: 16/9;
  background: #12201c;
  color: white;
  display: grid;
  place-items: center;
  outline-offset: 3px;
  max-height: 64vh;
}
.remote-surface video {
  width: 100%;
  height: 100%;
  object-fit: contain;
  display: none;
}
.remote-surface video.ready {
  display: block;
}
.remote-surface span {
  position: absolute;
}
.remote-workspace p {
  overflow-wrap: anywhere;
}
</style>
