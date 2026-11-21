// Explicit local-only real native ICE/SCTP test. No cloud, screen, input or system changes.
import { networkInterfaces } from 'node:os';
import { mkdirSync, writeFileSync, unlinkSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
const args = process.argv.slice(2);
if (args.length !== 2 || args[0] !== '--local')
  throw new Error('EXPLICIT_LOCAL_INTERFACE_REQUIRED');
const local = args[1];
const assigned = Object.values(networkInterfaces())
  .flat()
  .some((a) => a?.family === 'IPv4' && !a.internal && a.address === local);
if (!assigned) throw new Error('LOCAL_INTERFACE_NOT_ASSIGNED');
const dir = 'artifacts/p006c/local';
const privateDir = 'artifacts/private/p006c';
mkdirSync(dir, { recursive: true });
mkdirSync(privateDir, { recursive: true });
const path = `${privateDir}/local-input-${process.pid}.json`;
const binary = `target/probe/ice-path-probe${process.platform === 'win32' ? '.exe' : ''}`;
writeFileSync(
  path,
  JSON.stringify({
    operator_approved: true,
    local,
    approval_expires_at: Math.floor(Date.now() / 1000) + 300,
    turn: null,
  }),
  { mode: 0o600 },
);
const results = [];
try {
  for (const scenario of ['direct', 'all-unavailable']) {
    const r = spawnSync(
      binary,
      ['--approved-target', path, '--case', scenario],
      { encoding: 'utf8', timeout: 75000, maxBuffer: 1024 * 1024 },
    );
    if (r.error) throw new Error('PROBE_DID_NOT_EXIT_CLEANLY');
    const report = JSON.parse(r.stdout.trim());
    if (JSON.stringify(report).includes(local))
      throw new Error('REPORT_ADDRESS_LEAK');
    writeFileSync(
      `${dir}/${scenario}.json`,
      JSON.stringify(report, null, 2) + '\n',
    );
    const passed =
      report.resources_released === true &&
      report.policy === 'all' &&
      (scenario === 'direct'
        ? r.status === 0 &&
          report.sent === 100 &&
          report.received === 100 &&
          report.acknowledged === 100 &&
          report.selected_a?.path === 'direct' &&
          report.selected_b?.path === 'direct' &&
          report.error === null
        : r.status === 1 &&
          report.error === 'DIRECT_CONNECT_TIMEOUT' &&
          report.selected_a === null &&
          report.selected_b === null &&
          report.sent === 0 &&
          report.fault_dropped_packets > 0);
    results.push({
      scenario,
      exitCode: r.status,
      passed,
      scope:
        'two real native PeerConnections on one Windows PC; no cloud or video',
    });
    writeFileSync(
      `${dir}/results.json`,
      JSON.stringify(results, null, 2) + '\n',
    );
    console.log(`${scenario}: ${passed ? 'PASS' : 'FAIL'}`);
    if (!passed) {
      process.exitCode = 1;
      break;
    }
  }
} finally {
  unlinkSync(path);
}
