<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { CanvasVideoAdapter } from './canvasVideoAdapter';
import {
  observerSnapshot,
  type WindowAttachment,
  type WindowState,
} from './publicSessionWindow';
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
  invitationPicker?: boolean;
  mainObserver?: boolean;
  windowOwner?: WindowAttachment;
}>();
const canvas = ref<HTMLCanvasElement>();
const surface = ref<HTMLDivElement>();
const role = ref<PublicVideoRole | undefined>(props.windowOwner?.role);
const state = ref<PublicVideoState>('idle');
const error = ref('');
const endDetail = ref<PublicVideoEndDetail>();
const consumed = ref(false);
interface Invitation {
  id: string;
  role: PublicVideoRole;
  expires_at: number;
  round: number | null;
}
const invitations = ref<Invitation[]>([]);
const selectedId = ref('');
const nowSeconds = ref(Math.floor(Date.now() / 1000));
// The public control client requires at least 60 seconds on its TURN lease at start.
const availableRounds = computed(() =>
  invitations.value.filter((item) => item.expires_at - nowSeconds.value >= 60),
);
const selected = computed(() =>
  availableRounds.value.find((item) => item.id === selectedId.value),
);
let expiryTimer: ReturnType<typeof setInterval> | undefined;
watch(invitations, (list) => {
  clearInterval(expiryTimer);
  nowSeconds.value = Math.floor(Date.now() / 1000);
  if (list.length)
    expiryTimer = setInterval(() => {
      nowSeconds.value = Math.floor(Date.now() / 1000);
      if (!availableRounds.value.length) clearInterval(expiryTimer);
    }, 1000);
});
const archiveOpen = ref(false);
function chooseRound(id: string) {
  selectedId.value = id;
  role.value = selected.value?.role;
}
const selecting = ref(false);
const selectionError = ref('');
function validInvitation(value: unknown): value is Invitation {
  if (!value || typeof value !== 'object') return false;
  const item = value as Partial<Invitation>;
  return (
    Object.keys(item).sort().join(',') === 'expires_at,id,role,round' &&
    (item.round === null ||
      (typeof item.round === 'number' &&
        Number.isInteger(item.round) &&
        item.round >= 1 &&
        item.round <= 8)) &&
    typeof item.id === 'string' &&
    /^[0-9a-f]{32}$/.test(item.id) &&
    (item.role === 'host' || item.role === 'viewer') &&
    typeof item.expires_at === 'number' &&
    Number.isSafeInteger(item.expires_at) &&
    item.expires_at > 0 &&
    item.expires_at <= 8640000000000
  );
}
async function selectInvitation() {
  if (!props.invitationPicker || !canChoose.value) return;
  selecting.value = true;
  selectionError.value = '';
  try {
    const result: unknown = await invoke('select_control_invitation');
    if (result === null) return;
    if (
      !Array.isArray(result) ||
      !result.length ||
      result.length > 8 ||
      !result.every(validInvitation) ||
      new Set(result.map((item) => item.id)).size !== result.length ||
      new Set(result.map((item) => item.round)).size !== result.length ||
      result.some(
        (item) =>
          item.role !== result[0].role ||
          (item.round === null && result.length !== 1),
      )
    )
      throw new Error();
    nowSeconds.value = Math.floor(Date.now() / 1000);
    invitations.value = result;
    archiveOpen.value = result[0].round !== null;
    chooseRound(archiveOpen.value ? '' : result[0].id);
    state.value = 'idle';
    endDetail.value = undefined;
    roundSeconds.value = undefined;
    error.value = '';
  } catch (cause) {
    invitations.value = [];
    archiveOpen.value = false;
    chooseRound('');
    selectionError.value =
      cause === 'PUBLIC_VIDEO_PROFILE_EXPIRED'
        ? '邀请已过期、尚未开放或剩余时间不足，请核对本轮有效时间。'
        : cause === 'PUBLIC_VIDEO_PROFILE_CONSUMED'
          ? '本机已使用这些轮次，请打开新的邀请。'
          : '无法打开这份邀请，请选择本机的共享端或观看端 ZIP，也可选择单轮 JSON。';
  } finally {
    selecting.value = false;
  }
}
const profileUnconfirmed = ref(false);
const roundSeconds = ref<PublicControlRoundSeconds>();
const connectionPath = ref<'direct' | 'relay'>();
const inputState = ref<PublicInputState>('view_only');
const inputEnabled = ref(false);
const releasingInput = ref(false);
const armingInput = ref(false);
let inputPauseGeneration = 0;
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
const observed = ref<WindowState>();
const opening = ref(false);
const centerOpen = ref(false);
const measuredResolution = ref<string>();
let unlistenPause: UnlistenFn | undefined;
let alive = true;
let observerTimer: ReturnType<typeof setTimeout> | undefined;
const observedActive = computed(
  () => observed.value !== undefined && observed.value.phase !== 'closed',
);

