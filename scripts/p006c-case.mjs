// Run ONE explicitly approved cloud diagnostic. Operator observes allocation release between cases.
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
const args = process.argv.slice(2);
const cases = ['direct', 'fallback', 'turn-unavailable', 'all-unavailable'];
if (
  args.length !== 4 ||
  args[0] !== '--approved-target' ||
  args[2] !== '--case' ||
  !cases.includes(args[3])
)
  throw new Error('EXPLICIT_CASE_AND_TARGET_REQUIRED');
const input = JSON.parse(readFileSync(args[1], 'utf8'));
if (!input.operator_approved || !input.turn)
  throw new Error('APPROVED_TURN_CONFIG_REQUIRED');
const scenario = args[3];
const binary = `target/probe/ice-path-probe${process.platform === 'win32' ? '.exe' : ''}`;
const r = spawnSync(binary, args, {
  encoding: 'utf8',
  timeout: 75000,
  maxBuffer: 1024 * 1024,
});
if (r.error) throw new Error('PATH_PROBE_DID_NOT_EXIT_CLEANLY');
const report = JSON.parse(r.stdout.trim());
const encoded = JSON.stringify(report);
for (const value of [
  input.local,
  input.turn.relay,
  input.turn.a.ice_servers[0].username,
  input.turn.a.ice_servers[0].credential,
  input.turn.b.ice_servers[0].username,
  input.turn.b.ice_servers[0].credential,
]) {
  if (encoded.includes(value)) throw new Error('PATH_REPORT_PRIVATE_DATA_LEAK');
}
const passed =
  report.resources_released &&
  report.policy === 'all' &&
  report.closes?.length === 2 &&
  (scenario === 'all-unavailable'
    ? r.status === 1 &&
      report.error === 'DIRECT_CONNECT_TIMEOUT' &&
      report.sent === 0 &&
      report.selected_a === null &&
      report.selected_b === null &&
      report.fault_dropped_packets > 0
    : r.status === 0 &&
      report.error === null &&
      report.sent === 100 &&
      report.received === 100 &&
      report.acknowledged === 100 &&
      [report.selected_a, report.selected_b].every(
        (p) => p?.path === (scenario === 'fallback' ? 'relay' : 'direct'),
      ));
const budget =
  scenario === 'direct' || scenario === 'fallback'
    ? [report.gathered_a, report.gathered_b].every(
        (c) => c?.host >= 1 && c?.relay >= 1,
      )
    : [report.gathered_a, report.gathered_b].every(
        (c) => c?.host >= 1 && c?.relay === 0,
      );
const dir = 'artifacts/p006c/cloud';
mkdirSync(dir, { recursive: true });
writeFileSync(
  `${dir}/${scenario}.json`,
  JSON.stringify(
    {
      ...report,
      expectedOutcomePassed: Boolean(passed && budget),
      scope:
        'two native PeerConnections on one Windows PC; actual cloud TURN, process-local packet faults; no media',
    },
    null,
    2,
  ) + '\n',
);
console.log(`${scenario}: ${passed && budget ? 'PASS' : 'FAIL'}`);
if (!passed || !budget) process.exitCode = 1;
