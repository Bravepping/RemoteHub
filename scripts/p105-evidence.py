"""Offline P1-05/C0-DRAFT evidence and scope checks; no UI, cloud or media execution."""
import datetime
import hashlib
import json
import pathlib
import re
import subprocess
import sys
from network_privacy import check_files, check_text

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / 'artifacts/p105'
BASE = '9418db2b6ff83cc2207b89df077a737e6580b825'
DOCS = ['README.md', 'TASKS.md', 'docs/P105_PLAN.md', 'docs/P105_TEST_REPORT.md',
        'docs/C0_COMPATIBILITY_TARGET_CONTRACT_DRAFT.md', 'docs/ADR/0020-p105-request-feedback-and-compatibility-draft.md',
        'docs/DEVELOPMENT_GUIDE.md', 'docs/PHASE1_DESKTOP_IMPLEMENTATION_PLAN.md', 'docs/PHASE0_RISK_REGISTER.md',
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
    assert re.search(r'Tests\s+88 passed', log('frontend-unit.txt'))
    assert '18 passed' in log('browser.txt') and 'Ran 12 tests' in log('python-lifecycle-unit.txt')
    matrix = read('build-matrix.json')
    assert matrix['passed'] and len(matrix['profiles']) == 8
    assert read('browser-preflight.json')['passed']
    extra = read('privacy/results.json')
    assert extra['passed'] and all(r['exitCode'] == 0 and r['passed'] for r in extra['results'])
    assert [(r['name'], r['expectedTests']) for r in extra['results']] == [('privacy', 12), ('delivery', 9)]
    STAGE = 'visual-native-and-c0'
    visuals = read('visuals.json')
    assert len(visuals['screenshots']) == 6
    for name, digest in visuals['screenshots'].items():
        assert hashlib.sha256((OUT / 'screenshots' / name).read_bytes()).hexdigest() == digest
    native = read('native/result.json')
    assert native['nativeFivePageAcceptance'] == 'NOT_COMPLETED' and native['pagesObservedThisRound'] == []
    assert native['launchAttempts'] == 2 and native['testInstancesCleaned'] and not native['userExistingApplicationsStopped']
    assert not native['applicationCaptureInvoked'] and native['edgeCannotReplaceNativeAcceptance']
    assert not list((OUT / 'native').glob('*.png'))
    sources = read('c0/public-sources.json')
    assert not sources['targetVersionsApproved'] and not sources['localInstallersRehashedThisRound']
    assert len(sources['sources']) == 3 and sources['allPublicLicenseHeadersVerified']
    assert all(s['localInstallerCorrespondence'] == 'NOT_VERIFIED' and s['interoperability'] == 'NOT_RUN' and re.fullmatch('[0-9a-f]{40}', s['sourceCommitObserved']) for s in sources['sources'])
    draft = (ROOT / 'docs/C0_COMPATIBILITY_TARGET_CONTRACT_DRAFT.md').read_text(encoding='utf-8')
    assert all(term in draft for term in ['DRAFT / NOT_APPROVED', 'PENDING_USER_DECISION', 'C0-H', 'C0-C', 'C0-U', 'C0-N', '许可证核验', '进程', 'R06继续OPEN'])
    STAGE = 'scope-and-history'
    unchanged = ['crates', 'apps/desktop/src-tauri', 'apps/desktop/src/bridge.ts', 'apps/desktop/src/ipc/tauriDesktopApi.ts',
                 'apps/desktop/src/features/experiments', 'apps/desktop/src/features/p004', 'apps/desktop/src/features/p006',
                 'apps/desktop/src/features/desktop/navigation.ts', 'apps/desktop/src/style.css', 'apps/desktop/src/features/desktop/desktop.css',
                 'apps/desktop/scripts/e2e.mjs', 'scripts/p101-checks.mjs', 'Cargo.toml', 'Cargo.lock',
                 'pnpm-lock.yaml', 'pnpm-workspace.yaml', 'apps/desktop/package.json']
    assert not git('diff', '--name-only', BASE, '--', *unchanged).strip()
    assert all(n.startswith('artifacts/p105/') for n in git('diff', '--name-only', BASE, '--', 'artifacts').decode('utf-8').splitlines())
    names = set(git('diff', '--name-only', BASE).decode('utf-8').splitlines())
    names.update(git('ls-files', '--others', '--exclude-standard').decode('utf-8').splitlines())
    allowed = set(DOCS + ['package.json', 'scripts/p105.ps1', 'scripts/p105-checks.mjs', 'scripts/p105-evidence.py',
                         'apps/desktop/src/App.vue', 'apps/desktop/src/App.test.ts', 'apps/desktop/src/state/desktop.ts',
                         'apps/desktop/src/state/desktop.test.ts', 'apps/desktop/src/state/feedback.test.ts',
                         'apps/desktop/src/ipc/contracts.ts', 'apps/desktop/tests/p105.spec.ts',
                         'apps/desktop/src/features/desktop/DesktopShell.vue', 'apps/desktop/src/features/desktop/LocalDeviceCard.vue',
                         'apps/desktop/src/features/desktop/pages/SettingsPage.vue', 'apps/desktop/src/features/desktop/ReadFeedback.vue',
                         'apps/desktop/src/features/desktop/ReadFeedback.test.ts', 'apps/desktop/src/features/desktop/p105.css'])
    assert all(n in allowed or n.startswith('artifacts/p105/') for n in names)
    tasks = (ROOT / 'TASKS.md').read_text(encoding='utf-8')
    assert dict((n, mark == 'x') for mark, n in re.findall(r'^- \[([ x])\] `P1-(0[1-6])`', tasks, re.M)) == {'01': True, '02': True, '03': True, '04': True, '05': True, '06': False}
    STAGE = 'privacy-and-links'
    code = [ROOT / n for n in names if pathlib.Path(n).suffix in ['.ts', '.vue', '.css', '.mjs', '.ps1'] and (ROOT / n).is_file()]
    for p in code: check_text(p.read_text(encoding='utf-8'), payloads=False)
    check_files([pathlib.Path(__file__)], source=True)
    check_files([ROOT / n for n in DOCS])
    links = 0
    for name in DOCS:
        p = ROOT / name
        for url in re.findall(r'\[[^\]]+\]\(([^)]+)\)', p.read_text(encoding='utf-8')):
            if re.match(r'^[a-z]+:', url) or url.startswith('#'): continue
            target = (p.parent / url.strip('<>').split('#')[0]).resolve(); target.relative_to(ROOT)
            assert target.exists() or target == OUT / 'verification.json'
            links += 1
    bundles = list((ROOT / 'apps/desktop/dist/assets').glob('*.js'))
    assert bundles and all(not any(marker in p.read_text(encoding='utf-8') for marker in ['P105-TEST-FIXTURE', '__p105Fixture', 'SENSITIVE_TOKEN_FIXTURE']) for p in bundles)
    evidence = [p for p in OUT.rglob('*') if p.suffix in ['.json', '.txt'] and p.name != 'verification.json' and 'browser-results' not in p.parts]
    check_files(evidence); git('diff', '--check')
    digest = hashlib.sha256((ROOT / 'target/probe/remotehub-desktop.exe').read_bytes()).hexdigest()
    assert digest == native['programSha256']
    value = dict(recordedAt=datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=8))).isoformat(), baselineCommit=BASE,
                 passed=True, implementedTask='P1-05 + C0-DRAFT', qualityCommands=47, frontendTests=88, edgeTests=18,
                 rustAllFeatures=counts('test.txt'), rustDefault=counts('default-unit.txt'), pythonLifecycleTests=12,
                 actualCargoProfiles=8, buildIsolationTests=9, privacyTests=12, deliveryTests=9, browserScreenshots=6,
                 nativeFivePageAcceptance='NOT_COMPLETED', nativeReason='No targetable window returned by automation inventory', nativeTestInstancesCleaned=True,
                 nativeScreenshots=False, applicationCaptureInvoked=False, applicationRemoteInputInvoked=False,
                 cloudServiceContacted=False, publicRepositoryMetadataOnly=True, newTwoPcTest='NOT_RUN', thirdPartySoftwareStarted=False,
                 c0Status='DRAFT_NOT_APPROVED', c1Status='NOT_COMPLETED', compatibilityRuntimePrototype='NOT_STARTED',
                 cspPermissionsAndRustMediaUnchanged=True, dependencyLocksUnchanged=True, historicalEvidenceUnchanged=True,
                 R06TechnicalStatus='OPEN_NOT_REPRODUCED', P106Started=False, relativeLinksChecked=links,
                 privacySourceFiles=len(code)+1, privacyTextEvidenceFiles=len(evidence), evidenceHashFormat='SHA256 of UTF-8 LF text without BOM',
                 textEvidenceSha256={p.relative_to(OUT).as_posix(): hashlib.sha256(p.read_text(encoding='utf-8-sig').encode('utf-8')).hexdigest() for p in evidence}, defaultNativeExeSha256=digest)
    check_text(json.dumps(value)); (OUT / 'verification.json').write_text(json.dumps(value, indent=2)+'\n', encoding='utf-8')
    print(json.dumps({k: value[k] for k in ['passed', 'qualityCommands', 'frontendTests', 'edgeTests', 'nativeFivePageAcceptance', 'c0Status', 'P106Started']}))


if __name__ == '__main__':
    try: main()
    except Exception:
        print(json.dumps(dict(passed=False, stage=STAGE, error='P105_EVIDENCE_FAILED_NO_PRIVATE_ECHO')), file=sys.stderr)
        raise SystemExit(1) from None