async function pollOwnedWindow() {
  try {
    const raw = await invoke('public_session_window_snapshot');
    if (alive) observed.value = observerSnapshot(raw);
  } catch {
    // Report no live facts when the native snapshot cannot be checked.
    if (alive) observed.value = undefined;
  } finally {
    if (alive) observerTimer = setTimeout(pollOwnedWindow, 350);
  }
}
async function ownerAction(
  command: 'public_session_window_stop' | 'public_session_window_focus',
) {
  try {
    await invoke(command);
  } catch {
    if (alive) selectionError.value = '未能确认远控窗口操作，请检查窗口状态。';
  }
}
async function openOwnedWindow() {
  const choice = selected.value;
  if (!choice || opening.value) return;
  opening.value = true;
  selectionError.value = '';
  try {
    const result = await invoke('public_session_window_open', {
      role: choice.role,
      invitationId: choice.id,
    });
    if (result !== 'opened' && result !== 'existing') throw new Error();
    if (result === 'opened') {
      invitations.value = invitations.value.filter(
        (item) => item.id !== choice.id,
      );
      selectedId.value = '';
    }
  } catch {
    if (alive)
      selectionError.value = '未能打开远控窗口，请检查邀请或现有会话。';
  } finally {
    if (alive) opening.value = false;
  }
}
function toggleCenter() {
  if (!centerOpen.value) pauseInput();
  centerOpen.value = !centerOpen.value;
}

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
const canChoose = computed(
  () =>
    props.available &&
    !selecting.value &&
    !opening.value &&
    !observedActive.value &&
    (state.value === 'idle' ||
      state.value === 'stopped' ||
      state.value === 'failed'),
);
const canStart = computed(
  () =>
    canChoose.value &&
    (props.invitationPicker
      ? selected.value !== undefined
      : !consumed.value && !profileUnconfirmed.value),
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
    !releasingInput.value &&
    !centerOpen.value,
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
  inputPauseGeneration++;
  session?.pauseInput();
}

async function toggleInput() {
  if (inputEnabled.value) return pauseInput();
  if (!canEnableInput.value || armingInput.value) return;
  const generation = props.windowOwner?.generation;
  if (generation === undefined) {
    if (session?.enableInput()) {
      lastMove = -Infinity;
      surface.value?.focus();
      if (document.activeElement !== surface.value) pauseInput();
    }
    return;
  }

  surface.value?.focus();
  if (document.activeElement !== surface.value) return;
  const pauseGeneration = inputPauseGeneration;
  armingInput.value = true;
  try {
    await invoke('public_session_window_arm_input', { generation });
    if (
      alive &&
      pauseGeneration === inputPauseGeneration &&
      canEnableInput.value &&
      document.activeElement === surface.value &&
      session?.enableInput()
    ) {
      lastMove = -Infinity;
      return;
    }
    // Undo a late arm without forwarding any desktop events.
    await invoke('public_control_send_input', {
      generation,
      event: '{"kind":"release_all"}',
    });
  } catch {
    // A denied arm never enables the local renderer forwarding.
  } finally {
    armingInput.value = false;
  }
}

function inputPointOrPause(e: MouseEvent) {
  const target = surface.value;
  const image = canvas.value;
  if (!inputEnabled.value || !target || !image) return;
  const normalized = normalizedPublicPoint(
    target.getBoundingClientRect(),
    image.width,
    image.height,
    e.clientX,
    e.clientY,
  );
  if (!normalized) pauseInput();
  return normalized;
}

function move(e: MouseEvent) {
  const normalized = inputPointOrPause(e);
  if (!normalized || performance.now() - lastMove < 50) return;
  if (session?.sendInput({ kind: 'mouse_move', ...normalized }))
    lastMove = performance.now();
}

