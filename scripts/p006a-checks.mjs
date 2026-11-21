import { spawnSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';

// Offline checks only. This runner never supplies --approved-target to a usable config.
const dir = 'artifacts/p006a/final-checks';
mkdirSync(dir, { recursive: true });
const python =
  process.env.REMOTEHUB_PYTHON ??
  (process.platform === 'win32' ? 'python' : 'python3');
const bash = process.env.REMOTEHUB_BASH ?? 'bash';
const probe = `target/probe/turn-allocate-probe${process.platform === 'win32' ? '.exe' : ''}`;
const steps = [
  ['baseline-verify', 'pnpm', ['verify']],
  [
    'all-features-clippy',
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
    'all-features-test',
    'cargo',
    ['test', '--workspace', '--all-features', '--locked', '--offline'],
  ],
  ['edge-test', 'pnpm', ['test:e2e']],
  ...['p004-probe', 'p004-lan', 'p005-control', 'p006-turn-probe'].map(
    (feature) => [
      `${feature}-check`,
      'cargo',
      [
        'check',
        '-p',
        'remotehub-desktop',
        '--features',
        feature,
        '--locked',
        '--offline',
      ],
    ],
  ),
  [
    'desktop-default-build',
    'cargo',
    ['build', '-p', 'remotehub-desktop', '--locked', '--offline'],
  ],
  ['coturn-template', python, ['deploy/coturn/verify_config.py', '--template']],
  [
    'coturn-tests',
    python,
    [
      '-m',
      'unittest',
      'discover',
      '-s',
      'deploy/coturn',
      '-p',
      'test_config.py',
      '-v',
    ],
  ],
  ['bash-syntax', bash, ['-n', 'deploy/coturn/verify.sh']],
  ['bash-template', bash, ['deploy/coturn/verify.sh', '--template']],
  [
    'probe-build',
    'cargo',
    [
      'build',
      '-p',
      'hub-webrtc',
      '--bin',
      'turn-allocate-probe',
      '--features',
      'p006-turn-probe',
      '--profile',
      'probe',
      '--locked',
      '--offline',
    ],
  ],
  ['probe-help', probe, ['--help']],
  ['probe-default-denied', probe, [], 2],
  // Placeholders cannot deserialize into an IP. Fails before any PeerConnection is built.
  [
    'probe-placeholder-denied',
    probe,
    ['--approved-target', 'deploy/coturn/probe-input.json.example'],
    1,
  ],
];
const results = [];
for (const [name, command, args, expected = 0] of steps) {
  const r = spawnSync(command, args, {
    encoding: 'utf8',
    shell: command === 'pnpm' && process.platform === 'win32',
    maxBuffer: 20 * 1024 * 1024,
    env: {
      ...process.env,
      REMOTEHUB_VERIFY_DIR: 'artifacts/p006a/baseline',
      REMOTEHUB_PYTHON: python.replaceAll('\\', '/'),
    },
  });
  writeFileSync(
    `${dir}/${name}.txt`,
    `$ ${[command, ...args].join(' ')}\n${r.stdout ?? ''}${r.stderr ?? ''}\nEXIT=${r.status}\n`,
  );
  const passed = r.status === expected;
  results.push({
    name,
    command: `${command} ${args.join(' ')}`,
    exitCode: r.status,
    expectedExitCode: expected,
    passed,
  });
  writeFileSync(`${dir}/results.json`, JSON.stringify(results, null, 2));
  console.log(
    `${name}: exit=${r.status}, expected=${expected}, ${passed ? 'PASS' : 'FAIL'}`,
  );
  if (!passed) {
    process.exitCode = 1;
    break;
  }
}
