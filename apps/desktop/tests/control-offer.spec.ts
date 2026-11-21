import { expect, test } from '@playwright/test';
test('real Edge creates bundled H264 video and reliable control.v1 without screen capture', async ({
  page,
}) => {
  await page.goto('/');
  const result = await page.evaluate(async () => {
    const pc = new RTCPeerConnection({
      iceServers: [],
      bundlePolicy: 'max-bundle',
    });
    const channel = pc.createDataChannel('control.v1', {
      ordered: true,
      protocol: 'control.v1',
    });
    const transceiver = pc.addTransceiver('video', { direction: 'recvonly' });
    const h264 = RTCRtpReceiver.getCapabilities('video')?.codecs.filter(
      (c) =>
        c.mimeType.toLowerCase() === 'video/h264' &&
        c.sdpFmtpLine?.includes('profile-level-id=42001f') &&
        c.sdpFmtpLine.includes('packetization-mode=1'),
    );
    if (!h264?.length) throw new Error('H264_UNAVAILABLE');
    transceiver.setCodecPreferences(h264);
    const offer = await pc.createOffer();
    const lines = offer.sdp!.split(/\r?\n/);
    const summary = {
      media: lines.filter((l) => l.startsWith('m=')),
      mids: lines.filter((l) => l.startsWith('a=mid:')),
      bundle: lines.find((l) => l.startsWith('a=group:BUNDLE')),
      sctp: lines.filter(
        (l) =>
          l.startsWith('a=sctp-port:') || l.startsWith('a=max-message-size:'),
      ),
      ordered: channel.ordered,
      protocol: channel.protocol,
      retransmits: channel.maxRetransmits,
      lifetime: channel.maxPacketLifeTime,
    };
    channel.close();
    pc.close();
    return summary;
  });
  expect(result.media).toHaveLength(2);
  expect(result.media[0]).toMatch(/^m=video [1-9]\d* UDP\/TLS\/RTP\/SAVPF /);
  expect(result.media[1]).toMatch(
    /^m=application [1-9]\d* UDP\/DTLS\/SCTP webrtc-datachannel$/,
  );
  expect(result.mids).toEqual(['a=mid:0', 'a=mid:1']);
  expect(result.bundle).toBe('a=group:BUNDLE 0 1');
  expect(result.sctp).toHaveLength(2);
  expect(result.ordered).toBe(true);
  expect(result.protocol).toBe('control.v1');
  expect(result.retransmits).toBeNull();
  expect(result.lifetime).toBeNull();
});
