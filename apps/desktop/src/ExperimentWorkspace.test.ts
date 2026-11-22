import { flushPromises, mount } from '@vue/test-utils';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import App from './features/experiments/ExperimentWorkspace.vue';
import { getRuntimeInfo, runCaptureProbe } from './bridge';

vi.mock('./bridge', async (original) => {
  const actual = await original<typeof import('./bridge')>();
  return { ...actual, getRuntimeInfo: vi.fn(), runCaptureProbe: vi.fn() };
});

const runtime = {
  app_version: '0.1.0',
  os: 'windows',
  arch: 'x86_64',
  capture_backend: 'DXGI test fixture',
  remote_access_enabled: false,
};

describe('local probe consent and truthful states', () => {
  beforeEach(() => vi.resetAllMocks());

  it('blocks native capture when the desktop bridge is unavailable', async () => {
    vi.mocked(getRuntimeInfo).mockRejectedValue(new Error('当前为浏览器预览'));
    const wrapper = mount(App);
    await flushPromises();
    expect(wrapper.get('[role="alert"]').text()).toContain('浏览器预览');
    expect(wrapper.get('input').attributes('disabled')).toBeDefined();
    expect(runCaptureProbe).not.toHaveBeenCalled();
  });

  it('requires a fresh consent and only renders measurements returned by the core', async () => {
    vi.mocked(getRuntimeInfo).mockResolvedValue(runtime);
    vi.mocked(runCaptureProbe).mockResolvedValue({
      width: 1920,
      height: 1080,
      frames: 12,
      timeouts: 1,
      elapsed_ms: 523,
      observed_fps: 22.94,
      frame_timestamps_us: [1],
      adapter: 'fixture-gpu',
    });
    const wrapper = mount(App);
    await flushPromises();
    const button = wrapper.findAll('button')[1];
    expect(button.attributes('disabled')).toBeDefined();
    await wrapper.get('input').setValue(true);
    await button.trigger('click');
    await flushPromises();
    expect(runCaptureProbe).toHaveBeenCalledTimes(1);
    expect(wrapper.get('[aria-label="采集结果"]').text()).toContain('22.94');
    expect(wrapper.get('[aria-label="采集结果"]').text()).toContain(
      'fixture-gpu',
    );
    expect((wrapper.get('input').element as HTMLInputElement).checked).toBe(
      false,
    );
    expect(button.attributes('disabled')).toBeDefined();
  });

  it('shows native errors without displaying a successful capture result', async () => {
    vi.mocked(getRuntimeInfo).mockResolvedValue(runtime);
    vi.mocked(runCaptureProbe).mockRejectedValue(
      'CAPTURE_UNAVAILABLE: display unavailable',
    );
    const wrapper = mount(App);
    await flushPromises();
    await wrapper.get('input').setValue(true);
    await wrapper.findAll('button')[1].trigger('click');
    await flushPromises();
    expect(wrapper.get('[role="alert"]').text()).toContain(
      'CAPTURE_UNAVAILABLE',
    );
    expect(wrapper.find('[aria-label="采集结果"]').exists()).toBe(false);
  });
});
