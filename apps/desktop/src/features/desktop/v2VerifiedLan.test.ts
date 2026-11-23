import { flushPromises, mount } from '@vue/test-utils';
import { beforeEach, expect, it, vi } from 'vitest';
import type { LanConfig } from '../p004/lan';
import RemoteWorkspace from './RemoteWorkspace.vue';

const mocked = vi.hoisted(() => ({
  start: vi.fn<(config: LanConfig) => Promise<void>>(),
  stop: vi.fn<() => Promise<void>>(),
  input: vi.fn(),
  enableInput: vi.fn(),
  reportError: undefined as ((error: string) => void) | undefined,
  reportCode: undefined as ((code: string) => void) | undefined,
}));

vi.mock('../p004/lan', () => ({
  LanProbe: class {
    constructor(
      _video: HTMLVideoElement,
      _update: (state: string) => void,
      failed: (error: string) => void,
      code: (value: string) => void,
    ) {
      mocked.reportError = failed;
      mocked.reportCode = code;
    }
    start = mocked.start;
    stop = mocked.stop;
    input = mocked.input;
    enableInput = mocked.enableInput;
  },
}));

beforeEach(() => {
  vi.resetAllMocks();
  mocked.start.mockResolvedValue();
  mocked.stop.mockResolvedValue();
  mocked.reportError = undefined;
  mocked.reportCode = undefined;
});

const selector = '[aria-label="局域网会话模式"]';

it('leaves the legacy flow unchanged when the verified capability is absent', async () => {
  const view = mount(RemoteWorkspace);
  expect(view.find(selector).exists()).toBe(false);
  expect(view.text()).not.toContain('键鼠队列测试版');
  const [peer, local, pin, consent] = view.findAll('input');
  await peer.setValue('192.168.1.3');
  await local.setValue('192.168.1.2');
  await pin.setValue('legacy-pin');
  await consent.setValue(true);
  await view
    .findAll('button')
    .find((b) => b.text() === '确认后连接')!
    .trigger('click');
  expect(mocked.start).toHaveBeenCalledExactlyOnceWith({
    role: 'receiver',
    consent: true,
    peerIp: '192.168.1.3',
    localIp: '192.168.1.2',
    port: 48604,
    seconds: 30,
    pairingCode: 'legacy-pin',
    realControl: true,
    controlSim: false,
  });
  view.unmount();
});

it('describes independent default-No host input approval in the opt-in test build', async () => {
  const view = mount(RemoteWorkspace, { props: { lanInputQueueTest: true } });
  expect(view.text()).toContain('键鼠队列测试版');
  expect(view.text()).not.toContain('默认选择“否”');
  await view.get('select').setValue('sender');
  expect(view.text()).toContain('视频获批后，实际键鼠控制另有原生确认');
  expect(view.text()).toContain('默认选择“否”');
  expect(view.text()).toContain('此勾选不授权键鼠控制');
  const [peer, local, consent] = view.findAll('input');
  await peer.setValue('192.168.1.3');
  await local.setValue('192.168.1.2');
  await consent.setValue(true);
  await view
    .findAll('button')
    .find((b) => b.text() === '确认后等待配对')!
    .trigger('click');
  expect(mocked.start).toHaveBeenCalledExactlyOnceWith(
    expect.objectContaining({
      role: 'sender',
      realControl: true,
      consent: true,
    }),
  );
  expect(view.text()).not.toContain('已批准本次实际键鼠控制');
  mocked.reportError?.('Error: LAN_TEST_INPUT_CONSENT_DENIED');
  await flushPromises();
  expect(view.get('[role="alert"]').text()).toContain(
    '本次键鼠控制未获原生批准，会话已中止，不会自动退回仅视频模式',
  );
  expect(mocked.start).toHaveBeenCalledTimes(1);
  view.unmount();
});

it('does not present a known input release failure as completed or still running', async () => {
  const view = mount(RemoteWorkspace, { props: { lanInputQueueTest: true } });
  await view.get('select').setValue('sender');
  const [peer, local, consent] = view.findAll('input');
  await peer.setValue('192.168.1.3');
  await local.setValue('192.168.1.2');
  await consent.setValue(true);
  await view
    .findAll('button')
    .find((b) => b.text() === '确认后等待配对')!
    .trigger('click');

  mocked.reportError?.('INPUT_RELEASE_FAILED');
  await flushPromises();
  expect(view.get('[role="alert"]').text()).toContain(
    '无法确认本次会话按下的键或鼠标按钮已释放',
  );
  expect(view.text()).toContain('资源回收：未确认');
  expect(view.text()).not.toContain('资源回收：已完成');
  view.unmount();
});

it('does not show keyboard approval guidance in verified video, even on the test build', async () => {
  const view = mount(RemoteWorkspace, {
    props: { lanInputQueueTest: true, v2VerifiedLanEnabled: true },
  });
  await view.findAll('select')[1].setValue('sender');
  await view.get(selector).setValue('verified');
  expect(view.text()).toContain('仅观看，不授权键鼠');
  expect(view.text()).not.toContain('默认选择“否”');
  view.unmount();
});

