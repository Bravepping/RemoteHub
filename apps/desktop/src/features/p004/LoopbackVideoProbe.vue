<script setup lang="ts">
import { onBeforeUnmount, ref } from 'vue';
import {
  LoopbackProbe,
  type NativeStats,
  type ReceiverSnapshot,
} from './webrtc';

const video = ref<HTMLVideoElement>();
const consent = ref(false);
const seconds = ref(15);
const state = ref('idle');
const error = ref('');
const reportPath = ref('');
const native = ref<NativeStats>();
const receiver = ref<ReceiverSnapshot>();
const busy = ref(false);
let probe: LoopbackProbe | undefined;
const labels: Record<string, string> = {
  idle: '尚未开始',
  starting: '等待本次确认',
  negotiating: '正在建立同机连接',
  connected: '已连接，等待真实画面',
  playing: '正在实时播放',
  stopping: '正在停止与回收资源',
  stopped: '已停止',
  failed: '实验失败',
};
async function start() {
  if (!video.value || busy.value) return;
  busy.value = true;
  error.value = '';
  reportPath.value = '';
  native.value = undefined;
  receiver.value = undefined;
  probe = new LoopbackProbe(
    video.value,
    (next, n, r) => {
      state.value = next;
      if (n) native.value = n;
      if (r) receiver.value = r;
      if (next === 'stopped' || next === 'failed') busy.value = false;
    },
    (message) => {
      if (!error.value) error.value = message;
    },
    (path) => {
      reportPath.value = path;
    },
  );
  await probe.start(consent.value, seconds.value);
  consent.value = false;
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
  <section class="loopback">
    <div class="probe-heading">
      <div>
        <span class="step">P0-04a / 同机视频实验</span>
        <h2>实时桌面 → 本机播放</h2>
      </div>
      <span class="badge" role="status">{{ labels[state] ?? state }}</span>
    </div>
    <p>
      仅在这台电脑内传送主屏画面。实验结束自动停止，并保存不含画面的测试统计。
    </p>
    <video
      ref="video"
      autoplay
      playsinline
      muted
      aria-label="本机实时桌面视频"
    ></video>
    <div class="probe-controls">
      <label
        >实验时长
        <select v-model="seconds" :disabled="busy">
          <option :value="5">5 秒</option>
          <option :value="10">10 秒</option>
          <option :value="15">15 秒</option>
          <option :value="30">30 秒</option>
        </select></label
      >
      <label class="consent"
        ><input
          v-model="consent"
          type="checkbox"
          :disabled="busy"
        />我同意本次主屏采集与同机播放</label
      >
      <button :disabled="!consent || busy" @click="start">
        开始本机视频实验
      </button>
      <button
        class="secondary"
        :disabled="!busy || state === 'starting' || state === 'stopping'"
        @click="stop"
      >
        停止实验
      </button>
    </div>
    <div v-if="error" role="alert" class="error">{{ error }}</div>
    <dl class="probe-stats">
      <dt>编码器</dt>
      <dd>{{ native?.stream.encoder || '未测' }}</dd>
      <dt>输入 / 编码 / 发送帧</dt>
      <dd>
        {{
          native
            ? `${native.stream.frames_input} / ${native.stream.frames_encoded} / ${native.frames_sent}`
            : '未测'
        }}
      </dd>
      <dt>接收 / 解码 / 呈现帧</dt>
      <dd>
        {{
          receiver
            ? `${receiver.framesReceived ?? '未测'} / ${receiver.framesDecoded ?? '未测'} / ${receiver.framesRendered ?? '未测'}`
            : '未测'
        }}
      </dd>
      <dt>视频实际尺寸</dt>
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
      <dt>开始到首帧</dt>
      <dd>
        {{
          receiver?.firstFrameMs != null
            ? `${receiver.firstFrameMs.toFixed(0)} ms（含确认等待）`
            : '未测'
        }}
      </dd>
      <dt>媒体播放时间</dt>
      <dd>{{ receiver ? `${receiver.currentTime.toFixed(2)} 秒` : '未测' }}</dd>
      <dt>同机候选检查</dt>
      <dd>
        {{
          receiver?.selectedRemoteLoopback === true
            ? '实际选中 127.0.0.1'
            : '未测'
        }}
      </dd>
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
    <p v-if="reportPath" class="note">本轮统计：{{ reportPath }}</p>
    <p class="note">
      本轮不包含其他电脑连接、键鼠操作或公网传输。P0-04 两机验证仍未完成。
    </p>
  </section>
</template>

<style scoped>
.loopback {
  border: 1px solid #dbe4ef;
  border-radius: 16px;
  background: white;
  padding: 22px;
}
.probe-heading {
  display: flex;
  justify-content: space-between;
  gap: 12px;
  align-items: center;
}
video {
  display: block;
  width: 100%;
  max-height: 380px;
  aspect-ratio: 16/9;
  background: #101b30;
  object-fit: contain;
  border-radius: 10px;
  margin: 18px 0;
}
.probe-controls {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 12px;
}
.probe-controls .consent {
  margin: 0;
}
select {
  border: 1px solid #cbd5e1;
  border-radius: 6px;
  padding: 6px;
}
.probe-stats {
  grid-template-columns: 150px 1fr;
  font-variant-numeric: tabular-nums;
}
</style>