function button(e: MouseEvent, down: boolean) {
  const normalized = inputPointOrPause(e);
  if (
    !normalized ||
    e.button < 0 ||
    e.button > 2 ||
    pressedButtons.has(e.button) === down
  )
    return;
  e.preventDefault();
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
  if (props.mainObserver) {
    void pollOwnedWindow();
    return;
  }
  if (props.windowOwner) {
    void listen('remotehub-session-input-pause', pauseInput)
      .then((unsubscribe) => {
        if (alive) unlistenPause = unsubscribe;
        else unsubscribe();
      })
      .catch(() => {});
  }
  window.addEventListener('keydown', stopShortcut);
  window.addEventListener('blur', pauseInput);
  document.addEventListener('visibilitychange', pauseInput);
  session = new PublicVideoSession(
    {
      start: (role) => {
        if (props.windowOwner)
          return Promise.resolve(props.windowOwner.generation);
        if (!props.invitationPicker)
          return invoke('public_video_start', { role });
        const invitationId = selected.value?.id;
        invitations.value = invitations.value.filter(
          (item) => item.id !== invitationId,
        );
        selectedId.value = '';
        if (!invitationId)
          return Promise.reject('PUBLIC_VIDEO_PROFILE_UNAVAILABLE');
        return invoke('public_video_start', { role, invitationId });
      },
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
        () => {
          const frame = canvas.value;
          if (props.windowOwner && frame && frame.width && frame.height)
            measuredResolution.value = `${frame.width} × ${frame.height}`;
          presented();
        },
      ),
    (next, code, detail) => {
      state.value = next;
      error.value = code ?? '';
      endDetail.value = detail;
    },
    (profile) => {
      if (props.invitationPicker) return;
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
    (path) => {
      connectionPath.value = path;
    },
  );
  if (props.windowOwner) session.start(props.windowOwner.role, true);
});
onBeforeUnmount(() => {
  alive = false;
  unlistenPause?.();
  clearTimeout(observerTimer);
  clearInterval(expiryTimer);
  window.removeEventListener('keydown', stopShortcut);
  window.removeEventListener('blur', pauseInput);
  document.removeEventListener('visibilitychange', pauseInput);
  session?.dispose();
});

function start() {
  if (role.value && canStart.value) {
    roundSeconds.value = undefined;
    if (props.mainObserver) void openOwnedWindow();
    else session?.start(role.value, props.controlAvailable === true);
  }
}
</script>

