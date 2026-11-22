// Actual native TLS/HMAC clients + actual Session + shipping frontend class, local synthetic only.
import { createRequire } from 'node:module';
import { pathToFileURL } from 'node:url';
import { spawn, spawnSync } from 'node:child_process';
import { createInterface } from 'node:readline';
import { randomBytes } from 'node:crypto';
import { networkInterfaces } from 'node:os';
import { createServer as tcpServer, connect } from 'node:net';
import {
  mkdirSync,
  mkdtempSync,
  writeFileSync,
  unlinkSync,
  rmdirSync,
  readdirSync,
  realpathSync,
  readFileSync,
  statSync,
} from 'node:fs';
import path from 'node:path';
const args = process.argv.slice(2);
const cloud = args[0] === '--approved-wss';
if (
  cloud
    ? args.length !== 6 ||
      args[2] !== '--local' ||
      args[4] !== '--case' ||
      !['auto', 'fallback', 'sender-stop', 'receiver-stop'].includes(args[5])
    : args.length !== 4 ||
      args[0] !== '--local' ||
      args[2] !== '--case' ||
      ![
        'auto',
        'sender-stop',
        'receiver-stop',
        'stats-failure',
        'wss-disconnect',
      ].includes(args[3])
)
  throw new Error('EXPLICIT_WSS_CASE_AND_SCOPE_REQUIRED');
const local = cloud ? args[3] : args[1],
  scenario = cloud ? args[5] : args[3];
const shortLocal = process.env.REMOTEHUB_R06_SHORT_LOCAL;
if (shortLocal !== undefined && (cloud || shortLocal !== '1'))
  throw new Error('R06_SHORT_FIXTURE_LOCAL_ONLY');
const seconds = shortLocal
  ? 8
  : ['auto', 'fallback'].includes(scenario)
    ? 10
    : 15;
const startedAt = new Date().toISOString();
if (
  !Object.values(networkInterfaces())
    .flat()
    .some((a) => a?.family === 'IPv4' && !a.internal && a.address === local)
)
  throw new Error('LOCAL_INTERFACE_NOT_ASSIGNED');
const root = process.cwd(),
  privateParent = path.resolve('artifacts/private');
mkdirSync(privateParent, { recursive: true });
const privateDir = cloud
  ? undefined
  : mkdtempSync(path.join(privateParent, 'p006c-wss-'));
