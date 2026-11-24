<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core';
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { CanvasVideoAdapter } from './canvasVideoAdapter';
import {
  PublicVideoSession,
  type PublicVideoEndDetail,
  type PublicVideoEndReason,
  type PublicVideoRole,
  type PublicVideoState,
} from './publicVideoSession';

const props = defineProps<{ available: boolean; profileConsumed?: boolean }>();
const canvas = ref<HTMLCanvasElement>();
const role = ref<PublicVideoRole>();
const state = ref<PublicVideoState>('idle');
const error = ref('');
const endDetail = ref<PublicVideoEndDetail>();
const consumed = ref(false);
const profileUnconfirmed = ref(false);
watch(
  () => props.profileConsumed,
  (used) => {
    if (used === true) consumed.value = true;
  },
  { immediate: true, flush: 'sync' },
);
let session: PublicVideoSession | undefined;

const statusText: Record<PublicVideoState, string> = {
  idle: '请选择本机角色。',
  starting: '正在等待本机确认或设备验证。',
  waiting: '设备已验证，正在等待可播放的画面。',
  sharing: '本次仅视频共享已启用。',
  watching: '正在显示本次已验证的画面。',
  stopping: '已清屏，正在停止本次会话。',
  stopped: '本次会话已停止。',
  failed: '会话未能继续，已清屏。',
  cleanup_unconfirmed:
    '已清屏，但尚未确认原生会话停止。重新打开应用前，请先在共享端停止共享。',
};
const endText: Record<PublicVideoEndReason, string> = {
  local_stop: '本机请求停止',
  peer_left: '对端已离开，具体原因未知',
  capture_limit: '共享达到时限',
  capture_ended: '本机采集已结束',
  failed: '会话出错',
};
const formatSeconds = (ms: number) => (ms / 1000).toFixed(1);
const canStart = computed(
  () =>
    props.available &&
    !consumed.value &&
    !profileUnconfirmed.value &&
    (state.value === 'idle' ||
      state.value === 'stopped' ||
      state.value === 'failed'),
);
const canStop = computed(
  () =>
    state.value === 'starting' ||
    state.value === 'waiting' ||
    state.value === 'sharing' ||
    state.value === 'watching',
);

function stopShortcut(event: KeyboardEvent) {
  if (event.key === 'Escape' && canStop.value) {
    event.preventDefault();
    void session?.stop();
  }
}
onMounted(() => {
  window.addEventListener('keydown', stopShortcut);
  session = new PublicVideoSession(
    {
      start: (role) => invoke('public_video_start', { role }),
      status: (generation) => invoke('public_video_status', { generation }),
      nextFrame: (generation) =>
        invoke('public_video_next_frame', { generation }),
      stop: (generation) => invoke('public_video_stop', { generation }),
    },
    (failed, presented) =>
      new CanvasVideoAdapter(
        canvas.value!,
        undefined,
        failed,
        undefined,
        presented,
      ),
    (next, code, detail) => {
      state.value = next;
      error.value = code ?? '';
      endDetail.value = detail;
    },
    (profile) => {
      if (profile === 'consumed') consumed.value = true;
      profileUnconfirmed.value = profile === 'unconfirmed';
    },
  );
});
onBeforeUnmount(() => {
  window.removeEventListener('keydown', stopShortcut);
  session?.dispose();
});

function start() {
  if (role.value && canStart.value) session?.start(role.value);
}
</script>

<template>
  <section class="public-video-panel" aria-label="公网仅视频会话">
    <h2>公网仅视频会话</h2>
    <p>
      使用应用启动时指定的本机配置。请先到「设备」核验对方的完整指纹。两端均须明确批准，不提供鼠标或键盘控制。
    </p>
    <fieldset :disabled="!canStart">
      <legend>本机角色</legend>
      <label><input v-model="role" type="radio" value="host" /> 共享屏幕</label>
      <label
        ><input v-model="role" type="radio" value="viewer" /> 观看屏幕</label
      >
    </fieldset>
    <div class="public-video-actions">
      <button type="button" :disabled="!role || !canStart" @click="start">
        开始本次会话
      </button>
      <button type="button" :disabled="!canStop" @click="session?.stop()">
        停止并清屏
      </button>
    </div>
    <p role="status">
      {{ statusText[state] }} <code v-if="error">{{ error }}</code>
      <span v-if="endDetail?.reason"> · {{ endText[endDetail.reason] }}</span>
      <span v-if="endDetail?.captureMs !== undefined">
        · 本机共享采集 {{ formatSeconds(endDetail.captureMs) }} 秒
      </span>
      <span v-if="endDetail?.playbackMs !== undefined">
        · 本机观看（首帧到清屏）{{ formatSeconds(endDetail.playbackMs) }} 秒
      </span>
    </p>
    <p v-if="consumed && !canStop && state !== 'stopping'">
      本次启动配置已使用，不能重新连接。请为下一次会话准备新的配置并重新启动应用。
    </p>
    <p v-else-if="profileUnconfirmed">
      无法确认本次启动配置是否仍可用。不会使用旧配置重连。
    </p>
    <canvas
      v-show="role === 'viewer'"
      ref="canvas"
      width="640"
      height="360"
      aria-label="仅显示已验证的当前会话画面"
    />
  </section>
</template>

<style scoped>
.public-video-panel {
  padding: 1rem;
  border: 1px solid currentColor;
  border-radius: 0.75rem;
}
.public-video-panel h2,
.public-video-panel p {
  margin: 0 0 0.5rem;
}
fieldset,
.public-video-actions {
  display: flex;
  gap: 0.75rem;
  margin-block: 0.5rem;
}
button {
  padding: 0.4rem 0.6rem;
}
canvas {
  display: block;
  max-width: 100%;
  height: auto;
  background: #000;
  pointer-events: none;
}
</style>
