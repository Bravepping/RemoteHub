import { afterEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { LoopbackProbe, loopbackOffer } from './webrtc';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
afterEach(() => {
  vi.resetAllMocks();
  vi.unstubAllGlobals();
});

describe('same-machine signaling', () => {
  it('removes LAN, relay and mDNS destinations before native signaling', () => {
    const sdp = [
      'v=0',
      'c=IN IP4 192.0.2.2',
      'a=candidate:1 1 udp 1 127.0.0.1 1234 typ host',
      'a=candidate:2 1 udp 1 192.0.2.2 1234 typ host',
      'a=candidate:3 1 udp 1 personal.local 1234 typ host',
      'a=candidate:4 1 udp 1 127.0.0.1 1234 typ relay',
      '',
    ].join('\r\n');
    const filtered = loopbackOffer(sdp);
    expect(filtered).toContain('127.0.0.1 1234 typ host');
    expect(filtered).not.toMatch(/192\.0\.2\.2|personal\.local|typ relay/);
    expect(filtered).toContain('c=IN IP4 0.0.0.0');
  });
  it('revokes a late native grant after the view has already stopped', async () => {
    let approve!: (value: { sessionId: string }) => void;
    const authorization = new Promise<{ sessionId: string }>((resolve) => {
      approve = resolve;
    });
    vi.mocked(invoke)
      .mockReturnValueOnce(authorization)
      .mockResolvedValue(undefined);
    const pc = vi.fn();
    vi.stubGlobal('RTCPeerConnection', pc);
    const failed = vi.fn();
    const probe = new LoopbackProbe(
      document.createElement('video'),
      vi.fn(),
      failed,
      vi.fn(),
    );
    const starting = probe.start(true, 15);
    await probe.stop();
    approve({ sessionId: 'late-window-grant' });
    await starting;
    expect(pc).not.toHaveBeenCalled();
    expect(invoke).toHaveBeenLastCalledWith('p004_stop_loopback_probe', {
      sessionId: 'late-window-grant',
    });
    expect(failed).not.toHaveBeenCalled();
  });
});
