import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import assert from 'node:assert/strict';

const folder = resolve(process.argv[2] ?? '');
const fixturePath = process.argv[3];
const ffmpeg = process.env.REMOTEHUB_FFMPEG ?? 'ffmpeg';
const ffprobe = process.env.REMOTEHUB_FFPROBE ?? 'ffprobe';
function run(binary, args, binaryOutput = false) {
  const result = spawnSync(binary, args, {
    encoding: binaryOutput ? undefined : 'utf8',
    maxBuffer: 20 * 1024 * 1024,
    timeout: 30000,
  });
  assert.equal(
    result.status,
    0,
    `${binary}: ${String(result.stderr)} ${result.error ?? ''}`,
  );
  return result.stdout;
}
const report = JSON.parse(
  readFileSync(join(folder, 'encode-report.json'), 'utf8'),
);
const raw = join(folder, 'desktop.h264');
const stream = JSON.parse(
  run(ffprobe, [
    '-v',
    'error',
    '-count_frames',
    '-select_streams',
    'v:0',
    '-show_entries',
    'stream=codec_name,width,height,pix_fmt,profile,has_b_frames,r_frame_rate,nb_read_frames,color_range,color_space',
    '-of',
    'json',
    raw,
  ]),
).streams[0];
assert.equal(stream.codec_name, 'h264');
assert.equal(stream.width, 1280);
assert.equal(stream.height, 720);
assert.equal(Number(stream.nb_read_frames), report.encoded_frames);
assert.equal(stream.has_b_frames, 0);
assert.ok(
  report.video_duration_ms >= 10000,
  'full acceptance sample must be at least 10 seconds',
);
assert.equal(
  report.missed_ticks,
  0,
  'cannot reconstruct CFR MP4 when ticks were missed',
);
assert.equal(
  report.encoded_frames,
  Math.round((report.video_duration_ms / 1000) * 30),
);
report.frames.forEach((frame, index) => {
  assert.equal(frame.pts_100ns, Math.floor((index * 10000000) / 30));
  assert.equal(
    frame.duration_100ns,
    Math.floor(((index + 1) * 10000000) / 30) - frame.pts_100ns,
  );
});
const idr = report.frames
  .map((frame, i) => (frame.idr ? i : -1))
  .filter((i) => i >= 0);
assert.equal(idr[0], 0);
assert.ok(report.frames[0].nal_types.includes(7));
assert.ok(report.frames[0].nal_types.includes(8));
const gaps = idr.slice(1).map((value, index) => value - idr[index]);
assert.ok(gaps.every((gap) => gap <= 60));
run(ffmpeg, [
  '-hide_banner',
  '-v',
  'error',
  '-xerror',
  '-i',
  raw,
  '-f',
  'null',
  '-',
]);
const md5 = run(ffmpeg, [
  '-hide_banner',
  '-v',
  'error',
  '-i',
  raw,
  '-f',
  'framemd5',
  '-',
]);
const rows = md5.split(/\r?\n/).filter((line) => line && !line.startsWith('#'));
const distinct = new Set(rows.map((line) => line.split(',').at(-1).trim()))
  .size;
assert.ok(
  distinct >= 30,
  `motion fixture not demonstrated: only ${distinct} decoded images`,
);
const mp4 = join(folder, 'desktop.mp4');
const reusedMp4 = existsSync(mp4);
// Raw Annex B has no container PTS. Exact CFR MF sample times were checked above before remuxing.
if (!reusedMp4)
  run(ffmpeg, [
    '-hide_banner',
    '-v',
    'error',
    '-fflags',
    '+genpts',
    '-r',
    '30',
    '-i',
    raw,
    '-c',
    'copy',
    '-movflags',
    '+faststart',
    mp4,
  ]);
const container = JSON.parse(
  run(ffprobe, [
    '-v',
    'error',
    '-select_streams',
    'v:0',
    '-show_packets',
    '-show_entries',
    'packet=pts_time,dts_time,duration_time:format=duration',
    '-of',
    'json',
    mp4,
  ]),
);
assert.equal(container.packets.length, report.encoded_frames);
container.packets.forEach((packet, i) =>
  assert.ok(Math.abs(Number(packet.pts_time) - i / 30) < 0.00001),
);
assert.ok(Number(container.format.duration) >= 10);
const mp4Stream = JSON.parse(
  run(ffprobe, [
    '-v',
    'error',
    '-select_streams',
    'v:0',
    '-show_entries',
    'stream=r_frame_rate,avg_frame_rate,width,height,codec_name',
    '-of',
    'json',
    mp4,
  ]),
).streams[0];
assert.equal(mp4Stream.r_frame_rate, '30/1');
assert.equal(mp4Stream.avg_frame_rate, '30/1');
const mp4Hashes = run(ffmpeg, [
  '-hide_banner',
  '-v',
  'error',
  '-i',
  mp4,
  '-f',
  'framemd5',
  '-',
])
  .split(/\r?\n/)
  .filter((line) => line && !line.startsWith('#'))
  .map((line) => line.split(',').at(-1).trim());
assert.deepEqual(
  mp4Hashes,
  rows.map((line) => line.split(',').at(-1).trim()),
  'MP4 and original MFT bitstream decode to identical frames',
);
let colorCheck = { status: 'not requested' };
if (fixturePath) {
  const fixture = JSON.parse(readFileSync(fixturePath, 'utf8').trim());
  const rgb = run(
    ffmpeg,
    [
      '-hide_banner',
      '-v',
      'error',
      '-i',
      raw,
      '-vf',
      'select=eq(n\\,10),format=rgb24',
      '-frames:v',
      '1',
      '-f',
      'rawvideo',
      '-',
    ],
    true,
  );
  const expected = [
    [255, 255, 255],
    [255, 255, 0],
    [0, 255, 255],
    [0, 255, 0],
    [255, 0, 255],
    [255, 0, 0],
    [0, 0, 255],
    [0, 0, 0],
  ];
  const colors = expected.map((color, i) => {
    const x = Math.floor(
      ((fixture.left + (fixture.width * (i + 0.5)) / 8) * 1280) /
        report.source_width,
    );
    const y = Math.floor(
      ((fixture.top + fixture.height * 0.25) * 720) / report.source_height,
    );
    const actual = [
      ...rgb.subarray((y * 1280 + x) * 3, (y * 1280 + x) * 3 + 3),
    ];
    assert.equal(actual.length, 3);
    assert.ok(
      color.every((v, c) => Math.abs(v - actual[c]) <= 12),
      `color ${i}: ${actual} expected ${color}`,
    );
    return { expected: color, decoded: actual };
  });
  colorCheck = { status: 'PASS', colors };
}
const summary = {
  status: 'PASS',
  stream,
  decoded_frames: rows.length,
  distinct_decoded_images: distinct,
  idr_frame_indices: idr,
  maximum_idr_gap_frames: Math.max(0, ...gaps),
  mf_pts_check: 'PASS',
  mp4_timestamp_note:
    'CFR timestamps reconstructed from the verified 30Hz MF sidecar; no re-encoding',
  mp4_duration_seconds: Number(container.format.duration),
  mp4_stream: mp4Stream,
  raw_rate_note:
    stream.r_frame_rate !== '30/1'
      ? 'raw Annex B parser nominal rate differs; use verified MF PTS and 30fps MP4/RTP timestamps, not raw-stream rate guesses'
      : 'raw parser rate agrees',
  color_check: colorCheck,
  decode_errors: 0,
};
writeFileSync(
  join(folder, 'validation.json'),
  JSON.stringify(summary, null, 2),
);
console.log(JSON.stringify(summary, null, 2));
