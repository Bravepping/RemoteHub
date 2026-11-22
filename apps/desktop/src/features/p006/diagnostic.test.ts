// Offline component fixtures; not a native consent or physical two-PC test.
import { flushPromises, mount } from '@vue/test-utils';
import { beforeEach, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import RelayVideoProbe from './RelayVideoProbe.vue';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => () => {}),
}));
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === 'p006_interfaces') return ['192.168.1.2'];
    throw new Error('OFFLINE_FIXTURE_STOP');
  });
});
it('diagnostic panel is synthetic-only, fresh-consent and never silently starts', async () => {
  const w = mount(RelayVideoProbe, {
    props: { directEnabled: true, diagnosticOnly: true },
  });
  await flushPromises();
  expect(w.text()).toContain('不采集桌面');
  expect(w.text()).toContain('最多8秒');
  expect(w.text()).not.toContain('发送屏幕');
  expect(w.findAll('select')).toHaveLength(2); // No relay-only/fault/source switch.
  expect(w.findAll('button')[0].attributes('disabled')).toBeDefined();
  expect(invoke).not.toHaveBeenCalledWith('p006_start', expect.anything());
  await w.get('input[type=checkbox]').setValue(true);
  await w.findAll('button')[0].trigger('click');
  await flushPromises();
  expect(invoke).toHaveBeenCalledWith('p006_start', {
    config: expect.objectContaining({
      consent: true,
      seconds: 8,
      mode: 'direct_preferred',
      role: 'sender',
    }),
  });
  expect(
    (w.get('input[type=checkbox]').element as HTMLInputElement).checked,
  ).toBe(false);
  w.unmount();
});
it('ordinary P006 UI keeps its desktop/30-second scope', async () => {
  const w = mount(RelayVideoProbe, { props: { directEnabled: true } });
  await flushPromises();
  expect(w.text()).toContain('发送屏幕');
  expect(w.text()).toContain('30秒');
  expect(w.text()).not.toContain('R06 ·');
  w.unmount();
});
