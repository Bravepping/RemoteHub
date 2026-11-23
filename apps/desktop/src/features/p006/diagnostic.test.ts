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
it.each(['direct_preferred', 'relay_only'] as const)(
  'diagnostic panel only launches the explicitly chosen synthetic mode %s',
  async (mode) => {
    const w = mount(RelayVideoProbe, {
      props: { directEnabled: true, diagnosticOnly: true },
    });
    await flushPromises();
    expect(w.text()).toContain('不采集桌面');
    expect(w.text()).toContain('最多8秒');
    expect(w.text()).not.toContain('发送屏幕');
    expect(w.findAll('select')).toHaveLength(3); // Mode, role and approved interface.
    const choice = w.get('[aria-label="本轮诊断方式"]');
    expect((choice.element as HTMLSelectElement).value).toBe('');
    expect(w.findAll('button')[0].attributes('disabled')).toBeDefined();
    expect(w.get('input[type=checkbox]').attributes('disabled')).toBeDefined();
    expect(invoke).not.toHaveBeenCalledWith('p006_start', expect.anything());
    await w.findAll('button')[0].trigger('click');
    expect(invoke).not.toHaveBeenCalledWith('p006_start', expect.anything());
    await choice.setValue(
      mode === 'relay_only' ? 'direct_preferred' : 'relay_only',
    );
    await w.get('input[type=checkbox]').setValue(true);
    await choice.setValue(mode);
    expect(
      (w.get('input[type=checkbox]').element as HTMLInputElement).checked,
    ).toBe(false);
    await w.get('input[type=checkbox]').setValue(true);
    await w.findAll('button')[0].trigger('click');
    await flushPromises();
    expect(invoke).toHaveBeenCalledWith('p006_start', {
      config: expect.objectContaining({
        consent: true,
        seconds: 8,
        mode,
        role: 'sender',
      }),
    });
    expect(
      (w.get('input[type=checkbox]').element as HTMLInputElement).checked,
    ).toBe(false);
    expect((choice.element as HTMLSelectElement).value).toBe('');
    w.unmount();
  },
);
it('ordinary P006 UI keeps its desktop/30-second scope', async () => {
  const w = mount(RelayVideoProbe, { props: { directEnabled: true } });
  await flushPromises();
  expect(w.text()).toContain('发送屏幕');
  expect(w.text()).toContain('30秒');
  expect(w.text()).not.toContain('R06 ·');
  expect(w.find('[aria-label="本轮诊断方式"]').exists()).toBe(false);
  expect((w.get('select').element as HTMLSelectElement).value).toBe(
    'direct_preferred',
  );
  w.unmount();
});
