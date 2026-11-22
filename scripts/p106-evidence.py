"""Offline P1-06 scope/evidence audit; never starts UI, media, network or services."""
import datetime
import hashlib
import json
import pathlib
import re
import subprocess
import sys
from network_privacy import check_files, check_text

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / 'artifacts/p106'
BASE = '7f36b393658e2050fa9f976bc95524bb48893798'
DOCS = ['README.md', 'TASKS.md', 'docs/P106_PLAN.md', 'docs/P106_TEST_REPORT.md',
        'docs/ADR/0021-default-tray-and-safe-exit-lifecycle.md', 'docs/C0_D1_D6_USER_DECISION_SHEET.md',
        'docs/PHASE1_EXIT_REVIEW_PREPARATION.md', 'docs/P105_USER_WINDOW_VISIBILITY_ADDENDUM.md',
        'docs/DEVELOPMENT_GUIDE.md', 'docs/PHASE1_DESKTOP_IMPLEMENTATION_PLAN.md', 'docs/PHASE0_RISK_REGISTER.md',
        '.ai-memory/handoff.md', '.ai-memory/project_memory.md']
SOURCES = ['package.json', 'scripts/p106.ps1', 'scripts/p106-checks.mjs', 'scripts/p106-native.ps1',
           'scripts/p106-native-checks.mjs', 'scripts/p106-evidence.py', 'scripts/quality/p106-permissions.mjs',
           'scripts/quality/p106-permissions.test.mjs', 'apps/desktop/src-tauri/Cargo.toml',
           'apps/desktop/src-tauri/capabilities/default.json', 'apps/desktop/src-tauri/src/main.rs',
           'apps/desktop/src-tauri/src/lifecycle.rs', 'apps/desktop/src-tauri/src/lifecycle_policy.rs',
           'apps/desktop/src-tauri/examples/lifecycle_native.rs', 'apps/desktop/src/features/desktop/pages/SettingsPage.vue']
STAGE = 'start'


def git(*args):
    return subprocess.check_output(['git', '-c', 'safe.directory='+ROOT.as_posix(), *args], cwd=ROOT, stderr=subprocess.PIPE)


