<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { LanProbe, type LanStatus, type LanReceiver } from '../p004/lan';
const props = defineProps<{
  lanInputQueueTest?: boolean;
  v2LoopbackDiagnosticEnabled?: boolean;
  v2VerifiedLanEnabled?: boolean;
  v2PrivateWssLanEnabled?: boolean;
}>();
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
type LanMode = 'legacy' | 'verified' | 'private-wss';
const mode = ref<LanMode>('legacy');
const activeMode = ref<LanMode>('legacy');
const modeAllowed = computed(
  () =>
    mode.value === 'legacy' ||
    (mode.value === 'verified' && props.v2VerifiedLanEnabled === true) ||
    (mode.value === 'private-wss' && props.v2PrivateWssLanEnabled === true),
);
const displayedMode = computed(() =>
  busy.value ? activeMode.value : mode.value,
);
const videoOnly = computed(() => displayedMode.value !== 'legacy');
const privateWss = computed(() => displayedMode.value === 'private-wss');
const testHostInput = computed(
  () =>
    props.lanInputQueueTest === true &&
    !videoOnly.value &&
    role.value === 'sender',
);
watch(
  [mode, modeAllowed],
  () => {
    consent.value = false;
    code.value = '';
    if (busy.value) return;
    invitation.value = '';
    error.value = '';
    native.value = undefined;
    receiver.value = undefined;
  },
  { flush: 'sync' },
);
watch(
  [role, peer, local],
  () => {
    if (mode.value !== 'legacy') consent.value = false;
  },
  { flush: 'sync' },
);
const diagnosticConsent = ref(false);
const diagnosticBusy = ref(false);
const diagnosticCount = ref<number>();
const diagnosticError = ref('');
const emit = defineEmits<{ sessionState: [state: string] }>();
let session: LanProbe | undefined;
let lastMove = 0;
let disposed = false;
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
const diagnosticMessages: Readonly<Record<string, string>> = {
  V2_NATIVE_CONSENT_DENIED: '没有批准本次诊断，未交换任何信令。',
  V2_BUSY: '本机信令诊断正在运行，请等它完成后重试。',
  V2_TOKEN_REQUIRED: '本机诊断未配置临时令牌。检查启动配置，不要分享令牌。',
  V2_CONNECT_FAILED: '无法连接本机信令服务，请确认服务已运行。',
  V2_TIMEOUT: '本机信令诊断超时，请检查服务后重试。',
  V2_SIGNAL_FAILED: '本机信令交换未通过验证。',
  V2_DISABLED: '当前客户端未启用本机信令诊断。',
};
const verifiedLanErrors: Readonly<Record<string, string>> = {
  LAN_VERIFIED_PEER_REQUIRED:
    '请先在“设备”页核对并验证指定对端的完整指纹，再重新开始。',
  LAN_DEVICE_AUTH_FAILED:
    '未能认证指定对端。请确认两端都选择“已验证设备（仅视频）”，且已互相核对指纹。',
  LAN_VERIFIED_MODE_DISABLED: '当前客户端未启用已验证设备视频模式。',
  LAN_VERIFIED_VIDEO_ONLY: '已验证设备模式只允许主屏视频，不允许键鼠输入。',
  BROWSER_SIGNED_HOST_CANDIDATE_MISSING:
    '对端签名应答没有提供有效的视频地址和端口，本次不会显示画面。',
  BROWSER_SELECTED_OFF_PEER:
    '浏览器实际选中的视频地址或端口与对端签名信息不符，会话已停止。',
  BROWSER_PEER_UNVERIFIED: '视频链路无法继续通过核验，已清屏并停止会话。',
};
const privateWssErrors: Readonly<Record<string, string>> = {
  ...verifiedLanErrors,
  LAN_DEVICE_AUTH_FAILED:
    '未能认证指定对端。请确认双方都选择了私网 WSS 模式，且已互相核对指纹。',
  LAN_WSS_CONFIG_REQUIRED:
    '缺少有效的私网 WSS 启动配置。请按部署说明在两台电脑配置原生客户端后重试，不要在此输入信令地址、令牌或证书。',
  LAN_WSS_MODE_DISABLED: '当前客户端未启用私网 WSS 视频模式。',
  LAN_WSS_AUTH_FAILED:
    '私网 WSS 信令连接或会话认证未通过。请检查两端的原生启动配置、完整指纹和本次配对码。',
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
  if (
    diagnosticBusy.value ||
    busy.value ||
    !modeAllowed.value ||
    !video.value ||
    !consent.value ||
    !local.value.trim() ||
    !peer.value.trim() ||
    (role.value === 'receiver' && !code.value)
  )
    return;
  const selectedMode = mode.value;
  const verified = selectedMode !== 'legacy';
  const wss = selectedMode === 'private-wss';
  const knownErrors = wss ? privateWssErrors : verifiedLanErrors;
  activeMode.value = selectedMode;
  busy.value = true;
  error.value = '';
  invitation.value = '';
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
      if (next !== 'playing' || r?.control?.sendClosed) pause();
      if (next === 'stopped' || next === 'failed') {
        busy.value = false;
      }
    },
    (e) => {
      const errorCode = e.replace(/^Error: /, '');
      error.value =
        props.lanInputQueueTest === true &&
        !verified &&
        errorCode === 'LAN_TEST_INPUT_CONSENT_DENIED'
          ? '本次键鼠控制未获原生批准，会话已中止，不会自动退回仅视频模式。'
          : verified
            ? Object.hasOwn(knownErrors, errorCode)
              ? knownErrors[errorCode]
              : wss
                ? '私网 WSS 视频会话未建立或已中断。请检查本机启动配置、临时配对码和两端的会话设置。没有切换到其他模式。'
                : '已验证设备视频会话未建立或已中断。请检查本机地址、临时配对码和两端的会话设置。'
            : safeError(e);
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
    ...(verified ? { verifiedPeer: true } : {}),
    ...(wss ? { verifiedWss: true } : {}),
    realControl: !verified,
    controlSim: false,
  });
}
async function runDiagnostic() {
  if (
    disposed ||
    props.v2LoopbackDiagnosticEnabled !== true ||
    busy.value ||
    diagnosticBusy.value ||
    !diagnosticConsent.value
  )
    return;
  diagnosticBusy.value = true;
  diagnosticConsent.value = false;
  diagnosticCount.value = undefined;
  diagnosticError.value = '';
  try {
    const result = await invoke<unknown>('run_v2_loopback_diagnostic', {
      consent: true,
    });
    if (disposed) return;
    if (
      !result ||
      typeof result !== 'object' ||
      ![Object.prototype, null].includes(Object.getPrototypeOf(result)) ||
      Object.keys(result).length !== 1 ||
      !Object.hasOwn(result, 'authenticatedSignals') ||
      (result as Record<string, unknown>).authenticatedSignals !== 4
    )
      throw new Error();
    diagnosticCount.value = (
      result as { authenticatedSignals: number }
    ).authenticatedSignals;
  } catch (error) {
    if (!disposed) {
      diagnosticError.value =
        typeof error === 'string' && Object.hasOwn(diagnosticMessages, error)
          ? diagnosticMessages[error]
          : '本机信令诊断未完成。请检查客户端和本机信令服务后重试。';
    }
  } finally {
    if (!disposed) diagnosticBusy.value = false;
  }
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
  if (videoOnly.value) return;
  input.value = session?.enableInput(true) ?? false;
  if (input.value) surface.value?.focus();
}
function send(event: Record<string, unknown>) {
  if (!videoOnly.value && input.value) session?.input(event);
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
  disposed = true;
  window.removeEventListener('blur', pause);
  document.removeEventListener('visibilitychange', pause);
  void stop();
});
</script>
<template>
  <section class="product-card remote-workspace" aria-label="局域网远程会话">
    <div class="card-header">
      <h2>
        局域网远程会话<span v-if="lanInputQueueTest === true"
          >（键鼠队列测试版）</span
        >
      </h2>
      <button class="outline-button" :disabled="!busy" @click="stop">
        停止并回收
      </button>
    </div>
    <p role="status">
      {{ labels[state] ?? '正在读取状态' }} · 本次最长30秒 · 主屏720p30
      <template v-if="privateWss"> · 模式：私网 WSS 信令（仅视频）</template>
      <template v-else-if="videoOnly"> · 模式：已验证设备（仅视频）</template>
    </p>
    <p v-if="busy && role === 'sender'" class="remote-warning">
      <template v-if="videoOnly">
        本机主屏正在共享或等待共享，仅允许观看。可点“停止并回收”或关闭窗口。
      </template>
      <template v-else-if="testHostInput">
        键鼠须在批准视频后，通过另一个默认选择“否”的原生对话框单独批准。拒绝会中止本次会话，不会降级为仅视频。
        可点“停止并回收”、关闭窗口，或在本机按 Ctrl+Alt+F12 紧急停止。
      </template>
      <template v-else>
        本机主屏正在共享或等待共享，并已批准本次实际键鼠控制。可点“停止并回收”、关闭窗口，或在本机按
        Ctrl+Alt+F12 紧急停止。
      </template>
    </p>
    <template v-if="!busy">
      <div class="remote-fields">
        <label
          v-if="
            v2VerifiedLanEnabled === true ||
            v2PrivateWssLanEnabled === true ||
            mode !== 'legacy'
          "
        >
          局域网会话模式
          <select v-model="mode" aria-label="局域网会话模式">
            <option value="legacy">临时配对码（视频与键鼠）</option>
            <option
              v-if="v2VerifiedLanEnabled === true || mode === 'verified'"
              :disabled="v2VerifiedLanEnabled !== true"
              value="verified"
            >
              已验证设备（仅视频）
            </option>
            <option
              v-if="v2PrivateWssLanEnabled === true || mode === 'private-wss'"
              :disabled="v2PrivateWssLanEnabled !== true"
              value="private-wss"
            >
              私网 WSS 信令（仅视频）
            </option>
          </select>
        </label>
        <label
          >本机角色<select v-model="role">
            <option value="receiver">
              {{
                videoOnly
                  ? '观看端：观看另一台主屏'
                  : '控制端：观看并操作另一台电脑'
              }}
            </option>
            <option value="sender">
              {{ videoOnly ? '共享端：共享本机主屏' : '被控端：共享本机主屏' }}
            </option>
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
          >{{ videoOnly ? '共享端本次配对码' : '被控端本次配对码'
          }}<input
            v-model="code"
            type="password"
            autocomplete="off"
            spellcheck="false"
        /></label>
      </div>
      <p v-if="videoOnly">
        两台电脑都需要在“设备”页验证对端，再选择此模式。观看端仍须输入临时配对码，两端都须确认本次视频会话。
      </p>
      <p v-if="privateWss">
        WSS 仅传递信令，视频仍通过两台电脑所选的局域网 IPv4
        传输。请在两台电脑预先配置原生客户端；此模式不会自动切换为普通局域网连接。
      </p>
      <p v-if="!modeAllowed">
        当前启动未启用所选模式。请明确选择其他模式或重新配置客户端。
      </p>
      <p v-if="testHostInput">
        视频获批后，实际键鼠控制另有原生确认，默认选择“否”。拒绝会中止本次会话，不会降级为仅视频。
      </p>
      <label
        ><input v-model="consent" type="checkbox" />{{
          videoOnly
            ? '我同意本次与指定设备建立局域网主屏视频会话，仅观看，不授权键鼠。'
            : testHostInput
              ? '我同意与指定电脑建立本次主屏视频会话。此勾选不授权键鼠控制。'
              : '我同意本次指定电脑的主屏视频和实际鼠标、基本键盘控制；不授予无人值守或管理员权限。'
        }}</label
      >
      <p>
        <button
          class="outline-button"
          :disabled="
            diagnosticBusy ||
            !modeAllowed ||
            !consent ||
            !local ||
            !peer ||
            (role === 'receiver' && !code)
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
    <p v-if="busy && role === 'receiver' && !videoOnly">
      <button
        class="outline-button"
        :disabled="
          state !== 'playing' || receiver?.control?.sendClosed === true
        "
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
      :tabindex="videoOnly ? undefined : 0"
      :aria-label="videoOnly ? '远程主屏视频区域' : '远程主屏操作区域'"
      @blur="pause"
      @keydown="key($event, true)"
      @keyup="key($event, false)"
      @mousemove="move"
      @mousedown="button($event, true)"
      @mouseup="button($event, false)"
      @mouseleave="pause"
      @wheel="wheel"
      @contextmenu="
        (e) => {
          if (!videoOnly) e.preventDefault();
        }
      "
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
      {{ receiver?.peerMatches === true ? '通过' : '尚未通过' }}
      <template v-if="videoOnly">
        · 信令设备身份 {{ native?.authenticated ? '已验证' : '尚未验证' }} ·
        键鼠未启用
      </template>
      <template v-else>
        · 输入发送/确认
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
      </template>
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
  <section
    v-if="v2LoopbackDiagnosticEnabled === true"
    class="product-card remote-workspace"
    aria-labelledby="v2-diagnostic-heading"
  >
    <div class="card-header">
      <h2 id="v2-diagnostic-heading">V2 本机信令诊断</h2>
    </div>
    <p>
      只在本机回环交换测试用 offer、answer 和 ICE
      消息。不会采集屏幕、发送视频、操作键鼠或连接其他设备，也不改变局域网远程会话。
    </p>
    <p>使用已启动的本机信令服务；诊断不会启动服务。</p>
    <label>
      <input
        v-model="diagnosticConsent"
        type="checkbox"
        :disabled="busy || diagnosticBusy"
      />
      我同意执行本次本机信令诊断
    </label>
    <p>
      <button
        class="outline-button"
        :disabled="busy || diagnosticBusy || !diagnosticConsent"
        @click="runDiagnostic"
      >
        {{ diagnosticBusy ? '诊断进行中…' : '运行本机诊断' }}
      </button>
    </p>
    <p v-if="diagnosticBusy" role="status">请在原生窗口确认本次诊断。</p>
    <p v-if="diagnosticCount !== undefined" role="status">
      本机信令诊断通过：已认证并转发
      {{ diagnosticCount }} 条测试消息。此结果不代表远程视频或键鼠连接通过。
    </p>
    <p v-if="diagnosticError" role="alert">{{ diagnosticError }}</p>
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
