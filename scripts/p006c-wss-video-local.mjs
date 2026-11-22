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
      !['auto', 'sender-stop', 'receiver-stop'].includes(args[3])
)
  throw new Error('EXPLICIT_WSS_CASE_AND_SCOPE_REQUIRED');
const local = cloud ? args[3] : args[1],
  scenario = cloud ? args[5] : args[3];
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
  const page = await context.newPage();
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
      ['auto', 'fallback'].includes(scenario) ? '10' : '15',
      ...(scenario === 'fallback' ? ['--drop-direct'] : []),
    ],
    { stdio: ['pipe', 'pipe', 'pipe'] },
  );
  const pending = new Map();
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
        else p.resolve(e.value);
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
      seconds: ['auto', 'fallback'].includes(scenario) ? 10 : 15,
      cloud,
    },
  );
  await page.waitForFunction(
    () =>
      window.wssFixture?.receiver?.control?.ackReceived === 100 ||
      window.wssFixture?.error,
    {},
    { timeout: cloud ? 75000 : 25000 },
  );
  const during = await page.evaluate(() => ({
    receiver: window.wssFixture.receiver,
    status: window.wssFixture.status,
    error: window.wssFixture.error,
  }));
  if (during.error) throw new Error('LOCAL_FRONTEND_FAILED');
  if (scenario === 'sender-stop') await rpc('sender_cancel');
  if (scenario === 'receiver-stop')
    await page.evaluate(() => window.wssFixture.probe.stop());
  await page.waitForFunction(
    () =>
      Boolean(window.wssFixture?.saved) || Boolean(window.wssFixture?.error),
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
  const passed =
    exitCode === 0 &&
    !nativeError &&
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
    !final.error &&
    (['auto', 'fallback'].includes(scenario)
      ? source.frames_sent === 300 &&
        source.stream.frames_input === 300 &&
        source.stream.frames_encoded === 300 &&
        final.receiver.framesReceived === 300 &&
        final.receiver.framesDecoded === 300 &&
        final.receiver.framesRendered === 300
      : source.frames_sent > 0 && source.frames_sent < 450) &&
    (scenario !== 'fallback' ||
      (source.native_path.path === 'relay' &&
        source.diagnostic_transport?.drop_direct &&
        source.diagnostic_transport.dropped_packets > 0)) &&
    (scenario === 'fallback' || source.native_path.path === 'direct');
  const result = {
    scenario,
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
    exitCode,
    passed: Boolean(passed),
  };
  const data = JSON.stringify(result, null, 2) + '\n';
  if (
    [local, ...secretValues].some((s) => data.includes(s)) ||
    data.includes('"sdp":')
  )
    throw new Error('LOCAL_WSS_EVIDENCE_PRIVATE_DATA');
  const out = cloud
    ? 'artifacts/p006c-video/cloud'
    : 'artifacts/p006c-video/wss-local';
  mkdirSync(out, { recursive: true });
  writeFileSync(`${out}/${scenario}.json`, data);
  console.log(`${scenario} WSS/video: ${passed ? 'PASS' : 'FAIL'}`);
  if (!passed) process.exitCode = 1;
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
