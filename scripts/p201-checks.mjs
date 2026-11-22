// Local identity plus the existing offline safety suite. Never starts the desktop or cloud.
import { runFoundation } from './p101-checks.mjs';
import { spawnSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { audit } from './quality/p106-permissions.mjs';
await runFoundation('artifacts/p201');
if (!process.exitCode) {
  const r = spawnSync(
    process.execPath,
    ['--test', 'scripts/quality/p106-permissions.test.mjs'],
    { encoding: 'utf8' },
  );
  writeFileSync(
    'artifacts/p201/permission-tests.txt',
    `${r.stdout ?? ''}${r.stderr ?? ''}\nEXIT=${r.status}\n`,
  );
  process.exitCode = r.status ?? 1;
  if (!process.exitCode)
    writeFileSync(
      'artifacts/p201/permissions.json',
      JSON.stringify(audit(), null, 2) + '\n',
    );
}
