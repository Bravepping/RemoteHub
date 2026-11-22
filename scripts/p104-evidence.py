"""Offline P1-04 evidence, dependency and scope checks; never opens native UI or a network session."""
import datetime
import hashlib
import json
import pathlib
import re
import subprocess
import sys
from network_privacy import check_files, check_text

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / 'artifacts/p104'
BASE = '481e17e5587186c08ee262d2159c1d8454dd7b0e'
DOCS = ['README.md', 'TASKS.md', 'docs/P104_PLAN.md', 'docs/P104_IPC_CONTRACT.md', 'docs/P104_TEST_REPORT.md',
        'docs/ADR/0019-p104-pinia-typed-readonly-ipc.md', 'docs/DEVELOPMENT_GUIDE.md',
        'docs/PHASE1_DESKTOP_IMPLEMENTATION_PLAN.md', 'docs/PHASE0_RISK_REGISTER.md',
        '.ai-memory/handoff.md', '.ai-memory/project_memory.md']
STAGE = 'start'


def git(*args):
    return subprocess.check_output(['git', *args], cwd=ROOT, stderr=subprocess.PIPE)


def main():
    global STAGE
    def read(name): return json.loads((OUT / name).read_text(encoding='utf-8-sig'))
    def log(name): return (OUT / 'automatic' / name).read_text(encoding='utf-8-sig')
    def counts(name):
        values = re.findall(r'test result: ok\. (\d+) passed; 0 failed; (\d+) ignored', log(name))
        assert values
        return dict(passed=sum(int(a) for a, _ in values), ignored=sum(int(b) for _, b in values))
    STAGE = 'actual-results'
    q = read('quality-results.json')
    assert q['passed'] and q['state'] == 'complete'
    assert len(q['inherited']) == 41 and len(q['foundation']) == 6
    assert all(r['exitCode'] == r.get('expectedExitCode', 0) for r in q['inherited'] + q['foundation'])
    assert counts('test.txt') == dict(passed=82, ignored=1)
    assert counts('default-unit.txt') == dict(passed=56, ignored=1)
    assert re.search(r'Tests\s+79 passed', log('frontend-unit.txt'))
    assert '15 passed' in log('browser.txt') and 'Ran 12 tests' in log('python-lifecycle-unit.txt')
    matrix = read('build-matrix.json')
    assert matrix['passed'] and len(matrix['profiles']) == 8
    STAGE = 'dependencies'
    deps = read('dependencies/review.json')
    assert deps['passed'] and deps['package'] == 'pinia@3.0.4' and deps['installedWithScriptsDisabled']
    assert all(n == 0 for n in read('dependencies/audit.json')['metadata']['vulnerabilities'].values())
    packages = read('dependencies/new-packages.json')
    assert packages['passed'] and packages['newPackageCount'] == 12
    assert all(not p['installScriptNames'] and p['license'] in ['MIT', 'ISC', 'Apache-2.0', 'BSD-3-Clause'] for p in packages['packages'])
    lock = ROOT / 'pnpm-lock.yaml'
    assert packages['lockfileSha256'] == hashlib.sha256(lock.read_bytes()).hexdigest()
    old = json.loads(git('show', BASE + ':apps/desktop/package.json'))
    current = json.loads((ROOT / 'apps/desktop/package.json').read_text(encoding='utf-8'))
    assert current['dependencies'].pop('pinia') == '3.0.4'
    assert current == old
    lockdiff = git('diff', '--unified=0', BASE, '--', 'pnpm-lock.yaml').decode('utf-8')
    assert not any(line.startswith('-') and not line.startswith('---') for line in lockdiff.splitlines())
    STAGE = 'visual-and-native'
    visuals = read('visuals.json')
    assert len(visuals['screenshots']) == 3 and 'p104-explicit-sdk-fixture.png' in visuals['screenshots']
    for name, digest in visuals['screenshots'].items():
        assert hashlib.sha256((OUT / 'screenshots' / name).read_bytes()).hexdigest() == digest
    native = read('native/readonly-home.json')
    assert native['nativeReadObserved'] and not native['screenshotsRequested'] and not native['keyboardOrMouseActionsExecuted']
    assert not native['applicationCaptureInvoked'] and native['windowLeftForUser']
    assert all(text in native['text'] for text in ['本机核心已连接', 'windows · x86_64', '远程访问关闭'])
    assert not list((OUT / 'native').glob('*.png'))
    STAGE = 'scope-and-history'
    unchanged = ['crates', 'apps/desktop/src-tauri', 'apps/desktop/src/features', 'apps/desktop/src/style.css',
                 'apps/desktop/scripts/e2e.mjs', 'apps/desktop/tests/navigation.spec.ts', 'apps/desktop/tests/p103.spec.ts',
                 'scripts/p101-checks.mjs', 'Cargo.toml', 'Cargo.lock', 'pnpm-workspace.yaml']
    assert not git('diff', '--name-only', BASE, '--', *unchanged).strip()
    changed_artifacts = git('diff', '--name-only', BASE, '--', 'artifacts').decode('utf-8').splitlines()
    assert all(name.startswith('artifacts/p104/') for name in changed_artifacts)
    before_app = git('show', BASE + ':apps/desktop/src/App.vue').decode('utf-8-sig')
    assert before_app.split('<template>', 1)[1] == (ROOT / 'apps/desktop/src/App.vue').read_text(encoding='utf-8').split('<template>', 1)[1]
    old_bridge = git('show', BASE + ':apps/desktop/src/bridge.ts').decode('utf-8-sig')
    assert old_bridge.split('export async function runCaptureProbe', 1)[1] == (ROOT / 'apps/desktop/src/bridge.ts').read_text(encoding='utf-8').split('export async function runCaptureProbe', 1)[1]
    bundles = list((ROOT / 'apps/desktop/dist/assets').glob('*.js'))
    assert bundles and all(not any(marker in p.read_text(encoding='utf-8') for marker in ['P104-TEST-FIXTURE', '__p104Fixture', 'private-native-failure-fixture']) for p in bundles)
    assert json.loads(git('show', BASE + ':package.json'))['devDependencies'] == json.loads((ROOT / 'package.json').read_text(encoding='utf-8'))['devDependencies']
    names = set(git('diff', '--name-only', BASE).decode('utf-8').splitlines())
    names.update(git('ls-files', '--others', '--exclude-standard').decode('utf-8').splitlines())
    allowed = set(DOCS + ['package.json', 'pnpm-lock.yaml', 'apps/desktop/package.json', 'scripts/p104.ps1', 'scripts/p104-checks.mjs',
                         'scripts/p104-evidence.py', 'apps/desktop/src/App.vue', 'apps/desktop/src/main.ts',
                         'apps/desktop/src/bridge.ts', 'apps/desktop/src/App.test.ts', 'apps/desktop/tests/p104.spec.ts'])
    assert all(n in allowed or n.startswith(('apps/desktop/src/ipc/', 'apps/desktop/src/state/', 'artifacts/p104/')) for n in names)
    tasks = (ROOT / 'TASKS.md').read_text(encoding='utf-8')
    assert dict((n, mark == 'x') for mark, n in re.findall(r'^- \[([ x])\] `P1-(0[1-6])`', tasks, re.M)) == {'01': True, '02': True, '03': True, '04': True, '05': False, '06': False}
    STAGE = 'privacy-and-links'
    sources = [ROOT / n for n in names if pathlib.Path(n).suffix in ['.ts', '.vue', '.mjs', '.ps1'] and (ROOT / n).is_file()]
    for p in sources: check_text(p.read_text(encoding='utf-8'), payloads=False)
    check_files([pathlib.Path(__file__)], source=True)
    check_files([ROOT / n for n in DOCS])
    links = 0
    for name in DOCS:
        p = ROOT / name
        for url in re.findall(r'\[[^\]]+\]\(([^)]+)\)', p.read_text(encoding='utf-8')):
            if re.match(r'^[a-z]+:', url) or url.startswith('#'): continue
            target = (p.parent / url.strip('<>').split('#')[0]).resolve()
            target.relative_to(ROOT)
            assert target.exists() or target == OUT / 'verification.json'
            links += 1
    evidence = [p for p in OUT.rglob('*') if p.suffix in ['.json', '.txt'] and p.name != 'verification.json' and 'browser-results' not in p.parts]
    check_files(evidence)
    git('diff', '--check')
    value = dict(recordedAt=datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=8))).isoformat(),
                 baselineCommit=BASE, passed=True, implementedTask='P1-04', qualityCommands=47, frontendTests=79, edgeTests=15,
                 rustAllFeatures=counts('test.txt'), rustDefault=counts('default-unit.txt'), pythonLifecycleTests=12,
                 actualCargoProfiles=8, buildIsolationTests=9, piniaVersion='3.0.4', newDependencyPackages=12, dependencyAuditVulnerabilities=0,
                 nativeUiScope='READ_ONLY_HOME_OBSERVED; no refresh/cancel/timeout input automation', nativeScreenshots=False,
                 browserScreenshots=3, explicitSdkFixtureImages=1, applicationCaptureInvoked=False, applicationRemoteInputInvoked=False,
                 cloudContacted=False, newTwoPcMediaTest='NOT_RUN', productRemoteSessionIntegration='NOT_IMPLEMENTED',
                 cspPermissionsAndRustMediaUnchanged=True, cargoLockUnchanged=True, historicalEvidenceUnchanged=True,
                 nodeLockChange='Pinia and 11 transitive additions only; existing entries not removed or upgraded',
                 R06TechnicalStatus='OPEN_NOT_REPRODUCED', P105Started=False, P106Started=False,
                 relativeLinksChecked=links, privacySourceFiles=len(sources)+1, privacyTextEvidenceFiles=len(evidence),
                 evidenceHashFormat='SHA256 of UTF-8 LF text without BOM',
                 textEvidenceSha256={p.relative_to(OUT).as_posix(): hashlib.sha256(p.read_text(encoding='utf-8-sig').encode('utf-8')).hexdigest() for p in evidence},
                 defaultNativeExeSha256=hashlib.sha256((ROOT / 'target/probe/remotehub-desktop.exe').read_bytes()).hexdigest())
    check_text(json.dumps(value))
    (OUT / 'verification.json').write_text(json.dumps(value, indent=2)+'\n', encoding='utf-8')
    print(json.dumps({k: value[k] for k in ['passed', 'implementedTask', 'qualityCommands', 'frontendTests', 'edgeTests', 'P105Started']}))


if __name__ == '__main__':
    try:
        main()
    except Exception:
        print(json.dumps(dict(passed=False, stage=STAGE, error='P104_EVIDENCE_FAILED_NO_PRIVATE_ECHO')), file=sys.stderr)
        raise SystemExit(1) from None
