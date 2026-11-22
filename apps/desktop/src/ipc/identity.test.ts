import { expect, it } from 'vitest';
import { decodeIdentity, decodeRuntimeInfo } from './contracts';
const ready = {
  status: 'ready',
  device_id: 'RH-0123-4567-89AB',
  fingerprint: '0123456789ab'.repeat(5) + '0123',
};
it('accepts only public frozen identity fields and their consistent identifier', () => {
  const value = decodeIdentity(ready);
  expect(value).toEqual(ready);
  expect(Object.isFrozen(value)).toBe(true);
  expect(
    decodeIdentity({
      status: 'unavailable',
      error_code: 'IDENTITY_UNPROTECT_FAILED',
    }).status,
  ).toBe('unavailable');
});
for (const bad of [
  null,
  [],
  { ...ready, private_key: 'sensitive-fixture' },
  { ...ready, path: 'sensitive-fixture' },
  { ...ready, device_id: 'RH-FFFF-FFFF-FFFF' },
  { ...ready, fingerprint: 'x'.repeat(64) },
  { status: 'unavailable', error_code: 'RAW_SECRET_FIXTURE' },
  {
    status: 'unavailable',
    error_code: 'IDENTITY_BUSY',
    fingerprint: ready.fingerprint,
  },
  Object.create(ready),
]) {
  it('rejects malformed, inconsistent, inherited or sensitive identity fields', () => {
    expect(() => decodeIdentity(bad)).toThrow('本机核心返回的信息无法识别');
  });
}
it('runtime contract keeps identity optional, validates it deeply and rejects private exports', () => {
  const runtime = {
    app_version: 'fixture',
    os: 'fixture',
    arch: 'fixture',
    capture_backend: 'fixture',
    remote_access_enabled: false,
  };
  expect(decodeRuntimeInfo(runtime).identity).toBeUndefined();
  expect(decodeRuntimeInfo({ ...runtime, identity: ready }).identity).toEqual(
    ready,
  );
  expect(() =>
    decodeRuntimeInfo({
      ...runtime,
      identity: { ...ready, secret: 'fixture' },
    }),
  ).toThrow();
});