it.each(['receiver', 'sender'] as const)(
  'requires fresh approval and uses verified video only on %s',
  async (role) => {
    const view = mount(RemoteWorkspace, {
      props: { v2VerifiedLanEnabled: true },
    });
    const selection = view.get(selector);
    expect((selection.element as HTMLSelectElement).value).toBe('legacy');
    await view.findAll('select')[1].setValue(role);
    const [peer, local] = view.findAll('input');
    await peer.setValue('192.168.1.3');
    await local.setValue('192.168.1.2');
    await view.get('input[type="checkbox"]').setValue(true);
    if (role === 'receiver')
      await view.get('input[type="password"]').setValue('old-mode-pin');
    await selection.setValue('verified');
    const consent = view.get('input[type="checkbox"]');
    expect((consent.element as HTMLInputElement).checked).toBe(false);
    if (role === 'receiver') {
      const pin = view.get('input[type="password"]');
      expect((pin.element as HTMLInputElement).value).toBe('');
      await pin.setValue('fresh-pin');
    }
    const start = view
      .findAll('button')
      .find(
        (b) =>
          b.text() === (role === 'sender' ? '确认后等待配对' : '确认后连接'),
      )!;
    expect(start.attributes('disabled')).toBeDefined();
    await consent.setValue(true);
    await start.trigger('click');
    await flushPromises();
    expect(mocked.start).toHaveBeenCalledExactlyOnceWith({
      role,
      consent: true,
      peerIp: '192.168.1.3',
      localIp: '192.168.1.2',
      port: 48604,
      seconds: 30,
      pairingCode: role === 'receiver' ? 'fresh-pin' : '',
      verifiedPeer: true,
      realControl: false,
      controlSim: false,
    });
    expect(view.text()).not.toContain('启用本次键鼠控制');
    expect(view.text()).not.toContain('已批准本次实际键鼠控制');
    expect(view.find('[aria-label="远程主屏视频区域"]').exists()).toBe(true);
    if (role === 'sender') {
      mocked.reportCode?.('fixture-invitation');
      await flushPromises();
      expect(view.text()).toContain('fixture-invitation');
    }
    mocked.reportError?.('LAN_VERIFIED_PEER_REQUIRED');
    await flushPromises();
    expect(view.get('[role="alert"]').text()).toContain('“设备”页');
    mocked.reportError?.('private-error-fixture');
    await flushPromises();
    expect(view.get('[role="alert"]').text()).not.toContain(
      'private-error-fixture',
    );
    view.unmount();
  },
);

it.each(['receiver', 'sender'] as const)(
  'starts explicit private WSS with verified video and no relay credentials from JS on %s',
  async (role) => {
    const view = mount(RemoteWorkspace, {
      props: { v2VerifiedLanEnabled: true, v2PrivateWssLanEnabled: true },
    });
    const selection = view.get(selector);
    expect(
      selection.findAll('option').map((option) => option.attributes('value')),
    ).toEqual(['legacy', 'verified', 'private-wss']);
    expect((selection.element as HTMLSelectElement).value).toBe('legacy');
    await view.findAll('select')[1].setValue(role);
    const [peer, local] = view.findAll('input');
    await peer.setValue('192.168.1.3');
    await local.setValue('192.168.1.2');
    await selection.setValue('private-wss');
    expect(view.text()).toContain('不会自动切换为普通局域网连接');
    if (role === 'receiver')
      await view.get('input[type="password"]').setValue('temporary-pin');
    const button = view
      .findAll('button')
      .find(
        (item) =>
          item.text() === (role === 'sender' ? '确认后等待配对' : '确认后连接'),
      )!;
    expect(button.attributes('disabled')).toBeDefined();
    await view.get('input[type="checkbox"]').setValue(true);
    await button.trigger('click');
    expect(mocked.start).toHaveBeenCalledExactlyOnceWith({
      role,
      consent: true,
      peerIp: '192.168.1.3',
      localIp: '192.168.1.2',
      port: 48604,
      seconds: 30,
      pairingCode: role === 'receiver' ? 'temporary-pin' : '',
      verifiedPeer: true,
      verifiedWss: true,
      realControl: false,
      controlSim: false,
    });
    expect(view.text()).not.toContain('启用本次键鼠控制');
    mocked.reportError?.('LAN_WSS_CONFIG_REQUIRED');
    await flushPromises();
    expect(view.get('[role="alert"]').text()).toContain(
      '在两台电脑配置原生客户端',
    );
    mocked.reportError?.('secret-token-fixture');
    await flushPromises();
    expect(view.get('[role="alert"]').text()).not.toContain(
      'secret-token-fixture',
    );
    expect(view.get('[role="alert"]').text()).toContain('没有切换到其他模式');
    expect(mocked.start).toHaveBeenCalledTimes(1);
    view.unmount();
  },
);

it('fails closed rather than starting ordinary LAN if the selected WSS capability disappears', async () => {
  const view = mount(RemoteWorkspace, {
    props: { v2VerifiedLanEnabled: true, v2PrivateWssLanEnabled: true },
  });
  const selection = view.get(selector);
  await selection.setValue('private-wss');
  const [peer, local, pin, consent] = view.findAll('input');
  await peer.setValue('192.168.1.3');
  await local.setValue('192.168.1.2');
  await pin.setValue('temporary-pin');
  await consent.setValue(true);
  await view.setProps({ v2PrivateWssLanEnabled: false });
  expect(view.text()).toContain('当前启动未启用所选模式');
  expect((selection.element as HTMLSelectElement).value).toBe('private-wss');
  expect((pin.element as HTMLInputElement).value).toBe('');
  expect((consent.element as HTMLInputElement).checked).toBe(false);
  await pin.setValue('another-temporary-pin');
  await consent.setValue(true);
  const start = view
    .findAll('button')
    .find((item) => item.text() === '确认后连接')!;
  expect(start.attributes('disabled')).toBeDefined();
  await start.trigger('click');
  expect(mocked.start).not.toHaveBeenCalled();
  view.unmount();
});
