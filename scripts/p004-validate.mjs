// Validate native/WebView2 measurements, never substitute this for actually running the app.
import { readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { resolve, join } from 'node:path';
import assert from 'node:assert/strict';

const folder = resolve(process.argv[2] ?? 'artifacts/p004a');
const reports = readdirSync(folder)
  .filter((name) => name.endsWith('.json'))
  .map((name) => JSON.parse(readFileSync(join(folder, name), 'utf8')))
  .filter((data) => data.native);
const sessions = reports.filter((data) => Array.isArray(data.receiver));
const full = sessions.filter(
  (data) =>
    data.native.stream.frames_encoded >= 300 && !data.native.stream.stopped,
);
assert.ok(full.length >= 5, 'five full native starts/stops required');
for (const { native, receiver, browserClosed } of sessions) {
  assert.equal(native.error, null);
  assert.equal(native.resources_released, true);
  assert.equal(browserClosed, true);
  assert.equal(native.backpressure_failures, 0);
  assert.equal(native.stream.missed_ticks, 0);
  assert.ok(native.native_candidates.length > 0);
  assert.ok(
    native.native_candidates.every(
      (candidate) => candidate === 'host/127.0.0.1/udp',
    ),
  );
  assert.equal(native.negotiation.payload_type, 102);
  assert.ok(native.negotiation.fmtp.includes('profile-level-id=42001f'));
  assert.ok(native.negotiation.fmtp.includes('packetization-mode=1'));
  assert.ok(['42001f', '42041f'].includes(native.source_sps_profile_level_id));
  assert.equal(
    native.ignored_constraint_set5,
    native.source_sps_profile_level_id === '42041f',
  );
  const final = receiver.at(-1);
  assert.equal(final.videoWidth, 1280);
  assert.equal(final.videoHeight, 720);
  assert.equal(final.codec, 'video/H264');
  assert.equal(final.selectedRemoteLoopback, true);
  assert.equal(final.packetsLost, 0);
  assert.equal(final.framesReceived, native.frames_sent);
  assert.equal(final.framesDecoded, native.frames_sent);
  assert.equal(final.framesRendered, native.frames_sent);
  assert.equal(final.packetsReceived, native.rtp_packets_written);
  assert.ok(receiver.every((point) => point.selectedRemoteLoopback !== false));
  const decoded = receiver.filter((point) => point.framesDecoded !== null);
  assert.ok(
    decoded.every(
      (point, i) =>
        i === 0 || point.framesDecoded >= decoded[i - 1].framesDecoded,
    ),
  );
  if (!native.stream.stopped) {
    assert.ok(final.currentTime >= 10);
    assert.ok(
      decoded.length >= 5 &&
        decoded.at(-1).framesDecoded > decoded[0].framesDecoded,
      'decode counter must grow',
    );
    assert.equal(native.stream.frames_input, native.stream.frames_encoded);
    assert.equal(native.frames_sent, native.stream.frames_encoded);
    assert.equal(
      native.last_rtp_timestamp,
      Math.round((native.stream.last_pts_100ns * 9) / 1000),
    );
  }
}
assert.ok(sessions.some((data) => data.native.stream.frames_encoded === 900));
assert.ok(
  sessions.some(
    (data) =>
      data.native.stream.stopped && data.native.stream.frames_encoded < 900,
  ),
);
assert.ok(
  reports.some(
    (data) =>
      data.event === 'window-close' &&
      data.cleanupSucceeded &&
      data.native.resources_released &&
      data.native.stream.stopped,
  ),
);
const result = {
  status: 'PASS',
  full_sessions: full.length,
  maximum_decoded_frames: Math.max(
    ...sessions.map((data) => data.receiver.at(-1).framesDecoded),
  ),
  active_stop_verified: true,
  active_window_close_verified: true,
  original_desktop_media_included: false,
};
writeFileSync(
  join(folder, 'acceptance-validation.json'),
  JSON.stringify(result, null, 2),
);
console.log(JSON.stringify(result, null, 2));
