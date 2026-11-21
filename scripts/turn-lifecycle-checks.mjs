// Offline regression only: never reads a usable target or starts a cloud test.
import { spawnSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
const dir =
  process.env.REMOTEHUB_VERIFY_DIR ?? 'artifacts/turn-lifecycle/automatic';
process.env.REMOTEHUB_VERIFY_DIR = dir;
await import('./p006b-checks.mjs');
if (!process.exitCode) {
  const results = JSON.parse(readFileSync(`${dir}/results.json`, 'utf8'));
  const probe = `target/probe/turn-allocate-probe${process.platform === 'win32' ? '.exe' : ''}`;
  const steps = [
    [
      'python-lifecycle-unit',
      process.env.REMOTEHUB_PYTHON ?? 'python',
      [
        '-m',
        'unittest',
        'discover',
        '-s',
        'scripts',
        '-p',
        'test_*turn*.py',
        '-v',
      ],
      0,
    ],
    [
      'turn-probe-build',
      'cargo',
      [
        'build',
        '-p',
        'hub-webrtc',
        '--bin',
        'turn-allocate-probe',
        '--profile',
        'probe',
        '--features',
        'p006-turn-probe',
        '--locked',
        '--offline',
      ],
      0,
    ],
    ['turn-probe-help', probe, ['--help'], 0],
    ['turn-default-denied', probe, [], 2],
    [
      'turn-placeholder-denied',
      probe,
      ['--approved-target', 'deploy/coturn/probe-input.json.example'],
      1,
    ],
    [
      'turn-connected-without-target-denied',
      probe,
      ['--connected-lifecycle'],
      2,
    ],
  ];
  for (const [name, command, args, expected] of steps) {
    const r = spawnSync(command, args, {
      encoding: 'utf8',
      maxBuffer: 10 * 1024 * 1024,
    });
    writeFileSync(
      `${dir}/${name}.txt`,
      `${r.stdout ?? ''}${r.stderr ?? ''}\nEXIT=${r.status}\n`,
    );
    const passed = r.status === expected;
    results.push({
      name,
      command: [command, ...args].join(' '),
      exitCode: r.status,
      expectedExitCode: expected,
      passed,
    });
    writeFileSync(
      `${dir}/results.json`,
      JSON.stringify(results, null, 2) + '\n',
    );
    console.log(`${name}: ${passed ? 'PASS' : 'FAIL'}`);
    if (!passed) {
      process.exitCode = 1;
      break;
    }
  }
}
