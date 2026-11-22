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

beforeEach(() => vi.resetAllMocks());
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