def main():
    global STAGE
    def text(name): return (ROOT / name).read_text(encoding='utf-8-sig')
    def read(name): return json.loads((OUT / name).read_text(encoding='utf-8-sig'))
    def log(name): return (OUT / 'automatic' / name).read_text(encoding='utf-8-sig')
    def counts(name):
        values = re.findall(r'test result: ok\. (\d+) passed; 0 failed; (\d+) ignored', log(name))
        assert values
        return dict(passed=sum(int(a) for a, _ in values), ignored=sum(int(b) for _, b in values))
    STAGE = 'actual-results'
    quality = read('quality-results.json')
    assert quality['passed'] and quality['state'] == 'complete'
    checks = quality['foundation'] + quality['inherited']
    assert len(quality['foundation']) == 6 and len(quality['inherited']) == 41
    assert all(r['exitCode'] == r.get('expectedExitCode', 0) for r in checks)
    assert counts('test.txt') == dict(passed=82, ignored=1)
    assert counts('default-unit.txt') == dict(passed=62, ignored=1)
    assert re.search(r'Tests\s+88 passed', log('frontend-unit.txt'))
    assert '18 passed' in log('browser.txt') and 'Ran 12 tests' in log('python-lifecycle-unit.txt')
    matrix = read('build-matrix.json')
    assert matrix['passed'] and len(matrix['profiles']) == 8
    permissions = read('permissions.json')
    assert permissions['passed'] and permissions['addedAllowedCommands'] == 0
    assert permissions['removedAllowedCommands'] == 34 and permissions['jsTrayAndMenuCommandsAllowed'] == 0
    assert permissions['customExperimentalPermissionsUnchanged']
    permission_log = (OUT/'permission-tests.txt').read_text(encoding='utf-8-sig')
    assert re.search(r'pass 7\b', permission_log) and 'EXIT=0' in permission_log
    extra = read('privacy/results.json')
    assert extra['passed'] and all(r['exitCode'] == 0 and r['passed'] for r in extra['results'])
    assert [(r['name'], r['expectedTests']) for r in extra['results']] == [('privacy', 12), ('delivery', 9)]
    STAGE = 'native-limitations-and-user-confirmation'
    native = read('native/assessment.json')
    assert native['nativeLifecycleAcceptance'] == 'NOT_COMPLETED' and native['launchAttempts'] == 2
    assert native['observedNativeLifecycleRecords'] == 0 and native['rootCause'] == 'NOT_DETERMINED'
    for name in native['attempts']:
        attempt = read('native/'+name)
        assert not attempt['passed'] and len(attempt['cases']) == 1
        case = attempt['cases'][0]
        assert case['case'] == 'default-close' and case['exitCode'] == 3221225785
        assert not case.get('native') and not case['passed'] and not case['timedOut']
        assert case['profileCleanup'] == 'removed'
        assert all(attempt[key] is False for key in ['captureStarted', 'remoteInputExecuted', 'cloudContacted'])
    user = read('native/user-confirmation.json')
    assert user['source'] == 'USER_STATEMENT_THIS_TASK' and user['status'] == 'USER_CONFIRMED_WINDOW_VISIBLE'
    assert user['fivePageNativeInteraction'] == 'NOT_FULLY_VERIFIED' and user['historicalLogsPreserved']
    cleanup = read('native/cleanup.json')
    assert cleanup['nativeProbeProcessCount'] == 0 and cleanup['privateNativeProfileCount'] == 0
    assert cleanup['userExistingApplicationsStopped'] is False
    STAGE = 'source-and-history-scope'
    unchanged = ['crates', 'Cargo.toml', 'Cargo.lock', 'pnpm-lock.yaml', 'pnpm-workspace.yaml',
                 'apps/desktop/package.json', 'apps/desktop/src-tauri/tauri.conf.json',
                 'apps/desktop/src-tauri/tauri.direct.conf.json', 'apps/desktop/src-tauri/permissions',
                 'apps/desktop/src-tauri/build.rs', 'apps/desktop/src-tauri/src/experiment_origin.rs',
                 'apps/desktop/src-tauri/src/p004.rs', 'apps/desktop/src-tauri/src/p004_lan.rs',
                 'apps/desktop/src-tauri/src/p006.rs', 'apps/desktop/src-tauri/src/p007b_diag.rs',
                 'apps/desktop/src/features/experiments', 'apps/desktop/src/features/p004', 'apps/desktop/src/features/p006',
                 'apps/desktop/src/bridge.ts', 'apps/desktop/src/ipc', 'apps/desktop/src/state',
                 'apps/desktop/scripts/e2e.mjs', 'scripts/p101-checks.mjs', 'docs/P105_TEST_REPORT.md']
    assert not git('diff', '--name-only', BASE, '--', *unchanged).strip()
    names = set(git('diff', '--name-only', BASE).decode('utf-8').splitlines())
    names.update(git('ls-files', '--others', '--exclude-standard').decode('utf-8').splitlines())
    assert all(n in set(DOCS+SOURCES) or n.startswith('artifacts/p106/') for n in names)
    assert all(n.startswith('artifacts/p106/') for n in git('diff', '--name-only', BASE, '--', 'artifacts').decode('utf-8').splitlines())
    cargo = text('apps/desktop/src-tauri/Cargo.toml')
    old_cargo = git('show', BASE+':apps/desktop/src-tauri/Cargo.toml').decode('utf-8')
    assert cargo == old_cargo.replace('tauri = { version = "=2.12.7", features = [] }', 'tauri = { version = "=2.12.7", features = ["tray-icon"] }')
    main_source = text('apps/desktop/src-tauri/src/main.rs')
    assert '#[cfg(not(feature = "p004-probe"))]\n    let builder = lifecycle::install(builder, probe.0.clone());' in main_source
    for handler in ['p004::on_window_event', 'p004_lan::on_window_event', 'p006::on_window_event']:
        assert handler in main_source
    lifecycle = text('apps/desktop/src-tauri/src/lifecycle.rs')
    assert '.on_window_event(close)' in lifecycle and 'api.prevent_close()' in lifecycle and 'api.prevent_exit()' in lifecycle
    assert 'state.busy.load(Ordering::Acquire)' in lifecycle and 'app.remove_tray_by_id(TRAY_ID)' in lifecycle
    assert 'std::time::Duration::from_secs(7)' in lifecycle
    tasks = text('TASKS.md')
    assert len(re.findall(r'^- \[x\] `P1-0[1-5]`', tasks, re.M)) == 5
    assert '- [ ] `P1-06` **implemented / native-validation-blocked' in tasks
    assert all(mark == ' ' for mark in re.findall(r'^- \[([ x])\] `P2-\d+`', tasks, re.M))
    decisions = text('docs/C0_D1_D6_USER_DECISION_SHEET.md')
    assert all('D'+str(n) in decisions for n in range(1,7)) and 'AWAITING_USER_DECISIONS' in decisions
    review = text('docs/PHASE1_EXIT_REVIEW_PREPARATION.md')
    assert all(term in review for term in ['PREPARED_AWAITING_USER_REVIEW', 'HOLD_FOR_NATIVE_TRAY_ACCEPTANCE', 'OPEN_NOT_REPRODUCED', 'NOT_DETERMINED'])
    STAGE = 'privacy-and-links'
    code = [ROOT/n for n in SOURCES if pathlib.Path(n).suffix in ['.rs', '.vue', '.mjs', '.ps1']]
    for path in code: check_text(path.read_text(encoding='utf-8'), payloads=False)
    check_files([pathlib.Path(__file__)], source=True)
    check_files([ROOT/n for n in DOCS])
    links = 0
    for name in DOCS:
        path = ROOT/name
        for url in re.findall(r'\[[^\]]+\]\(([^)]+)\)', path.read_text(encoding='utf-8')):
            if re.match(r'^[a-z]+:', url) or url.startswith('#'): continue
            target = (path.parent / url.strip('<>').split('#')[0]).resolve()
            target.relative_to(ROOT)
            assert target.exists() or target == OUT/'verification.json'
            links += 1
    evidence = [p for p in OUT.rglob('*') if p.suffix in ['.json', '.txt'] and p.name != 'verification.json' and 'browser-results' not in p.parts]
    check_files(evidence)
    git('diff', '--check')
    def sha(path): return hashlib.sha256(path.read_bytes()).hexdigest()
    example_sha = sha(ROOT/'target/probe/examples/lifecycle_native.exe')
    tested_sha = read('native/results.json')['programSha256']
    assert read('native/initial-launch.json')['programSha256'] == tested_sha
    final_build = read('native/final-build.json')
    assert final_build['exitCode'] == 0 and final_build['finalSourceBuilt'] and not final_build['executed']
    assert final_build['programSha256'] == example_sha
    value = dict(recordedAt=datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=8))).isoformat(),
                 baselineCommit=BASE, passed=True, scope='Offline evidence audit; not native acceptance',
                 implementedTask='P1-06', qualityCommands=47, frontendTests=88, edgeTests=18,
                 rustAllFeatures=counts('test.txt'), rustDefault=counts('default-unit.txt'),
                 actualCargoProfiles=8, buildIsolationTests=9, pythonLifecycleTests=12,
                 permissionTests=7, privacyTests=12, deliveryTests=9, permissions=permissions,
                 priorWindowVisible='USER_CONFIRMED_WINDOW_VISIBLE', nativeFivePageInteraction='NOT_FULLY_VERIFIED',
                 nativeLifecycleAcceptance='NOT_COMPLETED', nativeProbeLaunchAttempts=2, nativeProbeRootCause='NOT_DETERMINED',
                 nativeVideoCleanupThisRound='NOT_RUN', cloudContacted=False, newTwoPcTest='NOT_RUN',
                 actualDesktopCapture=False, remoteInputExecuted=False, thirdPartySoftwareStarted=False,
                 cspAndExperimentalPermissionsUnchanged=True, effectiveJsPermissionsReduced=True,
                 mediaCodeAndDependencyLocksUnchanged=True, historicalEvidenceUnchanged=True,
                 R06TechnicalStatus='OPEN_NOT_REPRODUCED', c0Status='AWAITING_USER_DECISIONS',
                 phase1Exit='PREPARED_AWAITING_USER_REVIEW', phase2Started=False,
                 relativeLinksChecked=links, sourcePrivacyFiles=len(code)+1, evidenceTextFiles=len(evidence),
                 defaultNativeExeSha256=sha(ROOT/'target/probe/remotehub-desktop.exe'), nativeProbeExeSha256=example_sha,
                 earlierExecutedNativeProbeExeSha256=tested_sha, finalNativeProbeExecution='NOT_RUN',
                 evidenceHashFormat='SHA256 of UTF-8 LF text without BOM',
                 textEvidenceSha256={p.relative_to(OUT).as_posix(): hashlib.sha256(p.read_text(encoding='utf-8-sig').encode('utf-8')).hexdigest() for p in evidence})
    check_text(json.dumps(value))
    (OUT/'verification.json').write_text(json.dumps(value, indent=2)+'\n', encoding='utf-8', newline='\n')
    print(json.dumps({k: value[k] for k in ['passed', 'qualityCommands', 'permissionTests', 'nativeLifecycleAcceptance', 'phase1Exit', 'phase2Started']}))


if __name__ == '__main__':
    try: main()
    except Exception:
        print(json.dumps(dict(passed=False, stage=STAGE, error='P106_EVIDENCE_FAILED_NO_PRIVATE_ECHO')), file=sys.stderr)
        raise SystemExit(1) from None
