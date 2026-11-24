import { invoke } from '@tauri-apps/api/core';
import { flushPromises, mount } from '@vue/test-utils';
import { afterEach, expect, it, vi } from 'vitest';
import type { RuntimeInfo } from '../../bridge';
import DesktopShell from './DesktopShell.vue';
import PublicVideoPanel from '../public-video/PublicVideoPanel.vue';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
afterEach(() => vi.resetAllMocks());

const runtime: RuntimeInfo = {
  app_version: 'fixture',
  os: 'windows',
  arch: 'x86_64',
  capture_backend: 'fixture',
  remote_access_enabled: false,
  public_video_available: true,
};
const generation = 'a'.repeat(32);

it('keeps a public session mounted through refresh and stale capability removal', async () => {
  vi.mocked(invoke).mockImplementation(async (command) => {
    switch (command) {
      case 'public_video_start':
        return generation;
      case 'public_video_status':
        return { phase: 'starting' };
      default:
        return undefined;
    }
  });
  const shell = mount(DesktopShell, {
    props: { runtime, loading: false, readStatus: 'ready', feedback: '' },
    global: { stubs: { HomePage: true } },
  });
  const panel = shell.getComponent(PublicVideoPanel);
  await panel.get('input[value="host"]').setValue(true);
  await panel.get('button').trigger('click');
  await flushPromises();
  expect(invoke).toHaveBeenCalledWith('public_video_status', { generation });

  await shell.setProps({
    runtime: undefined,
    loading: true,
    readStatus: 'refreshing',
  });
  expect(shell.getComponent(PublicVideoPanel).element).toBe(panel.element);
  expect(invoke).not.toHaveBeenCalledWith('public_video_stop', { generation });
  await shell.setProps({
    runtime: { ...runtime, public_video_available: false },
    loading: false,
    readStatus: 'ready',
  });
  expect(shell.getComponent(PublicVideoPanel).element).toBe(panel.element);
  expect(invoke).not.toHaveBeenCalledWith('public_video_stop', { generation });

  await panel.findAll('button')[1].trigger('click');
  await flushPromises();
  expect(invoke).toHaveBeenCalledWith('public_video_stop', { generation });
  expect(panel.get('button').attributes('disabled')).toBeDefined();
  shell.unmount();
});

it('does not mount the public panel without a validated launch capability', () => {
  const shell = mount(DesktopShell, {
    props: {
      runtime: { ...runtime, public_video_available: false },
      loading: false,
      readStatus: 'ready',
      feedback: '',
    },
    global: { stubs: { HomePage: true } },
  });
  expect(shell.findComponent(PublicVideoPanel).exists()).toBe(false);
  expect(invoke).not.toHaveBeenCalled();
  shell.unmount();
});

it('keeps consumed status from native session even through an old runtime read', async () => {
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === 'public_video_start') return generation;
    if (command === 'public_video_status')
      return { phase: 'closed', profile_consumed: true };
    return undefined;
  });
  const shell = mount(DesktopShell, {
    props: { runtime, loading: false, readStatus: 'ready', feedback: '' },
    global: { stubs: { HomePage: true } },
  });
  const panel = shell.getComponent(PublicVideoPanel);
  await panel.get('input[value="host"]').setValue(true);
  await panel.get('button').trigger('click');
  await flushPromises();
  expect(panel.text()).toContain('本次启动配置已使用');
  expect(panel.get('button').attributes('disabled')).toBeDefined();
  await shell.setProps({
    runtime: { ...runtime, public_video_profile_consumed: false },
  });
  expect(panel.get('button').attributes('disabled')).toBeDefined();
  expect(
    vi
      .mocked(invoke)
      .mock.calls.filter(([command]) => command === 'public_video_start'),
  ).toHaveLength(1);
  shell.unmount();
});

it('never enables a profile already reported consumed in runtime info', () => {
  const shell = mount(DesktopShell, {
    props: {
      runtime: { ...runtime, public_video_profile_consumed: true },
      loading: false,
      readStatus: 'ready',
      feedback: '',
    },
    global: { stubs: { HomePage: true } },
  });
  const panel = shell.getComponent(PublicVideoPanel);
  expect(panel.get('fieldset').attributes('disabled')).toBeDefined();
  expect(panel.text()).toContain('本次启动配置已使用');
  expect(invoke).not.toHaveBeenCalled();
  shell.unmount();
});

