<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core';
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import { CanvasVideoAdapter } from './canvasVideoAdapter';
import {
  PublicVideoSession,
  type PublicVideoRole,
  type PublicVideoState,
} from './publicVideoSession';

const canvas = ref<HTMLCanvasElement>();
const role = ref<PublicVideoRole>();
const state = ref<PublicVideoState>('idle');
const error = ref('');
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
const canStart = computed(
  () =>
    state.value === 'idle' ||
    state.value === 'stopped' ||
    state.value === 'failed',
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
    (next, code) => {
      state.value = next;
      error.value = code ?? '';
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
