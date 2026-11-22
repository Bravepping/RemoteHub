import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import App from './App.vue';
import { createDesktopState } from './state/desktopState';
import { getRuntimeInfo, runCaptureProbe, type RuntimeInfo } from './bridge';

vi.mock('./bridge', async (original) => {
  const actual = await original<typeof import('./bridge')>();
  return { ...actual, getRuntimeInfo: vi.fn(), runCaptureProbe: vi.fn() };
});
const info: RuntimeInfo = {
  app_version: 'fixture-version',
  os: 'fixture-os',
  arch: 'fixture-arch',
  capture_backend: 'fixture-backend',
  remote_access_enabled: false,
};
let wrapper: VueWrapper;
function testState() {
  return createDesktopState({ readRuntime: () => getRuntimeInfo() });
}
async function open() {
  wrapper = mount(App, { global: { plugins: [testState()] } });
  await flushPromises();
  return wrapper;
}
async function visit(title: string) {
  const link = wrapper.findAll('nav a').find((a) => a.text() === title)!;
  await link.trigger('click');
  await flushPromises();
}
describe('desktop navigation with real capability boundaries', () => {
  beforeEach(() => {
    vi.resetAllMocks();
    window.history.replaceState(null, '', '/');
    vi.mocked(getRuntimeInfo).mockResolvedValue(info);
  });
  afterEach(() => {
    wrapper?.unmount();
    window.history.replaceState(null, '', '/');
  });
  it('has five reachable pages and does not invoke capture', async () => {
    await open();
    expect(wrapper.findAll('nav a')).toHaveLength(5);
    for (const name of ['设备', '连接', '会话', '设置', '首页']) {
      await visit(name);
      expect(wrapper.get('h1').text()).toBe(name);
      expect(wrapper.get('nav [aria-current="page"]').text()).toBe(name);
    }
    expect(getRuntimeInfo).toHaveBeenCalledTimes(1);
    expect(runCaptureProbe).not.toHaveBeenCalled();
    expect(wrapper.find('input[type="checkbox"]').exists()).toBe(false);
  });
  it('passes a native opt-in diagnostic flag to the product workspace', async () => {
    vi.mocked(getRuntimeInfo).mockResolvedValue({
      ...info,
      p3_enabled: true,
      v2_loopback_diagnostic_enabled: true,
    });
    await open();
    expect(wrapper.find('[aria-label="局域网远程会话"]').exists()).toBe(true);
    expect(wrapper.get('#v2-diagnostic-heading').text()).toBe(
      'V2 本机信令诊断',
    );
    expect(runCaptureProbe).not.toHaveBeenCalled();
  });
  it('renders core-returned fields only in read-only settings', async () => {
    await open();
    await visit('设置');
    expect(wrapper.text()).toContain('fixture-version');
    expect(wrapper.text()).toContain('fixture-backend');
    expect(wrapper.find('input').exists()).toBe(false);
  });
  it('shows honest empty states instead of device or metric fixtures', async () => {
    await open();
    await visit('设备');
    expect(wrapper.text()).toContain('设备列表尚未接入');
    expect(
      wrapper
        .findAll('button')
        .find((b) => b.text() === '添加设备')!
        .attributes('disabled'),
    ).toBeDefined();
    await visit('连接');
    expect(wrapper.text()).toContain('连接能力暂不可用');
    expect(wrapper.get('button').attributes('disabled')).toBeDefined();
    await visit('会话');
    expect(wrapper.text()).toContain('暂无会话数据');
    expect(wrapper.text()).not.toMatch(/\b0\s*(FPS|ms)\b|RH-\d/);
    expect(runCaptureProbe).not.toHaveBeenCalled();
  });
  it('keeps all navigation available when the bridge is unavailable', async () => {
    vi.mocked(getRuntimeInfo).mockRejectedValue(
      new Error('private-error-fixture'),
    );
    await open();
    await visit('设置');
    expect(wrapper.get('[role="alert"]').text()).toContain('请重新检测');
    expect(wrapper.get('[role="alert"]').text()).not.toContain('浏览器');
    expect(wrapper.text()).not.toContain('private-error-fixture');
    expect(wrapper.text()).not.toContain('fixture-backend');
    expect(wrapper.text()).toContain('未读取');
    expect(runCaptureProbe).not.toHaveBeenCalled();
  });
  it('reuses the real capability card on devices without creating a remote identity', async () => {
    await open();
    await visit('设备');
    expect(wrapper.get('[aria-label="本机能力卡片"]').text()).toContain(
      'fixture-os',
    );
    expect(wrapper.text()).not.toMatch(/RH-\d|在线设备/);
    await wrapper
      .findAll('button')
      .find((b) => b.text().includes('重新检测'))!
      .trigger('click');
    await flushPromises();
    expect(getRuntimeInfo).toHaveBeenCalledTimes(2);
    expect(runCaptureProbe).not.toHaveBeenCalled();
  });
  it('session preparation only navigates to the connection page', async () => {
    await open();
    await visit('会话');
    await wrapper
      .findAll('button')
      .find((b) => b.text().includes('查看连接准备'))!
      .trigger('click');
    await flushPromises();
    expect(wrapper.get('h1').text()).toBe('连接');
    expect(wrapper.find('dialog').exists()).toBe(false);
    expect(getRuntimeInfo).toHaveBeenCalledTimes(1);
    expect(runCaptureProbe).not.toHaveBeenCalled();
  });
  it('can retry a failed read and clears stale fields if a later read fails', async () => {
    vi.mocked(getRuntimeInfo)
      .mockRejectedValueOnce('unavailable')
      .mockResolvedValueOnce(info)
      .mockRejectedValueOnce('unavailable');
    await open();
    await visit('设置');
    await wrapper.get('button').trigger('click');
    await flushPromises();
    expect(wrapper.text()).toContain('fixture-version');
    await wrapper.get('button').trigger('click');
    await flushPromises();
    expect(wrapper.text()).not.toContain('fixture-version');
    expect(wrapper.text()).toContain('未读取');
    expect(getRuntimeInfo).toHaveBeenCalledTimes(3);
  });
  it('prevents overlapping capability refresh requests', async () => {
    let done!: (value: RuntimeInfo) => void;
    vi.mocked(getRuntimeInfo).mockImplementation(
      () =>
        new Promise((resolve) => {
          done = resolve;
        }),
    );
    wrapper = mount(App, { global: { plugins: [testState()] } });
    await flushPromises();
    const readButton = wrapper
      .findAll('button')
      .find((b) => b.text() === '读取中…')!;
    expect(readButton.attributes('disabled')).toBeDefined();
    await readButton.trigger('click');
    expect(getRuntimeInfo).toHaveBeenCalledTimes(1);
    done(info);
    await flushPromises();
    expect(
      wrapper
        .findAll('button')
        .find((b) => b.text().includes('重新检测'))!
        .attributes('disabled'),
    ).toBeUndefined();
  });
  it('honors a known deep link, and ignores executable-looking hash input', async () => {
    window.history.replaceState(null, '', '/#/sessions');
    await open();
    expect(wrapper.get('h1').text()).toBe('会话');
    window.location.hash = '#/connect?autoStart=true';
    window.dispatchEvent(new Event('hashchange'));
    await flushPromises();
    expect(wrapper.get('h1').text()).toBe('首页');
    expect(runCaptureProbe).not.toHaveBeenCalled();
  });
  it('keeps explicit experiment builds in their existing isolated workspace without a duplicate read', async () => {
    vi.mocked(getRuntimeInfo).mockResolvedValue({
      ...info,
      p004_enabled: true,
    });
    wrapper = mount(App, {
      global: {
        plugins: [testState()],
        stubs: {
          ExperimentWorkspace: {
            template: '<div data-test="experiment">experiment fixture</div>',
          },
        },
      },
    });
    await flushPromises();
    expect(wrapper.find('[data-test="experiment"]').exists()).toBe(true);
    expect(wrapper.find('nav').exists()).toBe(false);
    expect(getRuntimeInfo).toHaveBeenCalledTimes(1);
    expect(runCaptureProbe).not.toHaveBeenCalled();
  });
});
