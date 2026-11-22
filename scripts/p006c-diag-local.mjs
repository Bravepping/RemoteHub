// Explicit local synthetic-only suite. No private cloud config or desktop API.
import { spawnSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
const args = process.argv.slice(2);
if (args.length !== 2 || args[0] !== '--local')
  throw new Error('EXPLICIT_LOCAL_INTERFACE_REQUIRED');
const out = 'artifacts/p006c-diag/local';
mkdirSync(out, { recursive: true });
const results = [];
for (const scenario of [
  'auto',
  'sender-stop',
  'receiver-stop',
  'stats-failure',
  'wss-disconnect',
]) {
  const p = spawnSync(
    process.execPath,
    [
      'scripts/p006c-wss-video-local.mjs',
      '--local',
      args[1],
      '--case',
      scenario,
    ],
    {
      encoding: 'utf8',
      env: { ...process.env, REMOTEHUB_LOCAL_EVIDENCE_DIR: out },
      maxBuffer: 4 * 1024 * 1024,
    },
  );
  writeFileSync(
    `${out}/${scenario}-runner.txt`,
    (p.stdout ?? '') + (p.stderr ?? ''),
  );
  results.push({
    scenario,
    exitCode: p.status,
    scope:
      'one physical Windows; real localhost TLS/WSS/Session/MF/RTP/Edge/SCTP, generated pixels; no cloud/desktop/two-PC',
    finishedAt: new Date().toISOString(),
  });
  writeFileSync(`${out}/results.json`, JSON.stringify(results, null, 2) + '\n');
  console.log(`${scenario}: ${p.status === 0 ? 'PASS' : 'FAIL'}`);
  if (p.status !== 0) {
    process.exitCode = 1;
    break;
  }
}
