import { createApp } from 'vue';
import { disposePinia } from 'pinia';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { createDesktopState } from './desktopState';
import { useDesktopStore } from './desktop';
import {
  DesktopIpcError,
  ipcFailure,
  type DesktopApi,
  type RuntimeInfo,
} from '../ipc/contracts';
const fixture: RuntimeInfo = {
  app_version: 'feedback-fixture',
  os: 'fixture-os',
  arch: 'fixture-arch',
  capture_backend: 'fixture-backend',
  remote_access_enabled: false,
};
const states: ReturnType<typeof createDesktopState>[] = [];
function setup(api: DesktopApi) {
  const state = createDesktopState(api);
  states.push(state);
  const app = createApp({});
  app.use(state);
  return app.runWithContext(() => useDesktopStore(state.pinia));
}
afterEach(() => {
  for (const s of states.splice(0)) disposePinia(s.pinia);
});
describe('P1-05 truthful read feedback', () => {
  it('clears previous data while refreshing and reports completion only on new success', async () => {
    let done!: (v: RuntimeInfo) => void;
    const api = {
      readRuntime: vi
        .fn()
        .mockResolvedValueOnce(fixture)
        .mockImplementationOnce(
          () =>
            new Promise<RuntimeInfo>((resolve) => {
              done = resolve;
            }),
        ),
    };
    const store = setup(api);
    await store.refresh();
    expect(store.feedback).toBe('本机信息已读取。');
    const task = store.refresh();
    expect(store.status).toBe('refreshing');
    expect(store.loading).toBe(true);
    expect(store.runtime).toBeUndefined();
    expect(store.feedback).toContain('等待本次结果');
    await Promise.resolve();
    done({ ...fixture, app_version: 'fresh-fixture' });
    await task;
    expect(store.feedback).toBe('刷新完成，本次信息已更新。');
    expect(store.runtime?.app_version).toBe('fresh-fixture');
  });
  it('shows cancellation, ignores late results and permits a recovery read', async () => {
    let done!: (v: RuntimeInfo) => void;
    const api = {
      readRuntime: vi
        .fn()
        .mockImplementationOnce(
          () =>
            new Promise<RuntimeInfo>((resolve) => {
              done = resolve;
            }),
        )
        .mockResolvedValueOnce(fixture),
    };
    const store = setup(api);
    const old = store.refresh();
    await Promise.resolve();
    store.cancelRead();
    expect(store.status).toBe('cancelled');
    expect(store.feedback).toContain('读取已取消');
    done(fixture);
    await old;
    expect(store.runtime).toBeUndefined();
    await store.refresh();
    expect(store.feedback).toBe('本机信息已重新读取，可以继续查看。');
  });
  it('reset is silent and invalidates a pending read without a cancellation alert', async () => {
    let done!: (v: RuntimeInfo) => void;
    const store = setup({
      readRuntime: () =>
        new Promise<RuntimeInfo>((resolve) => {
          done = resolve;
        }),
    });
    const old = store.refresh();
    await Promise.resolve();
    store.reset();
    done(fixture);
    await old;
    expect(store.status).toBe('idle');
    expect(store.errorMessage).toBe('');
    expect(store.runtime).toBeUndefined();
  });
  it('does not cancel an already completed read', async () => {
    const store = setup({ readRuntime: async () => fixture });
    await store.refresh();
    store.cancelRead();
    expect(store.status).toBe('ready');
    expect(store.runtime).toEqual(fixture);
  });
  it.each(['IPC_TIMEOUT', 'IPC_FAILED', 'IPC_RESPONSE_INVALID'] as const)(
    'recovers from %s without exposing stale or private values',
    async (code) => {
      const api = {
        readRuntime: vi
          .fn()
          .mockResolvedValueOnce(fixture)
          .mockRejectedValueOnce(new DesktopIpcError(code))
          .mockResolvedValueOnce(fixture),
      };
      const store = setup(api);
      await store.refresh();
      await store.refresh();
      expect(store.status).toBe('error');
      expect(store.runtime).toBeUndefined();
      expect(store.error?.code).toBe(code);
      expect(store.feedback).not.toContain('浏览器');
      await store.refresh();
      expect(store.feedback).toContain('已重新读取');
    },
  );
  it('separates unavailable browser from native failure and ignores overridden exception messages', () => {
    expect(new DesktopIpcError('BROWSER_UNAVAILABLE').message).toContain(
      '浏览器预览',
    );
    for (const code of [
      'IPC_TIMEOUT',
      'IPC_FAILED',
      'IPC_RESPONSE_INVALID',
      'IPC_CANCELLED',
    ] as const) {
      const error = new DesktopIpcError(code);
      error.message = 'sensitive-endpoint-and-credential-fixture';
      expect(ipcFailure(error).message).not.toMatch(/sensitive|fixture|浏览器/);
    }
    expect(
      ipcFailure({ code: 'IPC_FAILED', message: 'sensitive-fixture' }).message,
    ).not.toContain('sensitive');
  });
});
