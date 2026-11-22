// All old safety regressions plus dedicated diagnostic build. No cloud/capture action.
import { spawnSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
process.env.REMOTEHUB_VERIFY_DIR ??= 'artifacts/p007b/automatic';
await import('./p006c-video-checks.mjs');
if (!process.exitCode) {
  const dir = process.env.REMOTEHUB_VERIFY_DIR;
  const results = JSON.parse(readFileSync(`${dir}/results.json`, 'utf8'));
  for (const [name, args] of [
    [
      'diagnostic-clippy',
      [
        'clippy',
        '-p',
        'remotehub-desktop',
        '--all-targets',
        '--features',
        'p007b-diag',
        '--locked',
        '--offline',
        '--',
        '-D',
        'warnings',
      ],
    ],
    [
      'diagnostic-unit',
      [
        'test',
        '-p',
        'remotehub-desktop',
        '--features',
        'p007b-diag',
        '--locked',
        '--offline',
      ],
    ],
    [
      'diagnostic-build',
      [
        'build',
        '-p',
        'remotehub-desktop',
        '--profile',
        'probe',
        '--features',
        'custom-protocol,p007b-diag',
        '--locked',
        '--offline',
      ],
    ],
  ]) {
    const p = spawnSync('cargo', args, {
      encoding: 'utf8',
      maxBuffer: 4 * 1024 * 1024,
    });
    writeFileSync(
      `${dir}/${name}.txt`,
      (p.stdout ?? '') + (p.stderr ?? '') + `\nEXIT=${p.status}\n`,
    );
    results.push({
      name,
      command: ['cargo', ...args].join(' '),
      exitCode: p.status,
      expectedExitCode: 0,
    });
    writeFileSync(
      `${dir}/results.json`,
      JSON.stringify(results, null, 2) + '\n',
    );
    console.log(`${name}: ${p.status === 0 ? 'PASS' : 'FAIL'}`);
    if (p.status !== 0) {
      process.exitCode = 1;
      break;
    }
  }
  if (!process.exitCode) {
    const exe = 'target/probe/remotehub-desktop.exe';
    for (const [name, args, expected] of [
      ['diagnostic-self-check', ['--r06-self-check'], 0],
      ['diagnostic-desktop-override-denied', ['--source', 'desktop'], 2],
      ['diagnostic-fault-override-denied', ['--drop-direct'], 2],
    ]) {
      const p = spawnSync(exe, args, { encoding: 'utf8', timeout: 5000 });
      const info =
        name === 'diagnostic-self-check' && p.status === 0
          ? JSON.parse(p.stdout)
          : undefined;
      const valid =
        !info ||
        (info.sourceKind === 'synthetic_nv12' &&
          info.desktopCapture === false &&
          info.seconds === 8 &&
          info.nativeConsentRequired &&
          !info.automaticNetworkStart);
      writeFileSync(
        `${dir}/${name}.txt`,
        (p.stdout ?? '') + (p.stderr ?? '') + `\nEXIT=${p.status}\n`,
      );
      results.push({
        name,
        command: [exe, ...args].join(' '),
        exitCode: p.status,
        expectedExitCode: expected,
        metadataValid: valid,
      });
      writeFileSync(
        `${dir}/results.json`,
        JSON.stringify(results, null, 2) + '\n',
      );
      console.log(
        `${name}: ${p.status === expected && valid ? 'PASS' : 'FAIL'}`,
      );
      if (p.status !== expected || !valid) {
        process.exitCode = 1;
        break;
      }
    }
  }
}
