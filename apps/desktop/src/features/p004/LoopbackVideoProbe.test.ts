import { flushPromises, mount } from '@vue/test-utils';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  getLoopbackStats,
  signalLoopback,
  startLoopback,
  stopLoopback,
} from './bridge';
import LoopbackVideoProbe from './LoopbackVideoProbe.vue';

const receiver = vi.hoisted(() => ({
  close: vi.fn(),
  negotiate: vi.fn(),
  sample: vi.fn(),
}));

vi.mock('./webrtc', () => ({
  LoopbackReceiver: class {
    close = receiver.close;
    negotiate = receiver.negotiate;
    sample = receiver.sample;
  },
}));

vi.mock('./bridge', async (original) => ({
  ...(await original<typeof import('./bridge')>()),
  getLoopbackStats: vi.fn(),
  signalLoopback: vi.fn(),
  startLoopback: vi.fn(),
  stopLoopback: vi.fn(),
}));

const sending = {
  state: 'connected' as const,
  framesEncoded: 26,
  bytesSent: 8192,
  framesSent: 25,
  packetsSent: 9,
  rtpPacketsSent: null,
  backpressureCount: 0,
  elapsedMs: 200,
};
const connected = { peerState: 'connected', iceState: 'connected' };

describe('P0-04a probe UI', () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(startLoopback).mockResolvedValue({
      sessionId: 'one',
      offer: 'offer',
    });
    vi.mocked(signalLoopback).mockResolvedValue();
    vi.mocked(getLoopbackStats).mockResolvedValue(sending);
    vi.mocked(stopLoopback).mockImplementation(async () => {
      vi.mocked(getLoopbackStats).mockResolvedValue({
        ...sending,
        state: 'stopped',
      });
    });
    receiver.negotiate.mockResolvedValue('answer');
    receiver.sample.mockResolvedValue(connected);
  });

  afterEach(() => vi.useRealTimers());

  it('requires fresh consent and does not mistake connection or native frames for playback', async () => {
    vi.useFakeTimers();
    const wrapper = mount(LoopbackVideoProbe);
    const [start, stop] = wrapper.findAll('button');
    expect(start.attributes('disabled')).toBeDefined();
    await wrapper.get('input').setValue(true);
    await start.trigger('click');
    await flushPromises();
    expect(startLoopback).toHaveBeenCalledWith(15);
    expect(signalLoopback).toHaveBeenCalledWith('one', 'answer');
    expect(wrapper.get('[role="status"]').text()).toContain('等待真实视频帧');
    expect(wrapper.text()).not.toContain('1280 × 720');
    expect(wrapper.text()).toContain('未测');
    expect(wrapper.text()).not.toContain('null');
    receiver.sample.mockResolvedValue({
      ...connected,
      width: 1280,
      height: 720,
      framesRendered: 4,
      framesDecoded: 5,
      packetsLost: 0,
    });
    await vi.advanceTimersByTimeAsync(350);
    expect(wrapper.get('[role="status"]').text()).toContain('已收到视频帧');
    expect(wrapper.text()).toContain('1280 × 720');
    await stop.trigger('click');
    await flushPromises();
    expect(receiver.close).toHaveBeenCalledTimes(1);
    expect(stopLoopback).toHaveBeenCalledWith('one');
    expect(start.attributes('disabled')).toBeDefined();
    wrapper.unmount();
  });

  it('stops during native approval and cleans up a receipt that arrives late', async () => {
    let resolve!: (result: { sessionId: string; offer: string }) => void;
    vi.mocked(startLoopback).mockReturnValueOnce(
      new Promise((fn) => {
        resolve = fn;
      }),
    );
    const wrapper = mount(LoopbackVideoProbe);
    const [start, stop] = wrapper.findAll('button');
    await wrapper.get('input').setValue(true);
    await start.trigger('click');
    await stop.trigger('click');
    expect(stopLoopback).toHaveBeenCalledWith(undefined);
    resolve({ sessionId: 'late', offer: 'offer' });
    await flushPromises();
    expect(stopLoopback).toHaveBeenCalledWith('late');
    expect(signalLoopback).not.toHaveBeenCalled();
    expect(wrapper.get('[role="status"]').text()).toContain('已停止');
    wrapper.unmount();
  });

  it('closes browser and native resources if no loopback candidate is found', async () => {
    receiver.negotiate.mockRejectedValueOnce(
      new Error('ICE_LOOPBACK_UNAVAILABLE'),
    );
    const wrapper = mount(LoopbackVideoProbe);
    await wrapper.get('input').setValue(true);
    await wrapper.findAll('button')[0].trigger('click');
    await flushPromises();
    expect(signalLoopback).not.toHaveBeenCalled();
    expect(receiver.close).toHaveBeenCalledTimes(1);
    expect(stopLoopback).toHaveBeenCalledWith('one');
    expect(wrapper.get('[role="alert"]').text()).toContain(
      'ICE_LOOPBACK_UNAVAILABLE',
    );
    wrapper.unmount();
  });
});
