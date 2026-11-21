// Offline regressions and default-denial tests; does not read an approved endpoint or capture.
import { spawnSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
const dir = process.env.REMOTEHUB_VERIFY_DIR ?? 'artifacts/p006c/automatic';
process.env.REMOTEHUB_VERIFY_DIR = dir;
await import('./turn-lifecycle-checks.mjs');
if (!process.exitCode) {
  const results = JSON.parse(readFileSync(`${dir}/results.json`, 'utf8'));
  const binary = `target/probe/ice-path-probe${process.platform === 'win32' ? '.exe' : ''}`;
  const steps = [
    [
      'path-probe-build',
      'cargo',
      [
        'build',
        '-p',
        'hub-webrtc',
        '--bin',
        'ice-path-probe',
        '--profile',
        'probe',
        '--features',
        'p006-direct-probe',
        '--locked',
        '--offline',
      ],
      0,
    ],
    ['path-probe-help', binary, ['--help'], 0],
    ['path-default-denied', binary, [], 2],
    [
      'path-placeholder-denied',
      binary,
      [
        '--approved-target',
        'deploy/coturn/probe-input.json.example',
        '--case',
        'direct',
      ],
      1,
    ],
    ['path-case-without-approval-denied', binary, ['--case', 'fallback'], 2],
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
