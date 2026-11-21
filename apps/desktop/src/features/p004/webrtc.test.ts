import { describe, expect, it } from 'vitest';
import { browserMeasurements, loopbackSdp } from './webrtc';

const loopback = 'a=candidate:one 1 udp 100 127.0.0.1 53100 typ host';
const masked = 'a=candidate:two 1 udp 200 hidden.local 53142 typ host';
const lan = 'a=candidate:three 1 udp 300 192.0.2.42 53143 typ host';
const sdp = (
  type: 'offer' | 'answer',
  candidates: string[],
  defaultIp = '0.0.0.0',
) =>
  [
    'v=0',
    'm=video 9 UDP/TLS/RTP/SAVPF 96',
    `c=IN IP4 ${defaultIp}`,
    `a=rtcp:9 IN IP4 ${defaultIp}`,
    'a=rtcp-mux',
    'a=rtpmap:96 H264/90000',
    'a=fmtp:96 profile-level-id=42001f;packetization-mode=1',
    type === 'offer' ? 'a=sendonly' : 'a=recvonly',
    ...candidates,
    '',
  ].join('\r\n');

describe('P0-04a loopback-only signaling', () => {
  it('accepts the complete single-media loopback offer', () => {
    expect(loopbackSdp(sdp('offer', [loopback]), 'offer')).toContain(loopback);
  });

  it.each([
    masked,
    lan,
    loopback.replace(' udp ', ' tcp '),
    loopback.replace(' typ host', ' typ relay'),
    loopback.replace(' 1 udp ', ' 2 udp '),
    loopback.replace('53100', '65536'),
    `${loopback} raddr 192.0.2.42 rport 53143`,
  ])('rejects a non-loopback offer candidate (%s)', (candidate) => {
    expect(() =>
      loopbackSdp(sdp('offer', [loopback, candidate]), 'offer'),
    ).toThrow('ICE_OFFER_NOT_LOOPBACK');
  });

  it('rejects an off-host default or absent loopback candidate', () => {
    expect(() =>
      loopbackSdp(sdp('offer', [loopback], '192.0.2.42'), 'offer'),
    ).toThrow('ICE_DEFAULT_NOT_LOOPBACK');
    expect(() => loopbackSdp(sdp('answer', [masked, lan]), 'answer')).toThrow(
      'ICE_LOOPBACK_UNAVAILABLE',
    );
  });

  it('signals only the valid browser candidate and its default address', () => {
    const answer = loopbackSdp(
      sdp('answer', [masked, loopback, lan], '192.0.2.42'),
      'answer',
    );
    expect(answer).toContain(loopback);
    expect(answer).toContain('m=video 53100');
    expect(answer).toContain('c=IN IP4 127.0.0.1');
    expect(answer).not.toContain('hidden.local');
    expect(answer).not.toContain('192.0.2.42');
  });

  it('rejects additional media, directions or unsupported codec', () => {
    const baseline = sdp('offer', [loopback]);
    for (const invalid of [
      baseline + 'm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n',
      baseline + 'a=sendrecv\r\n',
      baseline.replace('H264', 'VP8'),
    ]) {
      expect(() => loopbackSdp(invalid, 'offer')).toThrow(
        'SIGNAL_UNSUPPORTED_VIDEO',
      );
    }
  });
});

describe('actual receiver measurements', () => {
  const stats = (entries: Record<string, unknown>[]) =>
    new Map(
      entries.map((entry) => [entry.id, entry]),
    ) as unknown as RTCStatsReport;

  it('preserves measured zero and does not infer unavailable stats', () => {
    const incoming = browserMeasurements(
      stats([
        {
          id: 'in',
          type: 'inbound-rtp',
          kind: 'video',
          packetsReceived: 5,
          packetsLost: 0,
        },
        { id: 'other', type: 'candidate-pair', nominated: true },
      ]),
    );
    expect(incoming.packetsLost).toBe(0);
    expect(incoming.packetsReceived).toBe(5);
    expect(incoming.framesDecoded).toBeUndefined();
    expect(incoming.candidateScope).toBeUndefined();
  });

  it('gets the selected candidate pair without returning any IP addresses', () => {
    const browserStats = (address: string) =>
      browserMeasurements(
        stats([
          {
            id: 'in',
            type: 'inbound-rtp',
            kind: 'video',
            framesDecoded: 7,
            transportId: 't',
          },
          { id: 't', type: 'transport', selectedCandidatePairId: 'pair' },
          {
            id: 'pair',
            type: 'candidate-pair',
            localCandidateId: 'l',
            remoteCandidateId: 'r',
          },
          { id: 'l', address, candidateType: 'host' },
          {
            id: 'r',
            address: '127.0.0.1',
            candidateType: 'host',
            protocol: 'udp',
          },
        ]),
      );
    expect(browserStats('127.0.0.1')).toMatchObject({
      framesDecoded: 7,
      localCandidateType: 'host',
      remoteCandidateType: 'host',
      candidateScope: 'loopback',
    });
    expect(browserStats('hidden.local').candidateScope).toBe('unverified');
    expect(browserStats('192.0.2.42').candidateScope).toBe('off-host');
    expect(JSON.stringify(browserStats('192.0.2.42'))).not.toContain(
      '192.0.2.42',
    );
  });
});
