import { invoke, isTauri } from '@tauri-apps/api/core';
import {
  decodeRuntimeInfo,
  DesktopIpcError,
  type DesktopApi,
} from './contracts';
export const IPC_READ_TIMEOUT_MS = 5000;

function readNative(signal?: AbortSignal): Promise<unknown> {
  return new Promise((resolve, reject) => {
    let done = false;
    function finish(error?: DesktopIpcError, value?: unknown) {
      if (done) return;
      done = true;
      clearTimeout(timer);
      signal?.removeEventListener('abort', abort);
      if (error) reject(error);
      else resolve(value);
    }
    const abort = () => finish(new DesktopIpcError('IPC_CANCELLED'));
    const timer = setTimeout(
      () => finish(new DesktopIpcError('IPC_TIMEOUT')),
      IPC_READ_TIMEOUT_MS,
    );
    signal?.addEventListener('abort', abort, { once: true });
    // Invoke is read-only and cannot be cancelled natively; late responses are observed and ignored.
    Promise.resolve()
      .then(() => {
        if (signal?.aborted) throw new DesktopIpcError('IPC_CANCELLED');
        return invoke<unknown>('get_runtime_info');
      })
      .then(
        (value) => finish(undefined, value),
        () =>
          finish(
            new DesktopIpcError(
              signal?.aborted ? 'IPC_CANCELLED' : 'IPC_FAILED',
            ),
          ),
      );
  });
}
export const tauriDesktopApi: DesktopApi = Object.freeze({
  async readRuntime({ signal }: { signal?: AbortSignal } = {}) {
    if (signal?.aborted) throw new DesktopIpcError('IPC_CANCELLED');
    if (!isTauri()) throw new DesktopIpcError('BROWSER_UNAVAILABLE');
    return decodeRuntimeInfo(await readNative(signal));
  },
});
