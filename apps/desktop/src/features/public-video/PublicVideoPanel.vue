<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core';
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { CanvasVideoAdapter } from './canvasVideoAdapter';
import {
  normalizedPublicPoint,
  validPublicKey,
  type PublicInputState,
} from './publicControlEvent';
import {
  PublicVideoSession,
  type PublicControlRoundSeconds,
  type PublicVideoEndDetail,
  type PublicVideoEndReason,
  type PublicVideoRole,
  type PublicVideoState,
} from './publicVideoSession';

const props = defineProps<{
  available: boolean;
  profileConsumed?: boolean;
  controlAvailable?: boolean;
}>();
const canvas = ref<HTMLCanvasElement>();
const surface = ref<HTMLDivElement>();
const role = ref<PublicVideoRole>();
const state = ref<PublicVideoState>('idle');
const error = ref('');
const endDetail = ref<PublicVideoEndDetail>();
const consumed = ref(false);
const profileUnconfirmed = ref(false);
const roundSeconds = ref<PublicControlRoundSeconds>();
const inputState = ref<PublicInputState>('view_only');
const inputEnabled = ref(false);
const releasingInput = ref(false);
const pressedKeys = new Set<string>();
const pressedButtons = new Set<number>();
let lastMove = -Infinity;
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
const canEnableInput = computed(
  () =>
    props.controlAvailable === true &&
    role.value === 'viewer' &&
    state.value === 'watching' &&
    inputState.value === 'allowed' &&
    !releasingInput.value,
);
const controlText = computed(() => {
  if (inputEnabled.value) return '键鼠已在本机启用。按 Esc 或移出画面可暂停。';
  if (releasingInput.value) return '键鼠已暂停，正在释放本次按键。';
  switch (inputState.value) {
    case 'pending':
      return role.value === 'host'
        ? '等待本机独立的原生键鼠确认，默认选择“否”。'
        : '等待共享端在原生对话框中单独批准键鼠。';
    case 'allowed':
      return role.value === 'host'
        ? '本机已单独批准本次键鼠控制。您仍可随时停止会话。'
        : '共享端已单独批准。看到画面后仍须在本机主动启用。';
    case 'denied':
      return '共享端没有批准键鼠，不能操作。';
    case 'closed':
      return '本次键鼠权限已关闭。';
    default:
      return '键鼠尚未获批，不能操作。';
  }
});

function pauseInput() {
  session?.pauseInput();
}

function toggleInput() {
  if (inputEnabled.value) return pauseInput();
  if (canEnableInput.value && session?.enableInput()) {
    lastMove = -Infinity;
    surface.value?.focus();
    if (document.activeElement !== surface.value) pauseInput();
  }
}

function point(e: MouseEvent) {
  const target = surface.value;
  const image = canvas.value;
  if (!inputEnabled.value || !target || !image) return;
  return normalizedPublicPoint(
    target.getBoundingClientRect(),
    image.width,
    image.height,
    e.clientX,
    e.clientY,
  );
}

function move(e: MouseEvent) {
  if (performance.now() - lastMove < 50) return;
  const normalized = point(e);
  if (normalized && session?.sendInput({ kind: 'mouse_move', ...normalized }))
    lastMove = performance.now();
}

function button(e: MouseEvent, down: boolean) {
  if (
    !inputEnabled.value ||
    e.button < 0 ||
    e.button > 2 ||
    pressedButtons.has(e.button) === down
  )
    return;
  e.preventDefault();
  const normalized = point(e);
  if (!normalized) {
    if (!down) pauseInput();
    return;
  }
  if (
    session?.sendInput({ kind: 'mouse_move', ...normalized }) &&
    session.sendInput({ kind: 'mouse_button', button: e.button, down })
  ) {
    if (down) pressedButtons.add(e.button);
    else pressedButtons.delete(e.button);
    lastMove = performance.now();
  }
}

function key(e: KeyboardEvent, down: boolean) {
  if (!inputEnabled.value || e.code === 'Escape') return;
  e.preventDefault();
  if (
    !validPublicKey(e.code) ||
    pressedKeys.has(e.code) === down ||
    (down && (e.isComposing || e.repeat || e.altKey || e.metaKey))
  )
    return;
  if (session?.sendInput({ kind: 'key', key: e.code, down })) {
    if (down) pressedKeys.add(e.code);
    else pressedKeys.delete(e.code);
  }
}

