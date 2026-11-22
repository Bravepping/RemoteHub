// Executes only the dedicated native Wry lifecycle example, never starts media or a product experiment.
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  mkdirSync,
  readFileSync,
  writeFileSync,
  mkdtempSync,
  rmSync,
} from 'node:fs';
import { resolve, relative, isAbsolute } from 'node:path';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../', import.meta.url));
const out = resolve(root, 'artifacts/p106/native');
mkdirSync(out, { recursive: true });
const exe = resolve(root, 'target/probe/examples/lifecycle_native.exe');
const programSha256 = createHash('sha256')
  .update(readFileSync(exe))
  .digest('hex');
const cases = [];
for (const name of [
  'default-close',
  'resident-menu-exit',
  'disable-then-close',
  'busy-close',
]) {
  const privateRoot = resolve(root, 'artifacts/private');
  mkdirSync(privateRoot, { recursive: true });
  const profile = mkdtempSync(resolve(privateRoot, 'p106-native-'));
  const startedAt = new Date().toISOString();
  const p = spawnSync(exe, [name], {
    cwd: root,
    env: { ...process.env, WEBVIEW2_USER_DATA_FOLDER: profile },
    encoding: 'utf8',
    maxBuffer: 2 * 1024 * 1024,
    timeout: 20000,
  });
  const stdout = p.stdout ?? '';
  const lines = stdout.split(/\r?\n/).filter((line) => line.startsWith('{'));
  let native;
  try {
    native = lines.length === 1 ? JSON.parse(lines[0]) : undefined;
  } catch {
    native = undefined;
  }
  // Report fixed failure metadata only; never echo arbitrary WebView/profile or native error text.
  const result = {
    case: name,
    startedAt,
    endedAt: new Date().toISOString(),
    exitCode: p.status,
    signal: p.signal,
    timedOut: p.error?.code === 'ETIMEDOUT',
    native,
    passed: p.status === 0 && native?.case === name && native?.passed === true,
    profileCleanup: 'not-attempted',
  };
  const within = relative(privateRoot, profile);
  if (within && !within.startsWith('..') && !isAbsolute(within)) {
    try {
      rmSync(profile, {
        recursive: true,
        force: true,
        maxRetries: 2,
        retryDelay: 100,
      });
      result.profileCleanup = 'removed';
    } catch {
      result.profileCleanup = 'locked-private-test-profile-retained';
    }
  }
  cases.push(result);
  writeFileSync(
    resolve(out, 'results.json'),
    JSON.stringify(
      {
        recordedAt: new Date().toISOString(),
        programSha256,
        source:
          'Real Windows Tauri/Wry/system tray API integration; isolated example, not manual menu clicks or media',
        runLoop:
          'run_return in test harness; production uses run with the same lifecycle callbacks',
        userConfirmedP105Window:
          'USER_CONFIRMED_WINDOW_VISIBLE (separate user evidence)',
        cases,
        passed: cases.length === 4 && cases.every((c) => c.passed),
        captureStarted: false,
        remoteInputExecuted: false,
        cloudContacted: false,
      },
      null,
      2,
    ) + '\n',
  );
  console.log(`${name}: ${result.passed ? 'PASS' : 'FAIL'}`);
  if (!result.passed) {
    process.exitCode = 1;
    break;
  }
}
