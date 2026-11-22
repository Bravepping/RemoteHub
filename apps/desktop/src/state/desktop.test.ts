import { createApp } from 'vue';
import { disposePinia } from 'pinia';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { createDesktopState } from './desktopState';
import { useDesktopStore } from './desktop';
import {
  DesktopIpcError,
  type DesktopApi,
  type RuntimeInfo,
} from '../ipc/contracts';
const fixture: RuntimeInfo = {
  app_version: 'fixture-version',
  os: 'fixture-os',
  arch: 'fixture-arch',
  capture_backend: 'fixture-backend',
  remote_access_enabled: false,
};
const instances: ReturnType<typeof createDesktopState>[] = [];
function setup(api: DesktopApi) {
  const state = createDesktopState(api);
  instances.push(state);
  const app = createApp({});
  app.use(state);
  const store = app.runWithContext(() => useDesktopStore(state.pinia));
  return { store, state };
}
function deferred() {
  let resolve!: (value: RuntimeInfo) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<RuntimeInfo>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
afterEach(() => {
  for (const s of instances.splice(0)) disposePinia(s.pinia);
});
describe('actual Pinia with explicitly injected test API', () => {
  it('isolates application instances and shares one store inside each instance', async () => {
    const one = setup({ readRuntime: async () => fixture });
    const two = setup({
      readRuntime: async () => ({ ...fixture, app_version: 'second-fixture' }),
    });
    await one.store.refresh();
    await two.store.refresh();
    expect(one.store.runtime?.app_version).toBe('fixture-version');
    expect(two.store.runtime?.app_version).toBe('second-fixture');
    expect(useDesktopStore(one.state.pinia)).toBe(one.store);
  });
  it('coalesces overlapping refreshes without duplicating IPC', async () => {
    const value = deferred();
    const api = { readRuntime: vi.fn(() => value.promise) };
    const { store } = setup(api);
    const one = store.refresh();
    const two = store.refresh();
    await Promise.resolve();
    expect(api.readRuntime).toHaveBeenCalledTimes(1);
    expect(store.loading).toBe(true);
    value.resolve(fixture);
    await Promise.all([one, two]);
    expect(store.status).toBe('ready');
  });
  it('clears old values after failure and can retry through the same API', async () => {
    const api = {
      readRuntime: vi
        .fn()
        .mockResolvedValueOnce(fixture)
        .mockRejectedValueOnce('private-fixture')
        .mockResolvedValueOnce(fixture),
    };
    const { store } = setup(api);
    await store.refresh();
    await store.refresh();
    expect(store.runtime).toBeUndefined();
    expect(store.status).toBe('error');
    expect(store.error?.code).toBe('IPC_FAILED');
    expect(store.errorMessage).not.toContain('private');
    await store.refresh();
    expect(store.status).toBe('ready');
    expect(store.error).toBeUndefined();
  });
  it('does not issue a read cancelled before the scheduling microtask', async () => {
    const api = { readRuntime: vi.fn(async () => fixture) };
    const { store } = setup(api);
    const task = store.refresh();
    store.cancelRead();
    await task;
    expect(api.readRuntime).not.toHaveBeenCalled();
    expect(store.status).toBe('cancelled');
  });
  it('ignores old success after cancel and a new generation even if a test API ignores abort', async () => {
    const old = deferred();
    const next = deferred();
    const api = {
      readRuntime: vi
        .fn()
        .mockReturnValueOnce(old.promise)
        .mockReturnValueOnce(next.promise),
    };
    const { store } = setup(api);
    const first = store.refresh();
    await Promise.resolve();
    store.cancelRead();
    const second = store.refresh();
    await Promise.resolve();
    next.resolve({ ...fixture, app_version: 'new-fixture' });
    await second;
    old.resolve(fixture);
    await first;
    expect(store.runtime?.app_version).toBe('new-fixture');
    expect(store.status).toBe('ready');
  });
  it('ignores an old failure without clearing a new pending read', async () => {
    const old = deferred();
    const next = deferred();
    const api = {
      readRuntime: vi
        .fn()
        .mockReturnValueOnce(old.promise)
        .mockReturnValueOnce(next.promise),
    };
    const { store } = setup(api);
    const first = store.refresh();
    await Promise.resolve();
    store.cancelRead();
    const second = store.refresh();
    await Promise.resolve();
    old.reject('private-old-fixture');
    await first;
    expect(store.loading).toBe(true);
    expect(store.error).toBeUndefined();
    const overlapping = store.refresh();
    expect(api.readRuntime).toHaveBeenCalledTimes(2);
    next.resolve(fixture);
    await Promise.all([second, overlapping]);
    expect(store.status).toBe('ready');
  });
  it('aborts and invalidates pending results when the Pinia scope is disposed', async () => {
    const old = deferred();
    let signal: AbortSignal | undefined;
    const { store, state } = setup({
      readRuntime: (options) => {
        signal = options?.signal;
        return old.promise;
      },
    });
    const task = store.refresh();
    await Promise.resolve();
    disposePinia(state.pinia);
    expect(signal?.aborted).toBe(true);
    old.resolve(fixture);
    await task;
    expect(store.runtime).toBeUndefined();
    expect(store.status).toBe('idle');
  });
  it('keeps browser unavailability explicit and rejects malformed injected data', async () => {
    const api = {
      readRuntime: vi
        .fn()
        .mockRejectedValueOnce(new DesktopIpcError('BROWSER_UNAVAILABLE'))
        .mockResolvedValueOnce({ ...fixture, remote_access_enabled: 'true' }),
    };
    const { store } = setup(api);
    await store.refresh();
    expect(store.error?.code).toBe('BROWSER_UNAVAILABLE');
    expect(store.runtime).toBeUndefined();
    await store.refresh();
    expect(store.error?.code).toBe('IPC_RESPONSE_INVALID');
    expect(store.runtime).toBeUndefined();
  });
});
