import { spawnSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
const args = process.argv.slice(2);
if (args.length !== 2 || args[0] !== '--local')
  throw new Error('EXPLICIT_LOCAL_INTERFACE_REQUIRED');
const results = [];
mkdirSync('artifacts/p006c-video/local', { recursive: true });
for (const scenario of [
  'auto',
  'sender-stop',
  'receiver-stop',
  'receiver-close',
]) {
  const r = spawnSync(
    process.execPath,
    ['scripts/p006c-video-local.mjs', ...args, '--case', scenario],
    { encoding: 'utf8', timeout: 75000, maxBuffer: 1024 * 1024 },
  );
  process.stdout.write(r.stdout ?? '');
  process.stderr.write(r.stderr ?? '');
  results.push({
    scenario,
    exitCode: r.status,
    passed: r.status === 0 && !r.error,
  });
  writeFileSync(
    'artifacts/p006c-video/local/results.json',
    JSON.stringify(results, null, 2) + '\n',
  );
  if (r.status !== 0 || r.error) {
    process.exitCode = 1;
    break;
  }
}
