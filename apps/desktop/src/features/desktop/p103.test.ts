import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vitest';
import LocalDeviceCard from './LocalDeviceCard.vue';
import SessionToolbar from './SessionToolbar.vue';
import type { RuntimeInfo } from '../../bridge';

const fixture: RuntimeInfo = {
  app_version: 'fixture-version',
  os: 'fixture-os',
  arch: 'fixture-arch',
  capture_backend: 'fixture-backend',
  remote_access_enabled: false,
};
describe('P1-03 presentation without remote actions', () => {
  it('uses supplied local fields without inventing remote device IDs or online status', () => {
    const w = mount(LocalDeviceCard, {
      props: { runtime: fixture, loading: false },
    });
    expect(w.text()).toContain('fixture-os · fixture-arch');
    expect(w.text()).toContain('不代表设备在线或已获远控授权');
    expect(w.text()).toContain('远程访问关闭');
    expect(w.text()).not.toMatch(/RH-\d|\d+\s*(FPS|ms)/);
    w.unmount();
  });
  it('handles loading, missing capability and refresh without stale fields', async () => {
    const w = mount(LocalDeviceCard, {
      props: { runtime: fixture, loading: true },
    });
    expect(w.get('section').attributes('aria-busy')).toBe('true');
    await w.get('button').trigger('click');
    expect(w.emitted('refresh')).toBeUndefined();
    await w.setProps({ runtime: undefined, loading: false });
    expect(w.text()).toContain('本机能力未连接');
    expect(w.text()).not.toContain('fixture-os');
    await w.get('button').trigger('click');
    expect(w.emitted('refresh')).toHaveLength(1);
    w.unmount();
  });
  it('does not turn a core flag into a product remote authorization', () => {
    const w = mount(LocalDeviceCard, {
      props: {
        runtime: { ...fixture, remote_access_enabled: true },
        loading: false,
      },
    });
    expect(w.text()).toContain('远程服务尚未接入');
    expect(w.text()).not.toContain('远程访问已开启');
    w.unmount();
  });
  it('can collapse and expand the toolbar without creating session events', async () => {
    const w = mount(SessionToolbar);
    const toggle = w.get('[aria-controls="session-toolbar-actions"]');
    await toggle.trigger('click');
    expect(toggle.attributes('aria-expanded')).toBe('false');
    expect(w.get('#session-toolbar-actions').attributes('style')).toContain(
      'display: none',
    );
    await toggle.trigger('click');
    expect(toggle.attributes('aria-expanded')).toBe('true');
    expect(w.emitted('prepareConnection')).toBeUndefined();
    await w
      .findAll('button')
      .find((b) => b.text().includes('查看连接准备'))!
      .trigger('click');
    expect(w.emitted('prepareConnection')).toHaveLength(1);
    w.unmount();
  });
  it('keeps all session actions disabled and unavailable statistics explicit', async () => {
    const w = mount(SessionToolbar);
    for (const name of ['全屏', '画质', '码率上限', '断开']) {
      const button = w.findAll('button').find((b) => b.text() === name)!;
      expect(button.attributes('disabled')).toBeDefined();
      expect(button.attributes('aria-describedby')).toBe(
        'session-actions-unavailable',
      );
      await button.trigger('click');
    }
    expect(w.emitted('prepareConnection')).toBeUndefined();
    expect(w.text()).toContain('暂无会话');
    expect(w.findAll('dd').map((d) => d.text())).toEqual([
      '暂无会话',
      '未取得',
      '未取得',
    ]);
    expect(w.find('video, canvas, input').exists()).toBe(false);
    w.unmount();
  });
});
