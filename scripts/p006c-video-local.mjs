// Real Session -> MF H264 -> RTP -> Edge decoder + existing control.v1. Generated pixels only.
import { createRequire } from 'node:module';
import { pathToFileURL } from 'node:url';
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { networkInterfaces } from 'node:os';
import { mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
const args = process.argv.slice(2);
if (
  args.length !== 4 ||
  args[0] !== '--local' ||
  args[2] !== '--case' ||
  !['auto', 'sender-stop', 'receiver-stop', 'receiver-close'].includes(args[3])
)
  throw new Error('EXPLICIT_LOCAL_VIDEO_CASE_REQUIRED');
const local = args[1],
  scenario = args[3];
if (
  !Object.values(networkInterfaces())
    .flat()
    .some((a) => a?.family === 'IPv4' && !a.internal && a.address === local)
)
  throw new Error('LOCAL_INTERFACE_NOT_ASSIGNED');
const require = createRequire(path.resolve('apps/desktop/package.json'));
const { chromium } = require('@playwright/test');
const { createServer } = await import(
  pathToFileURL(require.resolve('vite')).href
);
async function bounded(promise, ms, code) {
  let timer;
  try {
    return await Promise.race([
      promise,
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error(code)), ms);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}
let vite, browser, child, nativeFinal;
let exited = false,
  nativeError = false;
try {
  vite = await createServer({
    root: path.resolve('apps/desktop'),
    server: { host: '127.0.0.1', port: 0 },
    logLevel: 'error',
  });
  await vite.listen();
  const url = `http://127.0.0.1:${vite.httpServer.address().port}`;
  browser = await chromium.launch({
    channel: 'msedge',
    headless: true,
    args: [
      '--force-webrtc-ip-handling-policy=default',
      '--disable-features=WebRtcHideLocalIpsWithMdns',
      '--autoplay-policy=no-user-gesture-required',
      '--use-fake-device-for-media-stream',
      '--use-fake-ui-for-media-stream',
    ],
  });
  const context = await browser.newContext({
    permissions: ['camera', 'microphone'],
  });
  const page = await context.newPage();
  await page.goto(url);
  // Chromium exposes non-default NICs only after media permission. Devices in THIS
  // test process are forced synthetic; no physical camera/microphone is accessed.
  await page.evaluate(async () => {
    const fake = await navigator.mediaDevices.getUserMedia({
      video: true,
      audio: false,
    });
    fake.getTracks().forEach((t) => t.stop());
  });
  let readyResolve, answerResolve, answerReject;
  const ready = new Promise((resolve) => {
    readyResolve = resolve;
  });
  const answer = new Promise((resolve, reject) => {
    answerResolve = resolve;
    answerReject = reject;
  });
  answer.catch(() => {});
  child = spawn(
    path.resolve('target/probe/video-session-probe.exe'),
    [
      '--synthetic',
      '--approved-local',
      local,
      '--seconds',
      scenario === 'auto' ? '10' : '15',
    ],
    { stdio: ['pipe', 'pipe', 'pipe'] },
  );
  let errorText = '';
  const nativeExit = new Promise((resolve) =>
    child.once('exit', (code) => {
      exited = true;
      resolve(code);
      if (code !== 0) {
        const safe = /^[A-Z_0-9]+$/.test(errorText.trim())
          ? errorText.trim()
          : 'NATIVE_FAILED';
        console.log(safe);
        answerReject(new Error(safe));
      }
    }),
  );
  child.stderr.on('data', (b) => {
    nativeError = true;
    errorText += errorText.length < 8192 ? b.toString() : '';
  });
  createInterface({ input: child.stdout }).on('line', (line) => {
    const e = JSON.parse(line);
    if (e.event === 'private_ready') readyResolve(e.sessionId);
    else if (e.event === 'private_answer') answerResolve(e.sdp);
    else if (e.event === 'native_final') nativeFinal = e.native;
    else if (e.event === 'native_draining' && !page.isClosed())
      void page.evaluate(async () => {
        await window.sessionFixture?.control.close();
      });
  });
  const session = await bounded(ready, 5000, 'NATIVE_READY_TIMEOUT');
  await page.exposeFunction('sessionAnswer', async (sdp) => {
    child.stdin.write(JSON.stringify({ offer: sdp }) + '\n');
    return await bounded(answer, 18000, 'NATIVE_ANSWER_TIMEOUT');
  });
  await page.evaluate(
    async ({ local, session }) => {
      const { ControlClient } = await import('/src/features/p004/control.ts');
      const { receiveStats } = await import('/src/features/p004/webrtc.ts');
      const { directSdp, directAnswerCandidates, selectedDirect } =
        await import('/src/features/p006/direct.ts');
      const scope = {
        serverIp: '8.8.8.8',
        localIp: local,
        privatePeerIps: [local],
        publishPrivateLocal: true,
        expiresAt: Math.floor(Date.now() / 1000) + 180,
      };
      const pc = new RTCPeerConnection({
        iceTransportPolicy: 'all',
        bundlePolicy: 'max-bundle',
        iceServers: [],
      });
      const video = document.createElement('video');
      video.autoplay = true;
      video.muted = true;
      document.body.append(video);
      let error = null;
      const control = new ControlClient(pc, session, (e) => {
        error = e;
      });
      pc.ontrack = (e) => {
        video.srcObject = e.streams[0] ?? new MediaStream([e.track]);
        void video.play();
      };
      const transceiver = pc.addTransceiver('video', { direction: 'recvonly' });
      const codecs = RTCRtpReceiver.getCapabilities('video').codecs.filter(
        (c) =>
          c.mimeType.toLowerCase() === 'video/h264' &&
          c.sdpFmtpLine?.includes('profile-level-id=42001f') &&
          c.sdpFmtpLine.includes('packetization-mode=1'),
      );
      transceiver.setCodecPreferences(codecs);
      await pc.setLocalDescription(await pc.createOffer());
      await new Promise((resolve, reject) => {
        if (pc.iceGatheringState === 'complete') return resolve();
        const t = setTimeout(
          () => reject(new Error('LOCAL_GATHER_TIMEOUT')),
          5000,
        );
        pc.onicegatheringstatechange = () => {
          if (pc.iceGatheringState === 'complete') {
            clearTimeout(t);
            resolve();
          }
        };
      });
      const offer = directSdp(pc.localDescription.sdp, scope);
      const answer = await window.sessionAnswer(offer.sdp);
      const remote = directAnswerCandidates(answer, scope);
      await pc.setRemoteDescription({ type: 'answer', sdp: answer });
      const start = performance.now();
      let first = null,
        last = null;
      window.sessionFixture = {
        pc,
        video,
        control,
        get error() {
          return error;
        },
        async snapshot() {
          last = await receiveStats(pc, video, start, first);
          const pair = selectedDirect(
            await pc.getStats(),
            offer.candidates,
            remote,
          );
          if (first === null && (last.framesDecoded ?? 0) > 0)
            first = last.elapsedMs;
          return {
            ...last,
            connectionPath: pair.path,
            peerMatches: pair.verified,
            control: control.snapshot(),
            error,
          };
        },
      };
      window.sessionFixture.timer = setInterval(async () => {
        const s = await window.sessionFixture.snapshot();
        control.beginBatch(
          (s.framesDecoded ?? 0) > 0 && !video.paused && s.peerMatches === true,
        );
      }, 50);
    },
    { local, session },
  );
  await page.waitForFunction(
    () => window.sessionFixture?.control.snapshot().ackReceived === 100,
    {},
    { timeout: 20000 },
  );
  const during = await page.evaluate(() => window.sessionFixture.snapshot());
  if (scenario === 'sender-stop') child.stdin.write('stop\n');
  if (scenario === 'receiver-stop')
    await page.evaluate(async () => {
      await window.sessionFixture.control.close();
      window.sessionFixture.pc.close();
    });
  if (scenario === 'receiver-close') await page.close();
  const exitCode = await bounded(nativeExit, 25000, 'NATIVE_FINAL_TIMEOUT');
  const after = page.isClosed()
    ? during
    : await page.evaluate(() => window.sessionFixture.snapshot());
  const browserClosed =
    page.isClosed() ||
    (await page.evaluate(async () => {
      const f = window.sessionFixture;
      clearInterval(f.timer);
      await f.control.close();
      f.pc.getReceivers().forEach((r) => r.track.stop());
      f.pc.close();
      f.video.srcObject = null;
      return f.pc.connectionState === 'closed';
    }));
  const control = after.control;
  const expectedConnectionFailure =
    scenario === 'receiver-close' &&
    exitCode === 1 &&
    ['PEER_CONNECTION_FAILED', 'CONTROL_CHANNEL_ERROR'].includes(
      nativeFinal?.error,
    );
  const expectedExit =
    expectedConnectionFailure ||
    (exitCode === 0 && !nativeError && nativeFinal?.error === null);
  const passed =
    expectedExit &&
    browserClosed &&
    nativeFinal?.resources_released &&
    nativeFinal.source_kind === 'synthetic_nv12' &&
    nativeFinal.native_path?.path === 'direct' &&
    nativeFinal.control?.accepted_moves === 100 &&
    nativeFinal.control?.acks_sent === 100 &&
    nativeFinal.control?.task_joined === true &&
    control.sentMoves === 100 &&
    control.ackReceived === 100 &&
    control.pending === 0 &&
    control.anomalies === 0 &&
    during.videoWidth === 1280 &&
    during.videoHeight === 720 &&
    (during.framesDecoded ?? 0) > 0 &&
    during.localCandidateType === 'host' &&
    during.remoteCandidateType === 'host' &&
    (scenario === 'auto'
      ? nativeFinal.frames_sent === 300
      : nativeFinal.frames_sent > 0 && nativeFinal.frames_sent < 450);
  const result = {
    scenario,
    scope:
      'one Windows PC, real Rust Session/MF/RTP/Edge and control.v1; synthetic pixels, no desktop or cloud',
    browserTestOnlyFakeDevicePermission: true,
    native: nativeFinal,
    browserDuring: during,
    browserFinal: after,
    browserClosed,
    expectedConnectionFailure,
    exitCode,
    passed: Boolean(passed),
  };
  const data = JSON.stringify(result, null, 2) + '\n';
  if (data.includes(local) || data.includes(session) || data.includes('"sdp":'))
    throw new Error('LOCAL_VIDEO_REPORT_PRIVATE_DATA');
  mkdirSync('artifacts/p006c-video/local', { recursive: true });
  writeFileSync(`artifacts/p006c-video/local/${scenario}.json`, data);
  console.log(`${scenario}: ${passed ? 'PASS' : 'FAIL'}`);
  if (!passed) process.exitCode = 1;
} finally {
  if (child && !exited) {
    child.stdin.write('stop\n');
    child.stdin.end();
    await new Promise((r) => setTimeout(r, 1000));
    if (!exited) child.kill();
  }
  await browser?.close();
  await vite?.close();
}
