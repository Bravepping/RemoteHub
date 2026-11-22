// Single Phase3 regression output; real capture/input remain manual, never automatic here.
import { runFoundation } from './p101-checks.mjs';
import { spawnSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { audit } from './quality/p106-permissions.mjs';
await runFoundation('artifacts/p301');
if (!process.exitCode) {
  const results = [];
  for (const [name, command, args] of [
    [
      'product-clippy',
      'cargo',
      [
        'clippy',
        '-p',
        'remotehub-desktop',
        '--all-targets',
        '--features',
        'p3-lan',
        '--locked',
        '--offline',
        '--',
        '-D',
        'warnings',
      ],
    ],
    [
      'product-unit',
      'cargo',
      [
        'test',
        '-p',
        'remotehub-desktop',
        '--features',
        'p3-lan',
        '--locked',
        '--offline',
      ],
    ],
    [
      'product-webrtc',
      'cargo',
      [
        'test',
        '-p',
        'hub-webrtc',
        '--features',
        'p3-lan',
        '--locked',
        '--offline',
      ],
    ],
    [
      'product-build',
      'cargo',
      [
        'build',
        '-p',
        'remotehub-desktop',
        '--profile',
        'probe',
        '--features',
        'p3-lan,custom-protocol',
        '--locked',
        '--offline',
      ],
    ],
    [
      'permissions',
      process.execPath,
      ['--test', 'scripts/quality/p106-permissions.test.mjs'],
    ],
  ]) {
    const r = spawnSync(command, args, {
      encoding: 'utf8',
      maxBuffer: 16 * 1024 * 1024,
    });
    writeFileSync(
      `artifacts/p301/${name}.txt`,
      `${r.stdout ?? ''}${r.stderr ?? ''}\nEXIT=${r.status}\n`,
    );
    results.push({ name, exitCode: r.status });
    writeFileSync(
      'artifacts/p301/product-checks.json',
      JSON.stringify(results, null, 2) + '\n',
    );
    console.log(`${name}: ${r.status === 0 ? 'PASS' : 'FAIL'}`);
    if (r.status !== 0) {
      process.exitCode = 1;
      break;
    }
  }
  if (!process.exitCode)
    writeFileSync(
      'artifacts/p301/permissions.json',
      JSON.stringify(audit(), null, 2) + '\n',
    );
}
