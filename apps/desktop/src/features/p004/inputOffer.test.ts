import { expect, it } from 'vitest';
import { publishInputOffer } from './inputOffer';
import { lanOffer } from './lan';
const local = [10, 31, 47, 2].join('.');
const other = [10, 31, 47, 3].join('.');
const mdns = '00000000-0000-4000-8000-000000000001.local';
it('old empty publication is reproduced; product publishes only selected host UDP candidate with unchanged port', () => {
  const sdp = `v=0\r\na=candidate:f 1 udp 12 ${mdns} 51001 typ host\r\n`;
  expect(lanOffer(sdp, local)).not.toContain('a=candidate:');
  const result = publishInputOffer(sdp, local);
  expect(result.sdp).toContain(`${local} 51001 typ host`);
  expect(result.summary).toEqual({
    before: 1,
    mdns: 1,
    published: 1,
    mdnsPublished: 1,
  });
});
it('never publishes off-interface IPs, arbitrary DNS, relay/srflx, TCP or a second component', () => {
  const lines = [
    `f 1 udp 1 ${other} 51001 typ host`,
    `f 1 udp 1 external.example 51002 typ host`,
    `f 1 udp 1 ${mdns} 51003 typ srflx`,
    `f 1 tcp 1 ${mdns} 51004 typ host`,
    `f 2 udp 1 ${mdns} 51005 typ host`,
  ]
    .map((x) => 'a=candidate:' + x)
    .join('\r\n');
  expect(publishInputOffer(lines, local).summary.published).toBe(0);
  expect(() => publishInputOffer(lines, [192, 0, 2, 1].join('.'))).toThrow(
    'INPUT_LOCAL_ADDRESS_INVALID',
  );
});
it('retains literal selected candidates and rejects invalid ports or excessive candidates', () => {
  const line = `a=candidate:f 1 udp 12 ${local} 51001 typ host\r\n`;
  expect(publishInputOffer(line, local).sdp).toContain(
    `${local} 51001 typ host`,
  );
  for (const port of ['0', '1023', '65536', 'NaN'])
    expect(
      publishInputOffer(line.replace('51001', port), local).summary.published,
    ).toBe(0);
  expect(() => publishInputOffer(line.repeat(129), local)).toThrow(
    'INPUT_CANDIDATE_LIMIT',
  );
});
