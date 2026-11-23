import { invoke } from '@tauri-apps/api/core';
import { flushPromises, mount } from '@vue/test-utils';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import DevicesPage from './pages/DevicesPage.vue';
import V2PeerPairing from './V2PeerPairing.vue';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const localCard = {
  card: `RH2-${'ab'.repeat(32)}`,
  fingerprint: 'dc'.repeat(32),
};
const peerCard = `RH2-${'14'.repeat(32)}`;
const peerFingerprint = 'f9'.repeat(32);
const prefillKey = 'remotehub.v2.peer-form-prefill.v1';
const runtime = {
  app_version: 'fixture',
  os: 'windows',
  arch: 'x64',
  capture_backend: 'fixture',
  remote_access_enabled: false,
};
const originalClipboard = Object.getOwnPropertyDescriptor(
  navigator,
  'clipboard',
);

beforeEach(() => {
  vi.resetAllMocks();
  window.localStorage.removeItem(prefillKey);
});
afterEach(() => {
  if (originalClipboard)
    Object.defineProperty(navigator, 'clipboard', originalClipboard);
  else Reflect.deleteProperty(navigator, 'clipboard');
});

it('does not render or read the pairing card until the native feature is enabled and explicitly requested', async () => {
  const view = mount(DevicesPage, { props: { loading: false, runtime } });
  expect(view.findComponent(V2PeerPairing).exists()).toBe(false);
  await view.setProps({
    runtime: { ...runtime, v2_peer_pairing_enabled: true },
  });
  expect(view.findComponent(V2PeerPairing).exists()).toBe(true);
  expect(invoke).not.toHaveBeenCalled();
  view.unmount();
});

it('displays complete local public values and copies only the explicitly selected text', async () => {
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, 'clipboard', {
    configurable: true,
    value: { writeText },
  });
  vi.mocked(invoke).mockResolvedValue(localCard);
  const view = mount(V2PeerPairing);
  await view.get('button').trigger('click');
  await flushPromises();
  expect(invoke).toHaveBeenCalledExactlyOnceWith('get_v2_pairing_card');
  expect(view.text()).toContain(localCard.card);
  expect(view.text()).toContain(localCard.fingerprint);
  expect(writeText).not.toHaveBeenCalled();
  await view
    .findAll('button')
    .find((button) => button.text() === '复制指纹')!
    .trigger('click');
  expect(writeText).toHaveBeenCalledExactlyOnceWith(localCard.fingerprint);
  view.unmount();
});

it('requires separately entered full fingerprint, fresh confirmation, and exact matched response', async () => {
  vi.mocked(invoke).mockResolvedValue({ fingerprint: peerFingerprint });
  const view = mount(V2PeerPairing);
  const cardInput = view.get('#v2-peer-card');
  const fpInput = view.get('#v2-peer-fingerprint');
  const consent = view.get('input[type="checkbox"]');
  const form = view.get('form');
  await cardInput.setValue(peerCard);
  await fpInput.setValue(peerFingerprint.slice(0, 12));
  await form.trigger('submit');
  expect(invoke).not.toHaveBeenCalled();
  await fpInput.setValue(peerFingerprint.toUpperCase());
  await form.trigger('submit');
  expect(invoke).not.toHaveBeenCalled();
  await consent.setValue(true);
  await cardInput.setValue(peerCard + ' ');
  await form.trigger('submit');
  expect(invoke).not.toHaveBeenCalled();
  await consent.setValue(true);
  await form.trigger('submit');
  await flushPromises();
  expect(invoke).toHaveBeenCalledExactlyOnceWith('verify_v2_peer', {
    card: peerCard,
    fingerprint: peerFingerprint,
    consent: true,
  });
  expect(view.get('[role="status"]').text()).toContain(peerFingerprint);
  expect(view.get('[role="status"]').text()).toContain('尚未连接或授权远控');
  await fpInput.setValue('');
  expect(view.find('[role="status"]').exists()).toBe(false);
  view.unmount();
});