const require = createRequire(path.resolve('apps/desktop/package.json'));
const { chromium } = require('@playwright/test');
const { createServer } = await import(
  pathToFileURL(require.resolve('vite')).href
);
async function bounded(p, ms, code) {
  let timer;
  try {
    return await Promise.race([
      p,
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error(code)), ms);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
let service,
  native,
  browser,
  page,
  vite,
  exited = false,
  nativeFinal,
  serverExited = false,
  secretValues = [];
try {
  if (!cloud) {
    const reserve = tcpServer();
    await new Promise((r) => reserve.listen(0, '127.0.0.1', r));
    const port = reserve.address().port;
    await new Promise((r) => reserve.close(r));
    const python = process.env.REMOTEHUB_PYTHON ?? 'python';
    const prepared = spawnSync(
      python,
      [
        'scripts/p006c-local-tls.py',
        '--dir',
        privateDir,
        '--port',
        String(port),
        '--deps',
        process.env.REMOTEHUB_PYTHON_DEPS ?? '.tools/ssh-libs',
      ],
      { encoding: 'utf8' },
    );
    if (prepared.status !== 0)
      throw new Error('LOCAL_TLS_FIXTURE_PREPARATION_FAILED');
    const config = JSON.parse(
      readFileSync(path.join(privateDir, 'client-private.json'), 'utf8'),
    );
    secretValues = [config.sender_invitation, config.receiver_invitation];
    service = spawn(
      path.resolve('target/probe/hub-rendezvous-probe.exe'),
      ['--private-config', path.join(privateDir, 'server-private.json')],
      {
        env: {
          ...process.env,
          REMOTEHUB_TURN_SHARED_SECRET: randomBytes(32).toString('hex'),
        },
        stdio: ['ignore', 'pipe', 'pipe'],
      },
    );
    service.once('exit', () => {
      serverExited = true;
    });
    await bounded(
      (async () => {
        for (;;) {
          if (serverExited) throw new Error('LOCAL_TLS_SERVICE_EXITED');
          const ok = await new Promise((resolve) => {
            const s = connect({ host: '127.0.0.1', port });
            s.once('connect', () => {
              s.destroy();
              resolve(true);
            });
            s.once('error', () => resolve(false));
          });
          if (ok) break;
          await pause(50);
        }
      })(),
      5000,
      'LOCAL_TLS_START_TIMEOUT',
    );
  } else {
    const config = JSON.parse(readFileSync(args[1], 'utf8'));
    secretValues = [
      config.sender_invitation,
      config.receiver_invitation,
      config.server_ipv4,
    ];
  }
  vite = await createServer({
    root: path.resolve('apps/desktop'),
    server: { host: '127.0.0.1', port: 0 },
    logLevel: 'error',
  });
  await vite.listen();
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
  page = await context.newPage();
  await page.goto(`http://127.0.0.1:${vite.httpServer.address().port}`);
  await page.evaluate(async () => {
    const fake = await navigator.mediaDevices.getUserMedia({
      video: true,
      audio: false,
    });
    fake.getTracks().forEach((t) => t.stop());
  });
  let readyResolve, readyReject;
  const ready = new Promise((r, j) => {
    readyResolve = r;
    readyReject = j;
  });
  ready.catch(() => {});
  native = spawn(
    path.resolve('target/probe/authenticated-session-probe.exe'),
    [
      '--synthetic',
      cloud ? '--approved-wss' : '--private-local-wss',
      cloud ? args[1] : path.join(privateDir, 'client-private.json'),
      '--approved-local',
      local,
      '--seconds',
      String(seconds),
      ...(scenario === 'fallback' ? ['--drop-direct'] : []),
    ],
    { stdio: ['pipe', 'pipe', 'pipe'] },
  );
  const pending = new Map();
  const evidenceFiles = [];
  let next = 0,
    nativeError = '';
  native.stderr.on('data', (b) => {
    if (nativeError.length < 8192) nativeError += b.toString();
  });
  const nativeExit = new Promise((resolve) =>
    native.once('exit', (code) => {
      exited = true;
      resolve(code);
      const safe = /^[A-Z_0-9]+$/.test(nativeError.trim())
        ? nativeError.trim()
        : 'LOCAL_NATIVE_EXITED';
      readyReject(new Error(safe));
      for (const { reject } of pending.values()) reject(new Error(safe));
      pending.clear();
    }),
  );
  createInterface({ input: native.stdout }).on('line', (line) => {
    const e = JSON.parse(line);
    if (e.event === 'private_rpc_ready') readyResolve();
    else if (e.event === 'private_rpc_response') {
      if (cloud && e.value?.ice?.ice_servers) {
        const labels = Object.fromEntries(
          e.value.ice.ice_servers.map((s) => [
            s.username,
            `${scenario}-${startedAt.replace(/[^0-9]/g, '')}`,
          ]),
        );
        writeFileSync(
          path.join(privateParent, 'p006c-video-cloud-observer-labels.json'),
          JSON.stringify({ labels }),
        );
      }
      const p = pending.get(e.id);
      if (p) {
        pending.delete(e.id);
        if (e.error) p.reject(new Error(e.error));
        else {
          if (
            typeof e.value === 'string' &&
            /^[0-9a-f-]+-receiver-(pre-close|final)\.json$/.test(e.value)
          )
            evidenceFiles.push(e.value);
          p.resolve(e.value);
        }
      }
    } else if (e.event === 'native_final') nativeFinal = e;
  });
  function rpc(command, args = {}) {
    const id = ++next;
    const p = new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject });
      native.stdin.write(JSON.stringify({ id, command, args }) + '\n');
    });
    return bounded(p, 25000, 'LOCAL_RPC_RESPONSE_TIMEOUT');
  }
  await bounded(ready, 10000, 'LOCAL_NATIVE_PAIR_TIMEOUT');
  const senderEvidenceDenial = await rpc('test_sender_report_denial');
  if (senderEvidenceDenial.denial !== 'RECEIVER_EVIDENCE_WRONG_ROLE')
    throw new Error('LOCAL_REPORT_ROLE_CHECK');
  let foreignOwnerRejected = false;
  try {
    await rpc('p006_save', {
      sessionId: 'foreign-owner',
      receiverEvidence: {},
    });
  } catch (e) {
    foreignOwnerRejected = e.message === 'LOCAL_RPC_OWNER';
  }
  if (!foreignOwnerRejected) throw new Error('LOCAL_REPORT_OWNER_CHECK');
  await page.exposeFunction('nativeInvoke', rpc);
  await page.evaluate(
    async ({ local, seconds, cloud }) => {
      const { RelayProbe } = await import('/src/features/p006/relay.ts');
      const video = document.createElement('video');
      video.muted = true;
      video.autoplay = true;
      document.body.append(video);
      const state = {
        states: [],
        error: null,
        saved: null,
        receiver: null,
        status: null,
        pc: null,
      };
      window.wssFixture = state;
      const probe = new RelayProbe(
        video,
        (s, n, r) => {
          state.states.push(s);
          if (n) state.status = n;
          if (r) state.receiver = r;
        },
        (e) => {
          state.error ??= e;
        },
        () => {},
        (p) => {
          state.saved = p;
        },
        true,
        (command, args) => window.nativeInvoke(command, args),
        // The local operator fixture has no TURN server. Preserve All and use only
        // host candidates; normal desktop code uses the unmodified default factory.
        (options) => {
          state.pc = new RTCPeerConnection(
            cloud ? options : { ...options, iceServers: [] },
          );
          return state.pc;
        },
      );
      state.probe = probe;
      void probe
        .start({
          role: 'receiver',
          consent: true,
          localIp: local,
          seconds,
          mode: 'direct_preferred',
          peerIp: local,
        })
        .catch((e) => {
          state.error ??= String(e);
        });
    },
    {
      local,
      seconds,
      cloud,
    },
  );
  await page.waitForFunction(
    () =>
      window.wssFixture?.receiver?.control?.ackReceived === 100 ||
      window.wssFixture?.error ||
      ['stopped', 'failed'].includes(window.wssFixture?.states.at(-1)),
    {},
    { timeout: cloud ? 75000 : 25000 },
  );
  const during = await page.evaluate(() => ({
    receiver: window.wssFixture.receiver,
    status: window.wssFixture.status,
    error: window.wssFixture.error,
  }));
  if (during.error) throw new Error('LOCAL_FRONTEND_FAILED');
  if (scenario === 'stats-failure')
    await page.evaluate(() => {
      window.wssFixture.pc.getStats = () =>
        Promise.reject(new Error('DIAG_GETSTATS_FAILURE'));
    });
  if (scenario === 'wss-disconnect') {
    service.kill();
    await bounded(
      new Promise((r) => service.once('exit', r)),
      3000,
      'LOCAL_WSS_DISCONNECT_TIMEOUT',
    );
  }
  if (scenario === 'sender-stop') await rpc('sender_cancel');
  if (scenario === 'receiver-stop')
    await page.evaluate(() => window.wssFixture.probe.stop());
  await page.waitForFunction(
    () =>
      Boolean(window.wssFixture?.saved) &&
      window.wssFixture.pc.connectionState === 'closed' &&
      ['stopped', 'failed'].includes(window.wssFixture.states.at(-1)),
    {},
    { timeout: 20000 },
  );
  const final = await page.evaluate(() => ({
    states: window.wssFixture.states,
    error: window.wssFixture.error,
    saved: Boolean(window.wssFixture.saved),
    receiver: window.wssFixture.receiver,
    status: window.wssFixture.status,
    browserClosed: window.wssFixture.pc.connectionState === 'closed',
  }));
  native.stdin.write(
    JSON.stringify({ id: ++next, command: 'finish', args: {} }) + '\n',
  );
  native.stdin.end();
  const exitCode = await bounded(
    nativeExit,
    18000,
    'LOCAL_NATIVE_CLEANUP_TIMEOUT',
  );
  const source = nativeFinal?.sender?.source,
    control = during.receiver?.control;
  const evidenceDirectory = path.join(
    cloud ? path.dirname(path.resolve(args[1])) : privateDir,
    'receiver-evidence',
  );
  const receiverEvidence = evidenceFiles.map((name) =>
    JSON.parse(readFileSync(path.join(evidenceDirectory, name), 'utf8')),
  );
  const evidencePassed =
    receiverEvidence.length === 2 &&
    receiverEvidence[0].stage === 'pre_close' &&
    receiverEvidence[1].stage === 'cleanup' &&
    receiverEvidence[0].capture ===
      (scenario === 'stats-failure' ? 'last_known' : 'live') &&
    receiverEvidence[1].cleanup.peerClosed === true &&
    receiverEvidence[1].cleanup.nativeResourcesReleased === true;
  const passed =
    evidencePassed &&
    (scenario === 'wss-disconnect' ? exitCode === 1 : exitCode === 0) &&
    (scenario === 'wss-disconnect' || !nativeError) &&
    source?.resources_released &&
    nativeFinal.sender.authenticated &&
    nativeFinal.receiver.authenticated &&
    nativeFinal.receiver.resources_released &&
    source.source_kind === 'synthetic_nv12' &&
    source.ice_transport_policy === 'all' &&
    ['direct', 'relay'].includes(source.native_path?.path) &&
    source.control?.accepted_moves === 100 &&
    source.control?.acks_sent === 100 &&
    source.control?.task_joined &&
    control?.sentMoves === 100 &&
    control.ackReceived === 100 &&
    control.pending === 0 &&
    control.anomalies === 0 &&
    during.receiver.connectionPath === source.native_path.path &&
    during.receiver.peerMatches === true &&
    during.receiver.videoWidth === 1280 &&
    during.receiver.videoHeight === 720 &&
    during.receiver.framesDecoded > 0 &&
    final.browserClosed &&
    final.saved &&
    (scenario === 'stats-failure'
      ? final.error === 'Error: DIAG_GETSTATS_FAILURE'
      : scenario === 'wss-disconnect'
        ? Boolean(final.error)
        : !final.error) &&
    (['auto', 'fallback'].includes(scenario)
      ? source.frames_sent === seconds * 30 &&
        source.stream.frames_input === seconds * 30 &&
        source.stream.frames_encoded === seconds * 30 &&
        final.receiver.framesReceived === seconds * 30 &&
        final.receiver.framesDecoded === seconds * 30 &&
        final.receiver.framesRendered === seconds * 30
      : source.frames_sent > 0 && source.frames_sent < seconds * 30) &&
    (scenario !== 'fallback' ||
      (source.native_path.path === 'relay' &&
        source.diagnostic_transport?.drop_direct &&
        source.diagnostic_transport.dropped_packets > 0)) &&
    (scenario === 'fallback' || source.native_path.path === 'direct');
  const result = {
    scenario,
    sourceDurationSeconds: seconds,
    startedAt,
    finishedAt: new Date().toISOString(),
    scope: cloud
      ? 'one Windows PC; approved cloud WSS/TURN + real native clients/Session/MF/RTP/Edge/control.v1; synthetic video, not two physical PCs or desktop capture'
      : 'one Windows PC; real local TLS server + native WSS clients + Session MF/RTP + shared frontend RelayProbe/Edge + control.v1; generated video, no cloud/TURN/desktop',
    browserTestOnlyFakeDevicePermission: true,
    localTestPcFactoryRemovedIceServers: !cloud,
    native: nativeFinal,
    browserDuring: during,
    browserFinal: final,
    receiverEvidence,
    securityReportChecks: {
      senderRoleDenied: true,
      foreignOwnerDenied: foreignOwnerRejected,
    },
    exitCode,
    passed: Boolean(passed),
  };
  const data = JSON.stringify(result, null, 2) + '\n';
  if (
    [local, ...secretValues].some((s) => data.includes(s)) ||
    data.includes('"sdp":')
  )
    throw new Error('LOCAL_WSS_EVIDENCE_PRIVATE_DATA');
  const out =
    process.env.REMOTEHUB_LOCAL_EVIDENCE_DIR ??
    (cloud ? 'artifacts/p006c-video/cloud' : 'artifacts/p006c-video/wss-local');
  mkdirSync(out, { recursive: true });
  writeFileSync(`${out}/${scenario}.json`, data);
  console.log(`${scenario} WSS/video: ${passed ? 'PASS' : 'FAIL'}`);
  if (!passed) process.exitCode = 1;
} catch (error) {
  const partial = await page
    ?.evaluate(() => {
      const s = window.wssFixture;
      return s
        ? {
            states: s.states,
            error: s.error,
            receiver: s.receiver,
            status: s.status,
          }
        : null;
    })
    .catch(() => null);
  const code = String(error?.message ?? error);
  const failureCode = /^[A-Z_0-9]+$/.test(code)
    ? code
    : 'LOCAL_CASE_TIMEOUT_OR_RUNNER_ERROR';
  const out =
    process.env.REMOTEHUB_LOCAL_EVIDENCE_DIR ??
    (cloud ? 'artifacts/p006c-video/cloud' : 'artifacts/p006c-video/wss-local');
  const data =
    JSON.stringify(
      {
        scenario,
        startedAt,
        finishedAt: new Date().toISOString(),
        passed: false,
        scope:
          'local/cloud case failed before final assertion; not success evidence',
        failureCode,
        browserPartial: partial,
        native: nativeFinal ?? null,
      },
      null,
      2,
    ) + '\n';
  if (
    [local, ...secretValues].some((s) => data.includes(s)) ||
    data.includes('"sdp":')
  )
    throw new Error('LOCAL_WSS_FAILED_EVIDENCE_PRIVATE_DATA');
  mkdirSync(out, { recursive: true });
  writeFileSync(`${out}/${scenario}.json`, data);
  console.log(`${scenario} WSS/video: FAIL (${failureCode})`);
  process.exitCode = 1;
} finally {
  if (native && !exited) {
    native.stdin.end();
    await pause(500);
    if (!exited) native.kill();
  }
  await browser?.close();
  await vite?.close();
  if (service && !serverExited) {
    service.kill();
    await bounded(
      new Promise((r) => service.once('exit', r)),
      3000,
      'LOCAL_TLS_STOP_TIMEOUT',
    );
  }
  if (privateDir) {
    const checked = realpathSync(privateDir);
    if (!checked.startsWith(privateParent + path.sep) || checked !== privateDir)
      throw new Error('PRIVATE_CLEANUP_SCOPE');
    for (const name of readdirSync(privateDir)) {
      if (name === 'receiver-evidence') {
        const directory = path.join(privateDir, name);
        if (
          realpathSync(directory) !== directory ||
          !statSync(directory).isDirectory()
        )
          throw new Error('PRIVATE_EVIDENCE_CLEANUP_SCOPE');
        for (const file of readdirSync(directory)) {
          if (!/^[0-9a-f-]+-receiver-(pre-close|final)\.json$/.test(file))
            throw new Error('PRIVATE_EVIDENCE_CLEANUP_FILE');
          unlinkSync(path.join(directory, file));
        }
        rmdirSync(directory);
        continue;
      }
      if (
        ![
          'cert.pem',
          'key.pem',
          'server-private.json',
          'client-private.json',
        ].includes(name)
      )
        throw new Error('PRIVATE_CLEANUP_UNKNOWN_FILE');
      unlinkSync(path.join(privateDir, name));
    }
    rmdirSync(privateDir);
  }
}
