// Offline automatic checks. This runner never reads private config or captures a screen.
import { spawnSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
const dir = process.env.REMOTEHUB_VERIFY_DIR ?? 'artifacts/p006b/automatic';
mkdirSync(dir, { recursive: true });
const steps = [
  ['frontend-typecheck', 'pnpm', ['--dir', 'apps/desktop', 'typecheck']],
  ['frontend-unit', 'pnpm', ['--dir', 'apps/desktop', 'test']],
  ['frontend-build', 'pnpm', ['--dir', 'apps/desktop', 'build']],
  ['frontend-lint', 'pnpm', ['lint']],
  ['browser', 'pnpm', ['test:e2e']],
  ['fmt', 'cargo', ['fmt', '--all', '--', '--check']],
  [
    'clippy',
    'cargo',
    [
      'clippy',
      '--workspace',
      '--all-targets',
      '--all-features',
      '--locked',
      '--offline',
      '--',
      '-D',
      'warnings',
    ],
  ],
  [
    'default-clippy',
    'cargo',
    [
      'clippy',
      '--workspace',
      '--all-targets',
      '--locked',
      '--offline',
      '--',
      '-D',
      'warnings',
    ],
  ],
  [
    'test',
    'cargo',
    ['test', '--workspace', '--all-features', '--locked', '--offline'],
  ],
  [
    'legacy-test',
    'cargo',
    [
      'test',
      '-p',
      'remotehub-desktop',
      '--features',
      'p005-control',
      '--locked',
      '--offline',
    ],
  ],
  [
    'loopback-check',
    'cargo',
    [
      'check',
      '-p',
      'remotehub-desktop',
      '--features',
      'p004-probe',
      '--locked',
      '--offline',
    ],
  ],
  [
    'lan-check',
    'cargo',
    [
      'check',
      '-p',
      'remotehub-desktop',
      '--features',
      'p004-lan',
      '--locked',
      '--offline',
    ],
  ],
  [
    'build',
    'cargo',
    [
      'build',
      '-p',
      'remotehub-desktop',
      '--profile',
      'probe',
      '--features',
      'custom-protocol,p006-relay',
      '--locked',
      '--offline',
    ],
  ],
];
const results = [];
for (const [name, command, args] of steps) {
  const r = spawnSync(command, args, {
    encoding: 'utf8',
    shell: process.platform === 'win32' && command === 'pnpm',
    maxBuffer: 10 * 1024 * 1024,
  });
  writeFileSync(
    `${dir}/${name}.txt`,
    `${r.stdout ?? ''}${r.stderr ?? ''}\nEXIT=${r.status}\n`,
  );
  results.push({
    name,
    command: [command, ...args].join(' '),
    exitCode: r.status,
  });
  writeFileSync(`${dir}/results.json`, JSON.stringify(results, null, 2) + '\n');
  console.log(`${name}: ${r.status === 0 ? 'PASS' : 'FAIL'}`);
  if (r.status !== 0) {
    process.exitCode = 1;
    break;
  }
}
