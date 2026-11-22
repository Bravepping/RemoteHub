import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { tauriDesktopApi, IPC_READ_TIMEOUT_MS } from './tauriDesktopApi';
import { decodeRuntimeInfo, DesktopIpcError, ipcFailure } from './contracts';
import { getRuntimeInfo } from '../bridge';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(), isTauri: vi.fn() }));
const fixture = {
  app_version: 'fixture-version',
  os: 'fixture-os',
  arch: 'fixture-arch',
  capture_backend: 'fixture-backend',
  remote_access_enabled: false,
};
describe('typed read-only Tauri IPC (SDK transport is a test fixture)', () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(isTauri).mockReturnValue(true);
  });
  afterEach(() => vi.useRealTimers());
  it('calls only the existing argument-free command and decodes a real-shaped response', async () => {
    vi.mocked(invoke).mockResolvedValue({
      ...fixture,
      p004_enabled: false,
      v2_loopback_diagnostic_enabled: true,
      v2_peer_pairing_enabled: true,
      v2_verified_lan_enabled: true,
      p007b_diag_enabled: false,
    });
    const result = await tauriDesktopApi.readRuntime();
    expect(invoke).toHaveBeenCalledExactlyOnceWith('get_runtime_info');
    expect(result).toEqual({
      ...fixture,
      p004_enabled: false,
      v2_loopback_diagnostic_enabled: true,
      v2_peer_pairing_enabled: true,
      v2_verified_lan_enabled: true,
      p007b_diag_enabled: false,
    });
    expect(Object.isFrozen(result)).toBe(true);
  });
  it('keeps legacy getRuntimeInfo on the same validated read path', async () => {
    vi.mocked(invoke).mockResolvedValue(fixture);
    expect(await getRuntimeInfo()).toEqual(fixture);
    expect(invoke).toHaveBeenCalledExactlyOnceWith('get_runtime_info');
  });
  it('rejects browser use without invoking native IPC', async () => {
    vi.mocked(isTauri).mockReturnValue(false);
    await expect(tauriDesktopApi.readRuntime()).rejects.toMatchObject({
      code: 'BROWSER_UNAVAILABLE',
    });
    expect(invoke).not.toHaveBeenCalled();
  });
  it.each([
    null,
    [],
    'private-payload-fixture',
    {},
    { ...fixture, os: 42 },
    { ...fixture, arch: '' },
    { ...fixture, app_version: 'v'.repeat(65) },
    { ...fixture, capture_backend: 'b'.repeat(193) },
    { ...fixture, os: 'fixture\nprivate' },
    { ...fixture, remote_access_enabled: 'false' },
    { ...fixture, p004_enabled: 'true' },
    { ...fixture, v2_loopback_diagnostic_enabled: 'true' },
    { ...fixture, v2_peer_pairing_enabled: 'true' },
    { ...fixture, v2_verified_lan_enabled: 'true' },
    { ...fixture, unexpected_private_field: 'private-fixture' },
    Object.create(fixture),
  ])(
    'rejects malformed runtime response #%# without echoing its payload',
    async (value) => {
      vi.mocked(invoke).mockResolvedValue(value);
      await expect(tauriDesktopApi.readRuntime()).rejects.toMatchObject({
        code: 'IPC_RESPONSE_INVALID',
      });
    },
  );
  it('keeps absent optional flags absent and copies the decoded object', () => {
    const result = decodeRuntimeInfo(fixture);
    expect('p004_enabled' in result).toBe(false);
    expect(result).not.toBe(fixture);
  });
  it('redacts arbitrary native failures and reconstitutes known error messages', async () => {
    vi.mocked(invoke).mockRejectedValue(
      new Error('private-native-error-fixture'),
    );
    await expect(tauriDesktopApi.readRuntime()).rejects.toMatchObject({
      code: 'IPC_FAILED',
    });
    const cause = new DesktopIpcError('IPC_TIMEOUT');
    cause.message = 'private-error-fixture';
    expect(ipcFailure(cause).message).not.toContain('private');
    expect(ipcFailure('private').message).not.toContain('private');
  });
  it('times out and ignores a late native response without leaving a timer', async () => {
    vi.useFakeTimers();
    let resolve!: (value: unknown) => void;
    vi.mocked(invoke).mockReturnValue(
      new Promise((done) => {
        resolve = done;
      }),
    );
    const result = tauriDesktopApi.readRuntime();
    const rejected = expect(result).rejects.toMatchObject({
      code: 'IPC_TIMEOUT',
    });
    await vi.advanceTimersByTimeAsync(IPC_READ_TIMEOUT_MS);
    await rejected;
    resolve(fixture);
    await Promise.resolve();
    expect(vi.getTimerCount()).toBe(0);
  });
  it('cancels a pending read and removes its abort listener', async () => {
    vi.useFakeTimers();
    const controller = new AbortController();
    const removed = vi.spyOn(controller.signal, 'removeEventListener');
    let resolve!: (value: unknown) => void;
    vi.mocked(invoke).mockReturnValue(
      new Promise((done) => {
        resolve = done;
      }),
    );
    const result = tauriDesktopApi.readRuntime({ signal: controller.signal });
    const rejected = expect(result).rejects.toMatchObject({
      code: 'IPC_CANCELLED',
    });
    await Promise.resolve();
    controller.abort();
    await rejected;
    expect(removed).toHaveBeenCalledWith('abort', expect.any(Function));
    expect(vi.getTimerCount()).toBe(0);
    resolve(fixture);
    await Promise.resolve();
  });
  it('does not invoke after an already aborted or immediately aborted request', async () => {
    const controller = new AbortController();
    controller.abort();
    await expect(
      tauriDesktopApi.readRuntime({ signal: controller.signal }),
    ).rejects.toMatchObject({ code: 'IPC_CANCELLED' });
    const next = new AbortController();
    const result = tauriDesktopApi.readRuntime({ signal: next.signal });
    const rejected = expect(result).rejects.toMatchObject({
      code: 'IPC_CANCELLED',
    });
    next.abort();
    await rejected;
    expect(invoke).not.toHaveBeenCalled();
  });
});
