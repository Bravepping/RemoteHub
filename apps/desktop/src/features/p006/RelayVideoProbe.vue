<script setup lang="ts">
import { computed, onMounted, onBeforeUnmount, ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { RelayProbe, type RelayMode } from './relay';
import { evidenceCode } from './evidence';
import type { LanStatus, LanReceiver } from '../p004/lan';
const props = defineProps<{
  directEnabled?: boolean;
  diagnosticOnly?: boolean;
}>();
const mode = ref<RelayMode | ''>(
  props.diagnosticOnly
    ? ''
    : props.directEnabled
      ? 'direct_preferred'
      : 'relay_only',
);
const lastDiagnosticMode = ref<RelayMode>();
const peerIp = ref('');
const video = ref<HTMLVideoElement>();
const role = ref<'sender' | 'receiver'>('sender');
const interfaces = ref<string[]>([]);
const localIp = ref('');
const inputCode = ref('');
const hostCode = ref('');
const consent = ref(false);
const simulateDrop = ref(false);
const dropRequested = ref(false);
const dropAllowed = computed(
  () =>
    props.diagnosticOnly === true &&
    props.directEnabled === true &&
    mode.value === 'direct_preferred' &&
    role.value === 'sender',
);
const busy = ref(false);
const state = ref('idle');
const error = ref('');
const saved = ref('');
const status = ref<LanStatus>();
const receiver = ref<LanReceiver>();
const dropEvidence = computed(() => {
  if (!props.diagnosticOnly || role.value !== 'sender') return undefined;
  const evidence = status.value?.source?.diagnostic_transport;
  return typeof evidence?.drop_direct === 'boolean' ? evidence : undefined;
});
const nativePath = computed(() => {
  const path = status.value?.source?.native_path;
  return path?.path === 'direct' || path?.path === 'relay'
    ? path.path
    : undefined;
});
let probe: RelayProbe | undefined;
let unlistenClose: UnlistenFn | undefined;
watch(mode, () => {
  if (!props.diagnosticOnly) return;
  consent.value = false;
  if (mode.value !== 'direct_preferred') peerIp.value = '';
});
watch(
  dropAllowed,
  () => {
    simulateDrop.value = false;
    consent.value = false;
  },
  { flush: 'sync' },
);
watch(
  simulateDrop,
  () => {
    consent.value = false;
  },
  { flush: 'sync' },
);
function count(value: number | undefined) {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0
    ? value
    : '未获得';
}
function modeLabel(selected: RelayMode | '') {
  if (selected === 'direct_preferred') return '优先直连，需要时自动中继';
  if (selected === 'relay_only') return '两端强制 TURN';
  return '尚未选择连接方式';
}
function showError(error: string) {
  if (!props.diagnosticOnly) return error;
  const code = error.replace(/^Error:\s*/, '');
  if (code === 'R06_REQUESTED_MODE_MISMATCH')
    return '两端返回的诊断方式与本次选择不符，已停止且未切换模式。';
  if (code === 'R06_EXPLICIT_MODE_REQUIRED')
    return '请明确选择受支持的连接方式，且诊断固定为 8 秒。';
  return evidenceCode(error);
}
const labels: Record<string, string> = {
  idle: '尚未开始',
  waiting: '等待另一台电脑配对',
  waiting_peer: '等待另一台电脑配对',
  negotiating: '正在检查可用连接路径',
  connected: '已连接，等待画面',
  playing: '正在实时播放',
  stopping: '正在停止与回收',
  stopped: '已停止',
  failed: '实验失败',
};
onMounted(async () => {
  try {
    unlistenClose = await listen('p006-close-requested', () => {
      void probe?.stop();
    });
    interfaces.value = await invoke<string[]>('p006_interfaces');
    localIp.value =
      interfaces.value.find((ip) => ip.startsWith('192.168.')) ??
      interfaces.value[0] ??
      '';
  } catch (e) {
    error.value = showError(String(e));
  }
});
async function start() {
  const picked = mode.value;
  if (
    !video.value ||
    busy.value ||
    !consent.value ||
    !localIp.value ||
    !picked ||
    (picked === 'direct_preferred' && !props.directEnabled)
  )
    return;
  busy.value = true;
  const dropDirect = dropAllowed.value && simulateDrop.value;
  dropRequested.value = dropDirect;
  error.value = '';
  saved.value = '';
  status.value = undefined;
  receiver.value = undefined;
  state.value = 'starting';
  if (props.diagnosticOnly) lastDiagnosticMode.value = picked;
  probe = new RelayProbe(
    video.value,
    (s, n, r) => {
      state.value = s;
      if (n) status.value = n;
      if (r) receiver.value = r;
      if (['stopped', 'failed'].includes(s)) {
        busy.value = false;
        if (props.diagnosticOnly) mode.value = '';
      }
    },
    (e) => {
      if (!error.value) error.value = showError(e);
    },
    (c) => (hostCode.value = c),
    (p) => (saved.value = p),
    props.directEnabled ?? false,
  );
  const config = {
    role: role.value,
    consent: consent.value,
    localIp: localIp.value,
    seconds: props.diagnosticOnly ? 8 : 30,
    pairingCode: inputCode.value,
    mode: picked,
    ...(dropDirect ? { diagnosticDropDirect: true as const } : {}),
    peerIp:
      picked === 'direct_preferred' && peerIp.value.trim()
        ? peerIp.value.trim()
        : undefined,
  };
  inputCode.value = '';
  consent.value = false;
  simulateDrop.value = false;
  await probe.start(config, props.diagnosticOnly === true);
}
onBeforeUnmount(() => {
  unlistenClose?.();
  void probe?.stop();
});
</script>
<template>
  <section class="relay-panel">
    <h2>
      {{
        props.diagnosticOnly
          ? 'R06 · 连接定向诊断（仅合成图案）'
          : props.directEnabled
            ? 'P0-06c · 直连优先双机实验'
            : 'P0-06b · 跨网络 TURN 双机实验'
      }}
    </h2>
    <p>
      {{ modeLabel(mode) }}
      · 720p30 · {{ props.diagnosticOnly ? '最多8秒，可提前停止' : '30秒' }} ·
      最多100条模拟事件 · 不执行键鼠输入
    </p>
    <p v-if="props.diagnosticOnly">
      本构建只生成图案，不采集桌面、摄像头或麦克风。需等待另行授权并取得新配置后使用；每轮双方亲自确认。
    </p>
    <p v-if="props.diagnosticOnly">
      两端须为本轮选择同一种方式。优先直连可按该模式的规则经 TURN
      中继；强制中继不会尝试直连。
    </p>
    <p v-if="props.diagnosticOnly && lastDiagnosticMode && !busy && !mode">
      上轮选择：{{ modeLabel(lastDiagnosticMode) }}
    </p>
    <div class="relay-form">
      <label v-if="props.diagnosticOnly">
        本轮诊断方式
        <select v-model="mode" aria-label="本轮诊断方式" :disabled="busy">
          <option value="" disabled>请选择，不预选连接方式</option>
          <option value="direct_preferred" :disabled="!props.directEnabled">
            优先直连，允许中继
          </option>
          <option value="relay_only">强制 TURN 中继（对照）</option>
        </select>
      </label>
      <label v-if="props.directEnabled && !props.diagnosticOnly"
        >连接方式<select v-model="mode" :disabled="busy">
          <option value="direct_preferred">优先直连，自动中继</option>
          <option value="relay_only">强制中继（对照）</option>
        </select></label
      >
      <label
        >本机角色
        <select v-model="role" :disabled="busy">
          <option value="sender">
            {{ props.diagnosticOnly ? '发送合成图案' : '发送屏幕（宽带电脑）' }}
          </option>
          <option value="receiver">
            {{ props.diagnosticOnly ? '接收合成图案' : '接收画面（热点电脑）' }}
          </option>
        </select></label
      >
      <label
        >本机物理网卡
        <select v-model="localIp" :disabled="busy">
          <option v-for="ip in interfaces" :key="ip">{{ ip }}</option>
        </select></label
      >
      <label v-if="role === 'receiver'"
        >本轮配对码
        <input
          v-model="inputCode"
          :disabled="busy"
          autocomplete="off"
          spellcheck="false"
      /></label>
      <label v-if="mode === 'direct_preferred'"
        >允许直连的对端局域网IPv4<input
          v-model="peerIp"
          :disabled="busy"
          placeholder="同局域网填写，跨网可留空"
          autocomplete="off"
      /></label>
    </div>
    <template v-if="dropAllowed && !busy">
      <label>
        <input
          id="r06-diagnostic-drop-direct"
          v-model="simulateDrop"
          type="checkbox"
        />
        模拟直连不可用（仅本次合成诊断）
      </label>
      <p>
        两端都选“优先直连，允许中继”。只在发送端勾选此项，接收端照常确认本轮诊断。
      </p>
    </template>
    <label
      ><input
        id="p006-consent"
        v-model="consent"
        type="checkbox"
        :disabled="busy || (props.diagnosticOnly && !mode)"
      />
      {{
        props.diagnosticOnly
          ? '我同意本次合成图案连接诊断，明白不采集桌面'
          : '我同意本次视频与模拟事件实验，明白不保存画面'
      }}</label
    >
    <div class="relay-actions">
      <button
        :disabled="
          busy ||
          !consent ||
          !localIp ||
          !mode ||
          (mode === 'direct_preferred' && !props.directEnabled)
        "
        @click="start"
      >
        开始本次实验</button
      ><button :disabled="!busy" @click="probe?.stop()">停止实验</button>
    </div>
    <p v-if="hostCode" class="pair-code">
      本轮配对码（仅输入另一台电脑，不发到聊天）：<br /><code>{{
        hostCode
      }}</code>
    </p>
    <p>
      状态：{{ labels[state] ?? state }} · 资源回收：{{
        status?.resources_released ? '已完成' : '等待实验结束'
      }}
    </p>
    <p>
      浏览器路径：{{ receiver?.localCandidateType ?? '未选择' }} /
      {{ receiver?.remoteCandidateType ?? '未选择' }} ·
      {{
        receiver?.connectionPath === 'direct'
          ? '当前直连'
          : receiver?.connectionPath === 'relay'
            ? '当前中继'
            : '路径待确认'
      }}
      · 路径校验：{{ receiver?.peerMatches === true ? '通过' : '未验证' }}
    </p>
    <p>
      发送事件 / 收到ACK：{{ receiver?.control?.sentMoves ?? 0 }} /
      {{ receiver?.control?.ackReceived ?? 0 }} · 未确认 / 异常：{{
        receiver?.control?.pending ?? 0
      }}
      / {{ receiver?.control?.anomalies ?? 0 }}
    </p>
    <p>
      源发送 / 接收解码：{{ status?.source?.frames_sent ?? 0 }} /
      {{ receiver?.framesDecoded ?? 0 }} · 画面：{{
        receiver?.videoWidth ?? 0
      }}
      × {{ receiver?.videoHeight ?? 0 }}
    </p>
    <p v-if="error" class="relay-error">{{ error }}</p>
    <p v-if="props.diagnosticOnly">
      原生发送端候选校验：本地
      {{ status?.source?.direct_pair_diagnostic?.local.reason ?? '未获得' }} /
      远端
      {{ status?.source?.direct_pair_diagnostic?.remote.reason ?? '未获得' }}。
      独立最终统计会在停止时保存；缺失不会显示为通过。
    </p>
    <p v-if="props.diagnosticOnly && dropRequested && busy">
      本轮已请求模拟直连不可用，是否生效以原生统计为准。
    </p>
    <p v-if="dropEvidence" aria-label="原生直连模拟统计">
      原生报告：模拟直连不可用{{
        dropEvidence.drop_direct ? '已启用' : '未启用'
      }}； 拦截直连 ICE 检查 {{ count(dropEvidence.dropped_direct_checks) }}；
      拦截数据包总数 {{ count(dropEvidence.dropped_packets) }}。
      此统计不表示中继已通过。
    </p>
    <p v-if="props.diagnosticOnly && nativePath">
      发送端实际路径：{{ nativePath === 'direct' ? '直连' : '中继' }}。
    </p>
    <p v-if="saved">统计已保存：{{ saved }}</p>
    <video ref="video" autoplay muted playsinline></video>
  </section>
</template>
<style scoped>
.relay-panel {
  margin: 20px 0;
  padding: 22px;
  background: #111b2e;
  border-radius: 16px;
  color: #dce7f7;
}
.relay-form {
  display: flex;
  gap: 16px;
  flex-wrap: wrap;
  margin: 16px 0;
}
.relay-form label {
  display: grid;
  gap: 6px;
}
select,
input:not([type='checkbox']) {
  background: #202e45;
  color: #fff;
  border: 1px solid #53627b;
  padding: 8px;
  border-radius: 6px;
}
.relay-actions {
  display: flex;
  gap: 12px;
  margin: 16px 0;
}
button {
  padding: 10px 16px;
  background: #3579de;
  border-radius: 8px;
}
button:disabled {
  opacity: 0.4;
}
.pair-code {
  overflow-wrap: anywhere;
  background: #1c2c40;
  padding: 12px;
}
.relay-error {
  color: #ffacaa;
}
video {
  width: 100%;
  max-height: 320px;
  background: #060b13;
  border-radius: 8px;
}
</style>