function stopShortcut(event: KeyboardEvent) {
  if (event.key === 'Escape' && !event.repeat && canStop.value) {
    event.preventDefault();
    if (inputEnabled.value || releasingInput.value) pauseInput();
    else void session?.stop();
  }
}
onMounted(() => {
  window.addEventListener('keydown', stopShortcut);
  window.addEventListener('blur', pauseInput);
  document.addEventListener('visibilitychange', pauseInput);
  session = new PublicVideoSession(
    {
      start: (role) => invoke('public_video_start', { role }),
      status: (generation) => invoke('public_video_status', { generation }),
      nextFrame: (generation) =>
        invoke('public_video_next_frame', { generation }),
      stop: (generation) => invoke('public_video_stop', { generation }),
      sendInput: (generation, event) =>
        invoke('public_control_send_input', { generation, event }),
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
    (next, enabled, releasing) => {
      inputState.value = next;
      inputEnabled.value = enabled;
      releasingInput.value = releasing;
      if (!enabled) {
        pressedKeys.clear();
        pressedButtons.clear();
      }
    },
    (seconds) => {
      roundSeconds.value = seconds;
    },
  );
});
onBeforeUnmount(() => {
  window.removeEventListener('keydown', stopShortcut);
  window.removeEventListener('blur', pauseInput);
  document.removeEventListener('visibilitychange', pauseInput);
  session?.dispose();
});

function start() {
  if (role.value && canStart.value) {
    roundSeconds.value = undefined;
    session?.start(role.value, props.controlAvailable === true);
  }
}
</script>

<template>
  <section
    class="public-video-panel"
    :aria-label="controlAvailable ? '公网远控测试' : '公网仅视频会话'"
  >
    <h2>{{ controlAvailable ? '公网远控测试' : '公网仅视频会话' }}</h2>
    <p v-if="controlAvailable">
      双方须先核验完整指纹并批准视频。键鼠需共享端另行原生批准，只在本轮剩余时间内有效，默认拒绝。不提供滚轮或无人值守控制。
      <span v-if="roundSeconds !== undefined"
        >本轮会话最多 {{ roundSeconds }} 秒。</span
      >
      <span v-else>本轮为短时会话，时限以原生配置为准。</span>
    </p>
    <p v-else>
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
      {{
        controlAvailable && state === 'sharing'
          ? '本次视频共享已启用。'
          : statusText[state]
      }}
      <code v-if="error">{{ error }}</code>
      <span v-if="endDetail?.reason"> · {{ endText[endDetail.reason] }}</span>
      <span v-if="endDetail?.captureMs !== undefined">
        · 本机共享采集 {{ formatSeconds(endDetail.captureMs) }} 秒
      </span>
      <span v-if="endDetail?.playbackMs !== undefined">
        · 本机观看（首帧到清屏）{{ formatSeconds(endDetail.playbackMs) }} 秒
      </span>
    </p>
    <p v-if="consumed && !canStop && state !== 'stopping'">
      本次启动配置已使用，不能重新连接。请用下一轮未使用的配置重新启动应用。
    </p>
    <p v-else-if="profileUnconfirmed">
      无法确认本次启动配置是否仍可用。不会使用旧配置重连。
    </p>
    <div v-if="controlAvailable && canStop">
      <p>{{ controlText }}</p>
      <button
        v-if="role === 'viewer'"
        type="button"
        :disabled="!canEnableInput && !inputEnabled"
        @click="toggleInput"
      >
        {{ inputEnabled ? '暂停键鼠' : '启用本次键鼠' }}
      </button>
    </div>
    <div
      v-show="role === 'viewer'"
      ref="surface"
      class="public-video-surface"
      :tabindex="controlAvailable ? -1 : undefined"
      @blur="pauseInput"
      @mouseleave="pauseInput"
      @mousemove="move"
      @mousedown="button($event, true)"
      @mouseup="button($event, false)"
      @keydown="key($event, true)"
      @keyup="key($event, false)"
      @contextmenu="inputEnabled && $event.preventDefault()"
    >
      <canvas
        ref="canvas"
        width="640"
        height="360"
        aria-label="仅显示已验证的当前会话画面"
      />
    </div>
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
.public-video-surface {
  aspect-ratio: 16 / 9;
  max-width: 100%;
  background: #000;
}
.public-video-surface canvas {
  width: 100%;
  height: 100%;
  object-fit: contain;
}
</style>
