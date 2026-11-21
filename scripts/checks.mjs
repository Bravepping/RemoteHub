import { mkdirSync, writeFileSync, appendFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { join } from 'node:path';

const steps = [
  ['pnpm', ['lint']],
  ['pnpm', ['typecheck']],
  ['pnpm', ['test']],
  ['pnpm', ['build']],
  ['cargo', ['fmt', '--all', '--', '--check']],
  [
    'cargo',
    [
      'clippy',
      '--workspace',
      '--all-targets',
      '--locked',
      '--',
      '-D',
      'warnings',
    ],
  ],
  ['cargo', ['test', '--workspace', '--locked']],
];
const evidenceDir = process.env.REMOTEHUB_VERIFY_DIR ?? 'artifacts';
mkdirSync(evidenceDir, { recursive: true });
writeFileSync(
  join(evidenceDir, 'verification.txt'),
  `RemoteHub verification ${new Date().toISOString()}\n`,
);
const results = [];
for (const [command, args] of steps) {
  const title = `${command} ${args.join(' ')}`;
  // Windows pnpm is a .cmd shim. Commands/arguments here are fixed, never supplied by a network peer or UI.
  const result = spawnSync(command, args, {
    shell: process.platform === 'win32' && command === 'pnpm',
    encoding: 'utf8',
    maxBuffer: 10 * 1024 * 1024,
  });
  const output = `\n$ ${title}\n${result.stdout ?? ''}${result.stderr ?? ''}\nEXIT=${result.status}\n`;
  process.stdout.write(output);
  appendFileSync(join(evidenceDir, 'verification.txt'), output);
  results.push({
    command: title,
    exit_code: result.status,
    error: result.error?.message,
  });
  writeFileSync(
    join(evidenceDir, 'verification.json'),
    JSON.stringify(results, null, 2),
  );
  if (result.status !== 0) {
    process.exitCode = 1;
    break;
  }
}
