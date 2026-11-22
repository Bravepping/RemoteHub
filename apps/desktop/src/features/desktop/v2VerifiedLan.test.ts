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
