// Full offline regression; never loads private WSS config, starts cloud services or captures.
import { spawnSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
const dir =
  process.env.REMOTEHUB_VERIFY_DIR ?? 'artifacts/p006c-video/automatic';
process.env.REMOTEHUB_VERIFY_DIR = dir;
await import('./p006c-checks.mjs');
if (!process.exitCode) {
  const results = JSON.parse(readFileSync(`${dir}/results.json`, 'utf8'));
  const video = 'target/probe/video-session-probe.exe',
    wss = 'target/probe/authenticated-session-probe.exe';
  const steps = [
    [
      'direct-production-clippy',
      'cargo',
      [
        'clippy',
        '-p',
        'remotehub-desktop',
        '--all-targets',
        '--features',
        'p006-direct',
        '--locked',
        '--offline',
        '--',
        '-D',
        'warnings',
      ],
      0,
    ],
    [
      'direct-desktop-build',
      'cargo',
      [
        'build',
        '-p',
        'remotehub-desktop',
        '--profile',
        'probe',
        '--features',
        'custom-protocol,p006-direct',
        '--locked',
        '--offline',
      ],
      0,
    ],
    [
      'video-probe-build',
      'cargo',
      [
        'build',
        '-p',
        'hub-webrtc',
        '--bin',
        'video-session-probe',
        '--profile',
        'probe',
        '--features',
        'p006-session-probe',
        '--locked',
        '--offline',
      ],
      0,
    ],
    [
      'authenticated-probe-build',
      'cargo',
      [
        'build',
        '-p',
        'hub-webrtc',
        '--bin',
        'authenticated-session-probe',
        '--profile',
        'probe',
        '--features',
        'p006-session-probe',
        '--locked',
        '--offline',
      ],
      0,
    ],
    [
      'wss-build',
      'cargo',
      [
        'build',
        '-p',
        'hub-rendezvous-probe',
        '--profile',
        'probe',
        '--locked',
        '--offline',
      ],
      0,
    ],
    ['video-help', video, ['--help'], 0],
    ['video-default-denied', video, [], 2],
    [
      'video-invalid-local-denied',
      video,
      ['--synthetic', '--approved-local', '127.0.0.1', '--seconds', '10'],
      1,
    ],
    ['authenticated-help', wss, ['--help'], 0],
    ['authenticated-default-denied', wss, [], 2],
    [
      'authenticated-missing-target-denied',
      wss,
      ['--synthetic', '--approved-wss'],
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
