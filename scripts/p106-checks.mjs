// Default tray/exit and existing experimental safety regressions, without cloud or media starts.
import { runFoundation } from './p101-checks.mjs';
import { spawnSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { audit } from './quality/p106-permissions.mjs';
await runFoundation('artifacts/p106');
if (!process.exitCode) {
  const result = spawnSync(
    process.execPath,
    ['--test', 'scripts/quality/p106-permissions.test.mjs'],
    { encoding: 'utf8' },
  );
  writeFileSync(
    'artifacts/p106/permission-tests.txt',
    `${result.stdout ?? ''}${result.stderr ?? ''}\nEXIT=${result.status}\n`,
  );
  process.exitCode = result.status ?? 1;
  if (!process.exitCode)
    writeFileSync(
      'artifacts/p106/permissions.json',
      JSON.stringify(audit(), null, 2) + '\n',
    );
}