it('prefills only after explicit save and never restores consent or verification', async () => {
  vi.mocked(invoke).mockResolvedValue({ fingerprint: peerFingerprint });
  const first = mount(V2PeerPairing);
  const save = () =>
    first.findAll('button').find((b) => b.text() === '下次预填此设备');
  await first.get('#v2-peer-card').setValue(peerCard);
  await first.get('#v2-peer-fingerprint').setValue(peerFingerprint);
  expect(save()).toBeUndefined();
  await first.get('input[type="checkbox"]').setValue(true);
  await first.get('form').trigger('submit');
  await flushPromises();
  expect(window.localStorage.getItem(prefillKey)).toBeNull();
  await save()!.trigger('click');
  expect(JSON.parse(window.localStorage.getItem(prefillKey)!)).toEqual({
    card: peerCard,
    fingerprint: peerFingerprint,
  });
  first.unmount();
  vi.mocked(invoke).mockClear();

  const again = mount(V2PeerPairing);
  expect((again.get('#v2-peer-card').element as HTMLInputElement).value).toBe(
    peerCard,
  );
  expect(
    (again.get('#v2-peer-fingerprint').element as HTMLInputElement).value,
  ).toBe(peerFingerprint);
  expect(
    (again.get('input[type="checkbox"]').element as HTMLInputElement).checked,
  ).toBe(false);
  expect(again.find('[role="status"]').exists()).toBe(false);
  await again.get('form').trigger('submit');
  expect(invoke).not.toHaveBeenCalled();
  await again.get('input[type="checkbox"]').setValue(true);
  await again.get('form').trigger('submit');
  await flushPromises();
  expect(invoke).toHaveBeenCalledExactlyOnceWith('verify_v2_peer', {
    card: peerCard,
    fingerprint: peerFingerprint,
    consent: true,
  });
  await again
    .findAll('button')
    .find((b) => b.text() === '清除预填信息')!
    .trigger('click');
  expect(window.localStorage.getItem(prefillKey)).toBeNull();
  expect(again.get('[role="status"]').text()).toContain(peerFingerprint);
  again.unmount();

  const clean = mount(V2PeerPairing);
  expect((clean.get('#v2-peer-card').element as HTMLInputElement).value).toBe(
    '',
  );
  clean.unmount();
});

it('ignores a cached record with unexpected fields and requires manual entry', () => {
  window.localStorage.setItem(
    prefillKey,
    JSON.stringify({
      card: peerCard,
      fingerprint: peerFingerprint,
      verified: true,
    }),
  );
  const view = mount(V2PeerPairing);
  expect((view.get('#v2-peer-card').element as HTMLInputElement).value).toBe(
    '',
  );
  expect(
    view.get('button[type="submit"]').attributes('disabled'),
  ).toBeDefined();
  expect(invoke).not.toHaveBeenCalled();
  view.unmount();
});

it('prefills a role-bound bundle without restoring consent or persisting it', async () => {
  const view = mount(DevicesPage, {
    props: {
      loading: false,
      runtime: {
        ...runtime,
        v2_peer_pairing_enabled: true,
        v2_peer_prefill: { card: peerCard, fingerprint: peerFingerprint },
      },
    },
  });
  expect((view.get('#v2-peer-card').element as HTMLInputElement).value).toBe(
    peerCard,
  );
  expect(
    (view.get('#v2-peer-fingerprint').element as HTMLInputElement).value,
  ).toBe(peerFingerprint);
  await view.get('form').trigger('submit');
  expect(
    view.get('button[type="submit"]').attributes('disabled'),
  ).toBeDefined();
  expect(window.localStorage.getItem(prefillKey)).toBeNull();
  expect(invoke).not.toHaveBeenCalled();
  view.unmount();
});

it.each([
  [() => Promise.resolve({ fingerprint: '23'.repeat(32) }), '配对操作未完成'],
  [
    () =>
      Promise.resolve({
        fingerprint: peerFingerprint,
        private: 'secret fixture',
      }),
    '配对操作未完成',
  ],
  [() => Promise.reject('secret fixture'), '配对操作未完成'],
  [
    () => Promise.reject('V2_PAIRING_FINGERPRINT_MISMATCH'),
    '配对卡与完整指纹不匹配',
  ],
] as const)(
  'fails closed and does not expose raw native payload #%#',
  async (response, expected) => {
    vi.mocked(invoke).mockImplementation(() => response());
    const view = mount(V2PeerPairing);
    await view.get('#v2-peer-card').setValue(peerCard);
    await view.get('#v2-peer-fingerprint').setValue(peerFingerprint);
    await view.get('input[type="checkbox"]').setValue(true);
    await view.get('form').trigger('submit');
    await flushPromises();
    expect(view.get('[role="alert"]').text()).toContain(expected);
    expect(view.text()).not.toContain('secret fixture');
    expect(view.find('[role="status"]').exists()).toBe(false);
    view.unmount();
  },
);
