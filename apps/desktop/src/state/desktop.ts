import { defineStore } from 'pinia';
import { computed, inject, onScopeDispose, ref, shallowRef } from 'vue';
import {
  decodeRuntimeInfo,
  ipcFailure,
  type DesktopIpcError,
  type DesktopApi,
  type RuntimeInfo,
} from '../ipc/contracts';
import { desktopApiKey } from './desktopState';

export type ReadStatus =
  'idle' | 'loading' | 'refreshing' | 'ready' | 'error' | 'cancelled';
export const useDesktopStore = defineStore('desktop', () => {
  const injectedApi = inject(desktopApiKey);
  if (!injectedApi) throw new Error('DESKTOP_STATE_NOT_INSTALLED');
  const api: DesktopApi = injectedApi;
  const runtime = shallowRef<RuntimeInfo>();
  const status = ref<ReadStatus>('idle');
  const readKind = ref<'initial' | 'refresh' | 'retry'>('initial');
  const error = shallowRef<DesktopIpcError>();
  const loading = computed(
    () => status.value === 'loading' || status.value === 'refreshing',
  );
  const errorMessage = computed(() => error.value?.message ?? '');
  const feedback = computed(() => {
    switch (status.value) {
      case 'loading':
        return '正在读取本机信息，请稍候。';
      case 'refreshing':
        return '正在刷新本机信息，等待本次结果。';
      case 'cancelled':
        return '本次读取已取消，未保留旧信息。你可以重新检测。';
      case 'error':
        return errorMessage.value;
      case 'ready':
        return readKind.value === 'retry'
          ? '本机信息已重新读取，可以继续查看。'
          : readKind.value === 'refresh'
            ? '刷新完成，本次信息已更新。'
            : '本机信息已读取。';
      default:
        return '尚未读取本机信息。';
    }
  });
  let generation = 0;
  let pending:
    { controller: AbortController; promise: Promise<void> } | undefined;
  function refresh(): Promise<void> {
    if (pending) return pending.promise;
    const current = ++generation;
    const controller = new AbortController();
    readKind.value =
      status.value === 'ready'
        ? 'refresh'
        : status.value === 'error' || status.value === 'cancelled'
          ? 'retry'
          : 'initial';
    status.value = readKind.value === 'refresh' ? 'refreshing' : 'loading';
    runtime.value = undefined;
    error.value = undefined;
    const promise = Promise.resolve()
      .then(() =>
        current === generation
          ? api.readRuntime({ signal: controller.signal })
          : undefined,
      )
      .then((value) => {
        if (current !== generation) return;
        runtime.value = decodeRuntimeInfo(value);
        status.value = 'ready';
      })
      .catch((cause: unknown) => {
        if (current !== generation) return;
        runtime.value = undefined;
        error.value = ipcFailure(cause);
        status.value =
          error.value.code === 'IPC_CANCELLED' ? 'cancelled' : 'error';
      })
      .finally(() => {
        if (current === generation) pending = undefined;
      });
    pending = { controller, promise };
    return promise;
  }
  function invalidate() {
    ++generation;
    pending?.controller.abort();
    pending = undefined;
    runtime.value = undefined;
    error.value = undefined;
  }
  function cancelRead() {
    if (!pending) return;
    invalidate();
    status.value = 'cancelled';
  }
  function reset() {
    invalidate();
    status.value = 'idle';
    readKind.value = 'initial';
  }
  onScopeDispose(reset);
  return {
    runtime,
    status,
    error,
    loading,
    errorMessage,
    feedback,
    refresh,
    cancelRead,
    reset,
  };
});