<template>
  <section
    class="public-video-panel"
    :class="{ 'owned-session-panel': windowOwner }"
    :aria-label="controlAvailable ? '公网远控测试' : '公网仅视频会话'"
  >
    <header v-if="windowOwner" class="owned-session-header">
      <h2>
        远控窗口 · {{ windowOwner.role === 'viewer' ? '观看方' : '共享方' }}
      </h2>
      <button
        type="button"
        :aria-expanded="centerOpen"
        aria-controls="owned-control-center"
        @click="toggleCenter"
      >
        控制中心
      </button>
    </header>
    <h2 v-else>{{ controlAvailable ? '公网远控测试' : '公网仅视频会话' }}</h2>
    <p v-if="!windowOwner && controlAvailable">
      双方须先核验完整指纹并批准视频。键鼠需共享端另行原生批准，只在本轮剩余时间内有效，默认拒绝。不提供滚轮或无人值守控制。
      <span v-if="roundSeconds !== undefined"
        >本轮会话最多 {{ roundSeconds }} 秒。</span
      >
      <span v-else>本轮为短时会话，时限以原生配置为准。</span>
    </p>
    <p v-else-if="!windowOwner">
      使用应用启动时指定的本机配置。请先到「设备」核验对方的完整指纹。两端均须明确批准，不提供鼠标或键盘控制。
    </p>
    <div
      v-if="mainObserver && observedActive"
      class="public-window-observer"
      role="status"
    >
      <strong>{{
        observed?.phase === 'ready'
          ? '已建立远程连接'
          : observed?.phase === 'stopping'
            ? '正在断开远程连接'
            : '正在建立远程连接'
      }}</strong>
      <span v-if="observed?.path">{{
        observed.path === 'direct' ? '直连' : 'TURN 中继'
      }}</span>
      <button type="button" @click="ownerAction('public_session_window_focus')">
        显示远控窗口
      </button>
      <button
        type="button"
        :disabled="observed?.phase === 'stopping'"
        @click="ownerAction('public_session_window_stop')"
      >
        断开连接
      </button>
    </div>
    <div v-if="!windowOwner && invitationPicker">
      <button type="button" :disabled="!canChoose" @click="selectInvitation">
        {{ selecting ? '正在选择邀请…' : '打开邀请 ZIP 或 JSON' }}
      </button>
      <fieldset
        v-if="archiveOpen && availableRounds.length"
        :disabled="!canChoose"
      >
        <legend>
          本机{{ availableRounds[0].role === 'host' ? '共享屏幕' : '观看屏幕' }}
          · 可用轮次
        </legend>
        <label v-for="item in availableRounds" :key="item.id">
          <input
            type="radio"
            name="invitation-round"
            :checked="selectedId === item.id"
            @change="chooseRound(item.id)"
          />
          第 {{ item.round }} 轮
        </label>
      </fieldset>
      <p v-else-if="archiveOpen && canChoose">本包当前没有可用轮次。</p>
      <p v-if="selected">
        本机{{ selected.role === 'host' ? '共享屏幕' : '观看屏幕' }} ·
        邀请有效至 {{ new Date(selected.expires_at * 1000).toLocaleString() }}
      </p>
      <p v-if="selectionError" role="alert">{{ selectionError }}</p>
    </div>
    <fieldset v-else-if="!windowOwner" :disabled="!canStart">
      <legend>本机角色</legend>
      <label><input v-model="role" type="radio" value="host" /> 共享屏幕</label>
      <label
        ><input v-model="role" type="radio" value="viewer" /> 观看屏幕</label
      >
    </fieldset>
    <div class="public-video-actions">
      <button
        v-if="!windowOwner"
        type="button"
        :disabled="!role || !canStart"
        @click="start"
      >
        开始本次会话
      </button>
      <button
        v-if="!mainObserver"
        type="button"
        :disabled="!canStop"
        @click="session?.stop()"
      >
        停止并清屏
      </button>
    </div>
    <p v-if="canStop" aria-live="polite" data-testid="connection-path">
      本次连接：{{
        connectionPath === 'direct'
          ? '直连'
          : connectionPath === 'relay'
            ? 'TURN 中继'
            : '正在确认路径…'
      }}
    </p>
    <p role="status">
      {{
        controlAvailable && state === 'sharing'
          ? '本次视频共享已启用。'
          : invitationPicker && state === 'idle'
            ? selected
              ? '邀请已就绪，请确认后开始。'
              : archiveOpen
                ? '请从可用轮次中选择本次邀请。'
                : '请选择本机的角色包或单轮邀请。'
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
    <p
      v-if="
        !windowOwner &&
        !invitationPicker &&
        consumed &&
        !canStop &&
        state !== 'stopping'
      "
    >
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
      v-if="!mainObserver"
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
    <div
      v-if="windowOwner && state === 'watching' && measuredResolution"
      class="owned-session-metrics"
    >
      播放分辨率 {{ measuredResolution }}
    </div>
    <aside
      v-if="windowOwner && centerOpen"
      id="owned-control-center"
      class="owned-control-center"
      aria-label="控制中心"
    >
      <button type="button" @click="toggleCenter">关闭控制中心</button>
      <h3>窗口与显示</h3>
      <p>画面在远控窗口中显示。</p>
      <h3>输入控制</h3>
      <p>{{ controlText }}</p>
      <p>关闭控制中心不会恢复键鼠。恢复时需明确点击「启用本次键鼠」。</p>
      <h3>连接信息</h3>
      <p v-if="connectionPath">
        {{ connectionPath === 'direct' ? '直连' : 'TURN 中继' }}
      </p>
      <h3>退出远控</h3>
      <button type="button" :disabled="!canStop" @click="session?.stop()">
        断开本次会话
      </button>
    </aside>
  </section>
</template>

<style scoped>
.public-window-observer,
.owned-session-header {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 0.7rem;
}
.owned-session-header {
  justify-content: space-between;
}
.owned-session-panel {
  position: relative;
  min-height: calc(100dvh - 1.5rem);
  color: #edf5ff;
  border-color: #344c65;
}
.owned-session-panel .public-video-surface {
  max-height: calc(100dvh - 13rem);
  margin: 0 auto;
}
.owned-session-panel .public-video-surface canvas {
  max-height: inherit;
}
.owned-control-center {
  position: absolute;
  inset: 2.5rem 0 0 auto;
  width: min(22rem, 90%);
  padding: 1rem;
  background: #142638;
  border-left: 1px solid #344c65;
  z-index: 1;
}
.owned-session-metrics {
  margin-top: 0.35rem;
  font-size: 0.9rem;
}
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
