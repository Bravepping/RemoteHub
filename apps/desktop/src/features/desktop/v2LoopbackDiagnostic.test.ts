import { invoke } from '@tauri-apps/api/core';
import { flushPromises, mount } from '@vue/test-utils';
import { beforeEach, expect, it, vi } from 'vitest';
import RemoteWorkspace from './RemoteWorkspace.vue';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
beforeEach(() => vi.resetAllMocks());

const selector = '[aria-labelledby="v2-diagnostic-heading"]';
const open = () =>
  mount(RemoteWorkspace, { props: { v2LoopbackDiagnosticEnabled: true } });

it('is absent by default and leaves the LAN form usable', () => {
  const view = mount(RemoteWorkspace);
  expect(view.find(selector).exists()).toBe(false);
  expect(view.find('[aria-label="局域网远程会话"]').exists()).toBe(true);
  expect(invoke).not.toHaveBeenCalled();
  view.unmount();
});

it('requires fresh explicit consent, refuses repeats while busy and displays only the validated signal count', async () => {
  let complete!: (result: unknown) => void;
  vi.mocked(invoke).mockReturnValue(
    new Promise((done) => {
      complete = done;
    }),
  );
  const view = open();
  const panel = view.get(selector);
  const lan = view.get('[aria-label="局域网远程会话"]');
  await lan.get('select').setValue('sender');
  const [peer, local, lanConsent] = lan.findAll('input');
  await peer.setValue('10.0.0.2');
  await local.setValue('10.0.0.3');
  await lanConsent.setValue(true);
  const startLan = lan
    .findAll('button')
    .find((b) => b.text() === '确认后等待配对')!;
  expect(startLan.attributes('disabled')).toBeUndefined();
  const consent = panel.get('input');
  const button = panel.get('button');
  expect(button.attributes('disabled')).toBeDefined();
  expect(invoke).not.toHaveBeenCalled();
  await consent.setValue(true);
  await button.trigger('click');
  await button.trigger('click');
  await startLan.trigger('click');
  expect(startLan.attributes('disabled')).toBeDefined();
  expect(invoke).toHaveBeenCalledExactlyOnceWith('run_v2_loopback_diagnostic', {
    consent: true,
  });
  expect(panel.get('[role="status"]').text()).toContain('原生窗口确认');
  complete({ authenticatedSignals: 4 });
  await flushPromises();
  expect(panel.get('[role="status"]').text()).toContain('4 条测试消息');
  expect(panel.get('[role="status"]').text()).toContain(
    '不代表远程视频或键鼠连接通过',
  );
  expect((consent.element as HTMLInputElement).checked).toBe(false);
  expect(button.attributes('disabled')).toBeDefined();
  expect(startLan.attributes('disabled')).toBeUndefined();
  view.unmount();
});

it.each([
  [() => Promise.reject('V2_NATIVE_CONSENT_DENIED'), '没有批准本次诊断'],
  [() => Promise.reject('private-token-fixture'), '本机信令诊断未完成'],
  [
    () =>
      Promise.resolve({
        authenticatedSignals: 4,
        private: 'private-token-fixture',
      }),
    '本机信令诊断未完成',
  ],
  [() => Promise.resolve({ authenticatedSignals: 3 }), '本机信令诊断未完成'],
] as const)(
  'reports only known or generic errors and never echoes the native payload',
  async (answer, message) => {
    vi.mocked(invoke).mockImplementation(() => answer());
    const view = open();
    const panel = view.get(selector);
    await panel.get('input').setValue(true);
    await panel.get('button').trigger('click');
    await flushPromises();
    expect(panel.get('[role="alert"]').text()).toContain(message);
    expect(panel.text()).not.toContain('private-token-fixture');
    expect(panel.find('[role="status"]').exists()).toBe(false);
    view.unmount();
  },
);

it('ignores responses to a view that has been unmounted', async () => {
  let complete!: (result: unknown) => void;
  vi.mocked(invoke).mockReturnValue(
    new Promise((done) => {
      complete = done;
    }),
  );
  const before = open();
  await before.get(selector).get('input').setValue(true);
  await before.get(selector).get('button').trigger('click');
  before.unmount();
  const after = open();
  complete({ authenticatedSignals: 4 });
  await flushPromises();
  expect(after.get(selector).find('[role="status"]').exists()).toBe(false);
  expect(invoke).toHaveBeenCalledTimes(1);
  after.unmount();
});
