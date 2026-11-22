// P1 foundation checks reuse the existing non-capture safety suite; no cloud configuration is loaded.
import { spawnSync } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { root } from './quality/build-matrix.mjs';

process.chdir(root);
const dir = 'artifacts/p101/automatic';
process.env.REMOTEHUB_VERIFY_DIR = dir;
mkdirSync(dir, { recursive: true });
const foundation = [];
let inherited = [];
function record(passed, state) {
  writeFileSync(
    'artifacts/p101/quality-results.json',
    JSON.stringify(
      {
        recordedAt: new Date().toISOString(),
        scope:
          'Offline build and safety regression; no cloud, desktop capture, injected input or two-PC media',
        foundation,
        inherited,
        passed,
        state,
        applicationMediaStarted: false,
        newPhysicalTwoPcTest: 'NOT_RUN',
      },
      null,
      2,
    ) + '\n',
  );
}
// A failed rerun must not leave the previous successful summary looking current.
writeFileSync(`${dir}/results.json`, '[]\n');
record(false, 'running');
function run(name, command, args) {
  const r = spawnSync(command, args, {
    encoding: 'utf8',
    maxBuffer: 16 * 1024 * 1024,
  });
  writeFileSync(
    `${dir}/${name}.txt`,
    `${r.stdout ?? ''}${r.stderr ?? ''}\nEXIT=${r.status}\n`,
  );
  foundation.push({
    name,
    command: [command, ...args].join(' '),
    exitCode: r.status,
    expectedExitCode: 0,
  });
  writeFileSync(
    `${dir}/foundation-results.json`,
    JSON.stringify(foundation, null, 2) + '\n',
  );
  console.log(`${name}: ${r.status === 0 ? 'PASS' : 'FAIL'}`);
  if (r.status !== 0) throw new Error('P101_CHECK_FAILED');
}

try {
  if (process.argv.length !== 2) throw new Error('P101_ARGUMENTS_REJECTED');
  run('build-matrix-unit', process.execPath, [
    '--test',
    'scripts/quality/build-matrix.test.mjs',
  ]);
  run('build-matrix-resolved', process.execPath, [
    'scripts/quality/build-matrix.mjs',
  ]);
  await import('./p007b-checks.mjs');
  inherited = JSON.parse(readFileSync(`${dir}/results.json`, 'utf8'));
  if (process.exitCode) throw new Error('P101_EXISTING_SAFETY_SUITE_FAILED');
  for (const [name, args] of [
    ['default-check', ['check', '--workspace']],
    ['default-unit', ['test', '--workspace']],
    [
      'default-native-build',
      [
        'build',
        '-p',
        'remotehub-desktop',
        '--profile',
        'probe',
        '--features',
        'custom-protocol',
      ],
    ],
    [
      'turn-desktop-check',
      ['check', '-p', 'remotehub-desktop', '--features', 'p006-turn-probe'],
    ],
  ])
    run(name, 'cargo', [...args, '--locked', '--offline']);
  record(true, 'complete');
} catch {
  record(false, 'failed');
  console.error('P101_CHECKS_FAILED_SEE_LOCAL_RESULTS');
  process.exitCode = 1;
}
