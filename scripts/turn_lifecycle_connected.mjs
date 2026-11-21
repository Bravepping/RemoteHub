// Local operator broker for a real Edge <-> Rust SCTP connection over TURN. No media.
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import path from 'node:path';
const args = process.argv.slice(2);
if (
  args.length !== 4 ||
  args[0] !== '--approved-target' ||
  args[2] !== '--native-config'
) {
  console.error('EXPLICIT_APPROVAL_REQUIRED');
  process.exit(2);
}
const c = JSON.parse(readFileSync(args[1], 'utf8'));
const ticket = c.tickets['edge-connected'];
if (
  Date.now() / 1000 >= c.approval_expires_at ||
  ticket.expires_at - Date.now() / 1000 < 60
)
  throw new Error('APPROVAL_EXPIRED');
const child = spawn(
  path.resolve('target/probe/turn-allocate-probe.exe'),
  ['--approved-target', args[3], '--connected-lifecycle'],
  { stdio: ['pipe', 'pipe', 'pipe'] },
);
let answerResolve, answerReject;
const answer = new Promise((resolve, reject) => {
  answerResolve = resolve;
  answerReject = reject;
});
const nativeExit = new Promise((resolve) => child.once('exit', resolve));
createInterface({ input: child.stdout }).on('line', (line) => {
  try {
    const e = JSON.parse(line);
    if (e.event === 'private_answer') {
      answerResolve(e.sdp);
    } else {
      process.stdout.write(JSON.stringify(e) + '\n');
    }
  } catch {
    answerReject(new Error('NATIVE_OUTPUT_SCHEMA'));
  }
});
// Do not forward arbitrary native stderr (or SDP) to the published evidence stream.
let nativeError = false;
child.stderr.on('data', () => {
  nativeError = true;
});
const require = createRequire(path.resolve('apps/desktop/package.json'));
const { chromium } = require('@playwright/test');
let browser;
try {
  browser = await chromium.launch({
    channel: 'msedge',
    headless: true,
    args: ['--force-webrtc-ip-handling-policy=default_public_interface_only'],
  });
  const page = await browser.newPage();
  await page.exposeFunction('lifecycleReport', (e) =>
    process.stdout.write(JSON.stringify(e) + '\n'),
  );
  await page.exposeFunction('nativeAnswer', async (sdp) => {
    child.stdin.write(JSON.stringify({ offer: sdp }) + '\n');
    return await Promise.race([
      answer,
      new Promise((_, reject) =>
        setTimeout(() => reject(new Error('NATIVE_ANSWER_TIMEOUT')), 12000),
      ),
    ]);
  });
  await page.evaluate(
    async ({ target, ticket }) => {
      const report = async (event, data = {}) =>
        window.lifecycleReport({
          case: 'edge-connected',
          event,
          timeNs: String(BigInt(Date.now()) * 1000000n),
          data,
        });
      const pc = new RTCPeerConnection({
        iceTransportPolicy: 'relay',
        bundlePolicy: 'max-bundle',
        iceCandidatePoolSize: 0,
        iceServers: [
          {
            urls: [`turn:${target}:3478?transport=udp`],
            username: ticket.username,
            credential: ticket.credential,
          },
        ],
      });
      const dc = pc.createDataChannel('allocation.probe', { ordered: true });
      let candidates = 0;
      let invalid = false;
      pc.onicecandidate = (e) => {
        if (e.candidate) {
          const a = e.candidate;
          candidates++;
          if (
            a.type !== 'relay' ||
            a.protocol !== 'udp' ||
            a.address !== target ||
            a.port < 49160 ||
            a.port > 49259 ||
            candidates > 1
          ) {
            invalid = true;
            pc.close();
          }
        }
      };
      try {
        await report('before_gather');
        await pc.setLocalDescription(await pc.createOffer());
        await new Promise((resolve, reject) => {
          if (pc.iceGatheringState === 'complete') return resolve();
          const t = setTimeout(
            () => reject(new Error('EDGE_GATHER_TIMEOUT')),
            10000,
          );
          pc.onicegatheringstatechange = () => {
            if (pc.iceGatheringState === 'complete') {
              clearTimeout(t);
              resolve();
            }
          };
        });
        if (invalid || candidates !== 1)
          throw new Error('EDGE_ALLOCATION_BUDGET_OR_SCOPE');
        const sdp = await window.nativeAnswer(pc.localDescription.sdp);
        const pong = new Promise((resolve, reject) => {
          const t = setTimeout(() => reject(new Error('PONG_TIMEOUT')), 10000);
          dc.onopen = () => dc.send('lifecycle-ping');
          dc.onmessage = (e) => {
            if (e.data === 'lifecycle-pong') {
              clearTimeout(t);
              resolve();
            } else reject(new Error('PONG_SCHEMA'));
          };
        });
        await pc.setRemoteDescription({ type: 'answer', sdp });
        await pong;
        const stats = await pc.getStats();
        let verified = false;
        stats.forEach((v) => {
          if (v.type === 'transport' && v.selectedCandidatePairId) {
            const pair = stats.get(v.selectedCandidatePairId);
            const a = pair && stats.get(pair.localCandidateId),
              b = pair && stats.get(pair.remoteCandidateId);
            verified = !!(
              a &&
              b &&
              [a, b].every(
                (x) =>
                  x.candidateType === 'relay' &&
                  x.protocol === 'udp' &&
                  (x.address ?? x.ip) === target &&
                  x.port >= 49160 &&
                  x.port <= 49259,
              )
            );
          }
        });
        if (!verified || pc.connectionState !== 'connected')
          throw new Error('CONNECTED_RELAY_PAIR_REQUIRED');
        await report('connected_ping_pong', {
          localType: 'relay',
          remoteType: 'relay',
          approvedPair: true,
          connectionState: pc.connectionState,
          mediaTracks: 0,
        });
        await report('pc_close_call');
        pc.close();
        await report('pc_close_returned', {
          localPeerConnectionClosed: pc.connectionState === 'closed',
        });
        await new Promise((resolve) => setTimeout(resolve, 1500));
      } finally {
        pc.close();
      }
    },
    { target: c.target, ticket },
  );
  child.stdin.write('close\n');
  const exit = await Promise.race([
    nativeExit,
    new Promise((_, reject) =>
      setTimeout(() => reject(new Error('NATIVE_CLOSE_TIMEOUT')), 8000),
    ),
  ]);
  if (exit !== 0 || nativeError)
    throw new Error('NATIVE_CONNECTED_CASE_FAILED');
} catch {
  console.error('CONNECTED_LIFECYCLE_CASE_FAILED');
  process.exitCode = 1;
} finally {
  child.stdin.end();
  if (child.exitCode === null) child.kill();
  if (browser) await browser.close();
}