it('shows the separate native permission on the control host without offering local input', async () => {
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === 'public_video_start') return generation;
    if (command === 'public_video_status')
      return {
        phase: 'starting',
        input_state: 'pending',
        profile_consumed: true,
      };
    return undefined;
  });
  const shell = mount(DesktopShell, {
    props: {
      runtime: { ...runtime, public_control_available: true },
      loading: false,
      readStatus: 'ready',
      feedback: '',
    },
    global: { stubs: { HomePage: true } },
  });
  const panel = shell.getComponent(PublicVideoPanel);
  await panel.get('input[value="host"]').setValue(true);
  await panel.get('button').trigger('click');
  await flushPromises();
  expect(panel.text()).toContain('公网远控测试');
  expect(panel.text()).toContain('时限以原生配置为准');
  expect(panel.text()).toContain('等待本机独立的原生键鼠确认，默认选择“否”。');
  expect(panel.text()).not.toContain('启用本次键鼠');
  expect(invoke).not.toHaveBeenCalledWith(
    'public_control_send_input',
    expect.anything(),
  );
  shell.unmount();
});

it.each([
  [15, '本轮会话最多 15 秒'],
  [60, '本轮会话最多 60 秒'],
  ['60', '时限以原生配置为准'],
  [30, '时限以原生配置为准'],
] as const)(
  'shows only a trusted native round limit through runtime refresh (%s)',
  async (seconds, label) => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'public_video_start') return generation;
      if (command === 'public_video_status')
        return {
          phase: 'starting',
          round_seconds: seconds,
          profile_consumed: true,
        };
      return undefined;
    });
    const shell = mount(DesktopShell, {
      props: {
        runtime: { ...runtime, public_control_available: true },
        loading: false,
        readStatus: 'ready',
        feedback: '',
      },
      global: { stubs: { HomePage: true } },
    });
    const panel = shell.getComponent(PublicVideoPanel);
    expect(panel.text()).not.toContain('本轮会话最多');
    expect(panel.text()).toContain('键鼠需共享端另行原生批准');
    await panel.get('input[value="host"]').setValue(true);
    await panel.get('button').trigger('click');
    await flushPromises();
    expect(panel.text()).toContain(label);
    await shell.setProps({
      runtime: undefined,
      loading: true,
      readStatus: 'refreshing',
    });
    expect(panel.text()).toContain(label);
    expect(invoke).not.toHaveBeenCalledWith('public_video_stop', {
      generation,
    });
    shell.unmount();
  },
);

it.each([
  { flag: true, message: '本次启动配置已使用', canRestart: false },
  { flag: false, message: '本次会话已停止', canRestart: true },
  { flag: undefined, message: '无法确认本次启动配置', canRestart: false },
  { flag: 'false', message: '无法确认本次启动配置', canRestart: false },
])(
  'waits for same-generation closed proof after local Stop (profile_consumed=$flag)',
  async ({ flag, message, canRestart }) => {
    let resolveStop!: () => void;
    const stop = new Promise<void>((resolve) => {
      resolveStop = resolve;
    });
    let stopped = false;
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === 'public_video_start') return generation;
      if (command === 'public_video_status')
        return stopped
          ? { phase: 'closed', profile_consumed: flag }
          : { phase: 'starting', profile_consumed: false };
      if (command === 'public_video_stop') {
        await stop;
        stopped = true;
      }
      return undefined;
    });
    const shell = mount(DesktopShell, {
      props: { runtime, loading: false, readStatus: 'ready', feedback: '' },
      global: { stubs: { HomePage: true } },
    });
    const panel = shell.getComponent(PublicVideoPanel);
    await panel.get('input[value="host"]').setValue(true);
    await panel.get('button').trigger('click');
    await flushPromises();
    await panel.findAll('button')[1].trigger('click');
    await flushPromises();
    expect(panel.get('button').attributes('disabled')).toBeDefined();
    let statusReads = vi
      .mocked(invoke)
      .mock.calls.filter(([command]) => command === 'public_video_status');
    expect(statusReads).toHaveLength(1);
    resolveStop();
    await flushPromises();
    statusReads = vi
      .mocked(invoke)
      .mock.calls.filter(([command]) => command === 'public_video_status');
    expect(statusReads).toEqual([
      ['public_video_status', { generation }],
      ['public_video_status', { generation }],
    ]);
    expect(panel.text()).toContain(message);
    expect(panel.get('button').attributes('disabled') === undefined).toBe(
      canRestart,
    );
    shell.unmount();
  },
);
