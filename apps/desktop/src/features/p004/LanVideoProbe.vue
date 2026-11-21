<script setup lang="ts">
import { onBeforeUnmount, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import {
  LanProbe,
  type LanStatus,
  type LanReceiver,
  type LanConfig,
} from './lan';
const video = ref<HTMLVideoElement>();
const role = ref<'sender' | 'receiver'>('sender');
const peerIp = ref('');
const localIp = ref('');
const seconds = ref(30);
const port = ref(48604);
const consent = ref(false);
const inputCode = ref('');
const hostCode = ref('');
const busy = ref(false);
const state = ref('idle');
const error = ref('');
const report = ref('');
const native = ref<LanStatus>();
const receiver = ref<LanReceiver>();
let probe: LanProbe | undefined;
const labels: Record<string, string> = {
  idle: '尚未开始',
  starting: '等待本次确认',
  waiting: '等待接收端配对（最多 120 秒）',
  negotiating: '正在建立指定对端连接',
  connected: '已连接，等待真实画面',
  playing: '正在实时播放',
  finishing: '正在收集末帧统计',
  stopping: '正在停止与回收资源',
  stopped: '已停止',
  failed: '实验失败',
};
async function detect() {
  error.value = '';
  try {
    localIp.value = await invoke<string>('p004_lan_local_ip', {
      peerIp: peerIp.value.trim(),
    });
  } catch (e) {
    error.value = String(e);
  }
}
async function start() {
  if (!video.value || busy.value) return;
  busy.value = true;
  error.value = '';
  report.value = '';
  native.value = undefined;
  receiver.value = undefined;
  hostCode.value = '';
  const config: LanConfig = {
    role: role.value,
    consent: consent.value,
    localIp: localIp.value.trim(),
    peerIp: peerIp.value.trim(),
    seconds: seconds.value,
    port: port.value,
    pairingCode: inputCode.value,
  };
  probe = new LanProbe(
    video.value,
    (next, n, r) => {
      state.value = next;
      if (n) native.value = n;
      if (r) receiver.value = r;
      if (next === 'stopped' || next === 'failed') busy.value = false;
    },
    (e) => {
      if (!error.value) error.value = e;
    },
    (c) => {
      hostCode.value = c;
    },
    (p) => {
      report.value = p;
    },
  );
  inputCode.value = '';
  consent.value = false;
  await probe.start(config);
}
async function stop() {
  await probe?.stop();
  consent.value = false;
}
onBeforeUnmount(() => {
  void probe?.stop();
});
</script>
<template>
  <section class="lan">
    <div class="heading">
      <div>
        <span class="step">P0-04b / 双机视频实验 · v2</span>
        <h2>局域网实时桌面视频</h2>
      </div>
      <span class="badge" role="status">{{ labels[state] ?? state }}</span>
    </div>
    <p>
      发送端采集主屏，接收端播放。每轮两台电脑都需确认，最长 30
      秒，结束后只保存统计。
    </p>
    <div class="fields">
      <label
        >这台电脑的角色<select v-model="role" :disabled="busy">
          <option value="sender">发送端（提供主屏画面）</option>
          <option value="receiver">接收端（观看画面）</option>
        </select></label
      >
      <label
        >另一台电脑的 IP<input
          v-model="peerIp"
          :disabled="busy"
          placeholder="例如 192.168.1.20"
      /></label>
      <button class="secondary" :disabled="busy || !peerIp" @click="detect">
        检测本机 IP
      </button>
      <label
        >本机 IP<input
          v-model="localIp"
          :disabled="busy"
          placeholder="先点击检测"
      /></label>
      <label
        >时长<select v-model="seconds" :disabled="busy">
          <option :value="10">10 秒</option>
          <option :value="15">15 秒</option>
          <option :value="30">30 秒</option>
        </select></label
      >
      <label v-if="role === 'receiver'"
        >本轮配对码<input
          v-model="inputCode"
          type="password"
          :disabled="busy"
          placeholder="输入发送端显示的配对码"
          autocomplete="off"
      /></label>
    </div>
    <div v-if="hostCode" class="pairing">
      <strong>本轮配对码（仅交给接收端）</strong
      ><input :value="hostCode" readonly aria-label="发送端本轮配对码" />
      <p>请在另一台电脑输入；120 秒内有效，不要发到聊天中。</p>
    </div>
    <video
      v-show="role === 'receiver'"
      ref="video"
      autoplay
      muted
      playsinline
      aria-label="远端实时桌面视频"
    ></video>
    <div class="controls">
      <label class="consent"
        ><input
          v-model="consent"
          type="checkbox"
          :disabled="busy"
        />我同意本次与指定电脑的视频实验</label
      ><button
        :disabled="
          busy ||
          !consent ||
          !localIp ||
          !peerIp ||
          (role === 'receiver' && !inputCode)
        "
        @click="start"
      >
        {{ role === 'sender' ? '等待接收端连接' : '连接并接收视频' }}</button
      ><button
        class="secondary"
        :disabled="!busy || state === 'starting' || state === 'stopping'"
        @click="stop"
      >
        停止实验
      </button>
    </div>
    <div v-if="error" class="error" role="alert">{{ error }}</div>
    <dl>
      <dt>本轮配对</dt>
      <dd>{{ native?.authenticated ? '已认证指定对端' : '尚未认证' }}</dd>
      <dt>输入 / 编码 / 发送帧</dt>
      <dd>
        {{
          native?.source
            ? `${native.source.stream.frames_input} / ${native.source.stream.frames_encoded} / ${native.source.frames_sent}`
            : '未测'
        }}
      </dd>
      <dt>对端接收 / 解码 / 呈现帧</dt>
      <dd>
        {{
          receiver
            ? `${receiver.framesReceived ?? '未测'} / ${receiver.framesDecoded ?? '未测'} / ${receiver.framesRendered ?? '未测'}`
            : '未测'
        }}
      </dd>
      <dt>实际视频尺寸</dt>
      <dd>
        {{
          receiver?.videoWidth
            ? `${receiver.videoWidth} × ${receiver.videoHeight}`
            : '未测'
        }}
      </dd>
      <dt>接收包 / 丢包</dt>
      <dd>
        {{
          receiver
            ? `${receiver.packetsReceived ?? '未测'} / ${receiver.packetsLost ?? '未测'}`
            : '未测'
        }}
      </dd>
      <dt>指定对端检查</dt>
      <dd>
        {{ receiver?.peerMatches === true ? '实际选中指定对端 UDP' : '未测' }}
      </dd>
      <dt>媒体播放时间</dt>
      <dd>{{ receiver ? `${receiver.currentTime.toFixed(2)} 秒` : '未测' }}</dd>
      <dt>资源回收</dt>
      <dd>
        {{
          native
            ? native.resources_released
              ? '已完成'
              : '实验运行中'
            : '未测'
        }}
      </dd>
    </dl>
    <p v-if="report" class="note">本轮统计：{{ report }}</p>
    <p class="note">
      两台电脑使用相同时长。连接端口
      {{ port }}。本实验只验证视频，不包含键鼠操作、文件传输或公网连接。
    </p>
  </section>
</template>
<style scoped>
.lan {
  border: 1px solid #dbe4ef;
  border-radius: 16px;
  background: white;
  padding: 22px;
}
.heading,
.controls {
  display: flex;
  gap: 14px;
  align-items: center;
  justify-content: space-between;
}
.controls {
  flex-wrap: wrap;
  justify-content: flex-start;
}
.fields {
  display: flex;
  gap: 12px;
  align-items: end;
  flex-wrap: wrap;
  margin: 18px 0;
}
.fields label {
  display: flex;
  flex-direction: column;
  gap: 6px;
  font-size: 13px;
}
.fields input,
.fields select,
.pairing input {
  border: 1px solid #cbd5e1;
  border-radius: 6px;
  padding: 8px;
  min-width: 140px;
}
.pairing {
  background: #edf5ff;
  padding: 14px;
  border-radius: 10px;
  margin-bottom: 14px;
}
.pairing input {
  display: block;
  font-family: monospace;
  font-size: 18px;
  width: 400px;
  max-width: 100%;
  margin-top: 8px;
}
.pairing p {
  font-size: 13px;
}
video {
  display: block;
  width: 100%;
  max-height: 350px;
  aspect-ratio: 16/9;
  background: #101b30;
  object-fit: contain;
  border-radius: 10px;
  margin: 16px 0;
}
dl {
  grid-template-columns: 190px 1fr;
  font-variant-numeric: tabular-nums;
}
.consent {
  margin: 0;
}
</style>
