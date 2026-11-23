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
    expect(w.get('#p006-consent').attributes('disabled')).toBeDefined();
    expect(invoke).not.toHaveBeenCalledWith('p006_start', expect.anything());
    await w.findAll('button')[0].trigger('click');
    expect(invoke).not.toHaveBeenCalledWith('p006_start', expect.anything());
    await choice.setValue(
      mode === 'relay_only' ? 'direct_preferred' : 'relay_only',
    );
    await w.get('#p006-consent').setValue(true);
    await choice.setValue(mode);
    expect((w.get('#p006-consent').element as HTMLInputElement).checked).toBe(
      false,
    );
    await w.get('#p006-consent').setValue(true);
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
    const request = vi
      .mocked(invoke)
      .mock.calls.find(([name]) => name === 'p006_start')?.[1];
    expect(request).toHaveProperty('config');
    expect(request).not.toHaveProperty('config.diagnosticDropDirect');
    expect((w.get('#p006-consent').element as HTMLInputElement).checked).toBe(
      false,
    );
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
  expect(w.find('#r06-diagnostic-drop-direct').exists()).toBe(false);
  expect((w.get('select').element as HTMLSelectElement).value).toBe(
    'direct_preferred',
  );
  w.unmount();
});
it.each([
  ['R06_RELAY_PATH_METRIC_INVALID', 'R06_RELAY_PATH_METRIC_INVALID'],
  [
    'R06_RELAY_PATH_METRIC_INVALID: token=private-fixture',
    'RECEIVER_INTERNAL_ERROR',
  ],
])(
  'only renders an exact safe native error in the diagnostic panel',
  async (failure, label) => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'p006_interfaces') return ['192.168.1.2'];
      throw new Error(failure);
    });
    const w = mount(RelayVideoProbe, {
      props: { directEnabled: true, diagnosticOnly: true },
    });
    await flushPromises();
    await w.get('[aria-label="本轮诊断方式"]').setValue('relay_only');
    await w.get('#p006-consent').setValue(true);
    await w.findAll('button')[0].trigger('click');
    await flushPromises();
    expect(w.get('.relay-error').text()).toBe(label);
    expect(w.text()).not.toContain('private-fixture');
    w.unmount();
  },
);

it('sender may explicitly opt in to a single DirectPreferred simulation with fresh consent', async () => {
  const w = mount(RelayVideoProbe, {
    props: { directEnabled: true, diagnosticOnly: true },
  });
  await flushPromises();
  expect(w.find('#r06-diagnostic-drop-direct').exists()).toBe(false);
  const mode = w.get('[aria-label="本轮诊断方式"]');
  await mode.setValue('direct_preferred');
  const fault = w.get('#r06-diagnostic-drop-direct');
  const consent = w.get('#p006-consent');
  expect((fault.element as HTMLInputElement).checked).toBe(false);
  await consent.setValue(true);
  await fault.setValue(true);
  expect((consent.element as HTMLInputElement).checked).toBe(false);
  await w.findAll('button')[0].trigger('click');
  expect(invoke).not.toHaveBeenCalledWith('p006_start', expect.anything());
  await consent.setValue(true);
  await fault.setValue(false);
  expect((consent.element as HTMLInputElement).checked).toBe(false);
  await fault.setValue(true);
  await consent.setValue(true);
  await w.findAll('button')[0].trigger('click');
  await flushPromises();
  expect(invoke).toHaveBeenCalledWith('p006_start', {
    config: expect.objectContaining({
      role: 'sender',
      mode: 'direct_preferred',
      seconds: 8,
      consent: true,
      diagnosticDropDirect: true,
    }),
  });
  await mode.setValue('direct_preferred');
  expect(
    (w.get('#r06-diagnostic-drop-direct').element as HTMLInputElement).checked,
  ).toBe(false);
  w.unmount();
});

it.each(['receiver', 'relay_only'] as const)(
  'drops a stale fault request when changing to %s',
  async (change) => {
    const w = mount(RelayVideoProbe, {
      props: { directEnabled: true, diagnosticOnly: true },
    });
    await flushPromises();
    const mode = w.get('[aria-label="本轮诊断方式"]');
    await mode.setValue('direct_preferred');
    await w.get('#r06-diagnostic-drop-direct').setValue(true);
    await w.get('#p006-consent').setValue(true);
    if (change === 'relay_only') await mode.setValue(change);
    else await w.findAll('select')[1].setValue(change);
    expect(w.find('#r06-diagnostic-drop-direct').exists()).toBe(false);
    expect((w.get('#p006-consent').element as HTMLInputElement).checked).toBe(
      false,
    );
    await w.get('#p006-consent').setValue(true);
    await w.findAll('button')[0].trigger('click');
    await flushPromises();
    const request = vi
      .mocked(invoke)
      .mock.calls.find(([name]) => name === 'p006_start')?.[1];
    expect(request).toMatchObject({ config: { seconds: 8, consent: true } });
    expect(request).not.toHaveProperty('config.diagnosticDropDirect');
    w.unmount();
  },
);

it('shows actual native fault counts separately from path verification', async () => {
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === 'p006_interfaces') return ['192.168.1.2'];
    if (command === 'p006_start')
      return { sessionId: 'fixture', pairingCode: null };
    if (command === 'p006_ready') return { mode: 'direct_preferred' };
    if (command === 'p006_stats')
      return {
        state: 'connected',
        receiver_samples: [],
        resources_released: false,
        source: {
          diagnostic_transport: {
            drop_direct: true,
            dropped_packets: 7,
            dropped_direct_checks: 3,
          },
        },
      };
    return '';
  });
  const w = mount(RelayVideoProbe, {
    props: { directEnabled: true, diagnosticOnly: true },
  });
  await flushPromises();
  await w.get('[aria-label="本轮诊断方式"]').setValue('direct_preferred');
  await w.get('#r06-diagnostic-drop-direct').setValue(true);
  await w.get('#p006-consent').setValue(true);
  await w.findAll('button')[0].trigger('click');
  await flushPromises();
  const evidence = w.get('[aria-label="原生直连模拟统计"]');
  expect(evidence.text()).toContain('拦截直连 ICE 检查 3');
  expect(evidence.text()).toContain('拦截数据包总数 7');
  expect(evidence.text()).toContain('此统计不表示中继已通过');
  expect(w.text()).not.toContain('当前中继');
  w.unmount();
});
