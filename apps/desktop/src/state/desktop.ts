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

export const useDesktopStore = defineStore('desktop', () => {
  const injectedApi = inject(desktopApiKey);
  if (!injectedApi) throw new Error('DESKTOP_STATE_NOT_INSTALLED');
  const api: DesktopApi = injectedApi;
  const runtime = shallowRef<RuntimeInfo>();
  const status = ref<'idle' | 'loading' | 'ready' | 'error'>('idle');
  const error = shallowRef<DesktopIpcError>();
  const loading = computed(() => status.value === 'loading');
  const errorMessage = computed(() => error.value?.message ?? '');
  let generation = 0;
  let pending:
    { controller: AbortController; promise: Promise<void> } | undefined;
  function refresh(): Promise<void> {
    if (pending) return pending.promise;
    const current = ++generation;
    const controller = new AbortController();
    status.value = 'loading';
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
        status.value = 'error';
      })
      .finally(() => {
        if (current === generation) pending = undefined;
      });
    pending = { controller, promise };
    return promise;
  }
  function cancelRead() {
    ++generation;
    pending?.controller.abort();
    pending = undefined;
    runtime.value = undefined;
    error.value = undefined;
    status.value = 'idle';
  }
  onScopeDispose(cancelRead);
  return { runtime, status, error, loading, errorMessage, refresh, cancelRead };
});
