<script setup lang="ts">
import { onMounted, onBeforeUnmount, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { RelayProbe } from './relay';
import type { LanStatus, LanReceiver } from '../p004/lan';
const props = defineProps<{ directEnabled?: boolean }>();
const mode = ref<'relay_only' | 'direct_preferred'>(
  props.directEnabled ? 'direct_preferred' : 'relay_only',
);
const peerIp = ref('');
const video = ref<HTMLVideoElement>();
const role = ref<'sender' | 'receiver'>('sender');
const interfaces = ref<string[]>([]);
const localIp = ref('');
const inputCode = ref('');
const hostCode = ref('');
const consent = ref(false);
const busy = ref(false);
const state = ref('idle');
const error = ref('');
const saved = ref('');
const status = ref<LanStatus>();
const receiver = ref<LanReceiver>();
let probe: RelayProbe | undefined;
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
    interfaces.value = await invoke<string[]>('p006_interfaces');
    localIp.value =
      interfaces.value.find((ip) => ip.startsWith('192.168.')) ??
      interfaces.value[0] ??
      '';
  } catch (e) {
    error.value = String(e);
  }
});
async function start() {
  if (!video.value || busy.value) return;
  busy.value = true;
  error.value = '';
  saved.value = '';
  status.value = undefined;
  receiver.value = undefined;
  state.value = 'starting';
  probe = new RelayProbe(
    video.value,
    (s, n, r) => {
      state.value = s;
      if (n) status.value = n;
      if (r) receiver.value = r;
      if (['stopped', 'failed'].includes(s)) busy.value = false;
    },
    (e) => {
      if (!error.value) error.value = e;
    },
    (c) => (hostCode.value = c),
    (p) => (saved.value = p),
    props.directEnabled ?? false,
  );
  const config = {
    role: role.value,
    consent: consent.value,
    localIp: localIp.value,
    seconds: 30,
    pairingCode: inputCode.value,
    mode: mode.value,
    peerIp:
      mode.value === 'direct_preferred' && peerIp.value.trim()
        ? peerIp.value.trim()
        : undefined,
  };
  inputCode.value = '';
  consent.value = false;
  await probe.start(config);
}
onBeforeUnmount(() => void probe?.stop());
</script>
<template>
  <section class="relay-panel">
    <h2>
      {{
        props.directEnabled
          ? 'P0-06c · 直连优先双机实验'
          : 'P0-06b · 跨网络 TURN 双机实验'
      }}
    </h2>
    <p>
      {{
        mode === 'direct_preferred'
          ? '优先直连，需要时自动中继'
          : '两端强制 TURN'
      }}
      · 720p30 · 30秒 · 100条模拟事件 · 不执行键鼠输入
    </p>
    <div class="relay-form">
      <label v-if="props.directEnabled"
        >连接方式<select v-model="mode" :disabled="busy">
          <option value="direct_preferred">优先直连，自动中继</option>
          <option value="relay_only">强制中继（对照）</option>
        </select></label
      >
      <label
        >本机角色
        <select v-model="role" :disabled="busy">
          <option value="sender">发送屏幕（宽带电脑）</option>
          <option value="receiver">接收画面（热点电脑）</option>
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
    <label
      ><input v-model="consent" type="checkbox" :disabled="busy" />
      我同意本次视频与模拟事件实验，明白不保存画面</label
    >
    <div class="relay-actions">
      <button :disabled="busy || !consent || !localIp" @click="start">
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
