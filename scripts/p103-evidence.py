"""Offline P1-03 delivery evidence checks; no client, media or network is started."""
import datetime
import hashlib
import json
import pathlib
import re
import subprocess
import sys
from network_privacy import check_files, check_text

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / 'artifacts/p103'
BASE = 'd0fd876e17ccaf2293075a9b15178ed97d98a018'
DOCS = ['README.md', 'TASKS.md', 'docs/P103_PLAN.md', 'docs/P103_TEST_REPORT.md',
        'docs/ADR/0018-p103-inert-connection-and-session-ui.md', 'docs/DEVELOPMENT_GUIDE.md',
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
    assert re.search(r'Tests\s+50 passed', log('frontend-unit.txt'))
    assert '14 passed' in log('browser.txt') and 'Ran 12 tests' in log('python-lifecycle-unit.txt')
    matrix = read('build-matrix.json')
    assert matrix['passed'] and len(matrix['profiles']) == 8
    assert read('browser-preflight.json')['passed']
    post = read('post-review-build.json')
    assert post['passed'] and len(post['steps']) == 3
    assert all(r['exitCode'] == 0 for r in post['steps'])
    assert '14 passed' in (OUT / 'browser-preflight.txt').read_text(encoding='utf-8')
    raw = read('development/browser-initial-raw.json')
    assert hashlib.sha256(raw['output'].encode('utf-8')).hexdigest() == raw['originalUtf8Sha256']
    assert '\n'.join(line.rstrip(' \t') for line in raw['output'].splitlines())+'\n' == (OUT / 'development/browser-initial.txt').read_text(encoding='utf-8')
    STAGE = 'screenshots'
    visuals = read('visuals.json')
    assert len(visuals['screenshots']) == 10
    for name, digest in visuals['screenshots'].items():
        assert hashlib.sha256((OUT / 'screenshots' / name).read_bytes()).hexdigest() == digest
    STAGE = 'scope-and-history'
    unchanged = ['crates', 'apps/desktop/src-tauri', 'apps/desktop/src/App.vue',
                 'apps/desktop/src/bridge.ts', 'apps/desktop/src/features/experiments',
                 'apps/desktop/src/features/p004', 'apps/desktop/src/features/p006',
                 'apps/desktop/src/style.css', 'apps/desktop/src/features/desktop/desktop.css',
                 'apps/desktop/src/features/desktop/navigation.ts', 'apps/desktop/scripts/e2e.mjs',
                 'scripts/p101-checks.mjs', 'Cargo.toml', 'Cargo.lock', 'pnpm-lock.yaml',
                 'pnpm-workspace.yaml', 'apps/desktop/package.json']
    assert not git('diff', '--name-only', BASE, '--', *unchanged).strip()
    changed_artifacts = git('diff', '--name-only', BASE, '--', 'artifacts').decode('utf-8').splitlines()
    assert all(name.startswith('artifacts/p103/') for name in changed_artifacts)
    assert json.loads(git('show', BASE + ':package.json'))['devDependencies'] == json.loads((ROOT / 'package.json').read_text(encoding='utf-8'))['devDependencies']
    names = set(git('diff', '--name-only', BASE).decode('utf-8').splitlines())
    names.update(git('ls-files', '--others', '--exclude-standard').decode('utf-8').splitlines())
    allowed = set(DOCS + ['package.json', 'scripts/p103.ps1', 'scripts/p103-checks.mjs',
                         'scripts/p103-evidence.py', 'apps/desktop/src/App.test.ts', 'apps/desktop/tests/p103.spec.ts'])
    assert all(n in allowed or n.startswith(('apps/desktop/src/features/desktop/', 'artifacts/p103/')) for n in names)
    tasks = (ROOT / 'TASKS.md').read_text(encoding='utf-8')
    assert dict((n, mark == 'x') for mark, n in re.findall(r'^- \[([ x])\] `P1-(0[1-6])`', tasks, re.M)) == {'01': True, '02': True, '03': True, '04': False, '05': False, '06': False}
    STAGE = 'privacy-and-links'
    sources = [ROOT / n for n in names if pathlib.Path(n).suffix in ['.ts', '.vue', '.css', '.mjs', '.ps1'] and (ROOT / n).is_file()]
    for p in sources:
        text = p.read_text(encoding='utf-8')
        check_text(text, payloads=False)
        if p.suffix == '.vue':
            assert not re.search(r'\b(invoke|WebSocket|RTCPeerConnection|requestFullscreen|getDisplayMedia|SendInput)\s*\(', text)
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
                 baselineCommit=BASE, passed=True, implementedTask='P1-03', qualityCommands=47,
                 frontendTests=50, edgeTests=14, rustAllFeatures=counts('test.txt'), rustDefault=counts('default-unit.txt'),
                 postVisualReviewEdgeTests=14, postVisualReviewBuildChecks=3,
                 pythonLifecycleTests=12, actualCargoProfiles=8, buildIsolationTests=9, browserScreenshots=10,
                 source='Actual local Edge interactions and offline build/regression; unit capability values are explicit fixtures',
                 nativeUiTest='NOT_RUN', applicationCaptureInvoked=False, applicationRemoteInputInvoked=False,
                 cloudContacted=False, newTwoPcMediaTest='NOT_RUN', productRemoteSessionIntegration='NOT_IMPLEMENTED',
                 cspPermissionsAndRustMediaUnchanged=True, dependencyLocksUnchanged=True, historicalEvidenceUnchanged=True,
                 R06TechnicalStatus='OPEN_NOT_REPRODUCED', P104Started=False, P105Started=False, P106Started=False,
                 relativeLinksChecked=links, privacySourceFiles=len(sources)+1, privacyTextEvidenceFiles=len(evidence),
                 evidenceHashFormat='SHA256 of UTF-8 LF text without BOM',
                 textEvidenceSha256={p.relative_to(OUT).as_posix(): hashlib.sha256(p.read_text(encoding='utf-8-sig').encode('utf-8')).hexdigest() for p in evidence},
                 defaultNativeExeSha256=hashlib.sha256((ROOT / 'target/probe/remotehub-desktop.exe').read_bytes()).hexdigest())
    check_text(json.dumps(value))
    (OUT / 'verification.json').write_text(json.dumps(value, indent=2)+'\n', encoding='utf-8')
    print(json.dumps({k: value[k] for k in ['passed', 'implementedTask', 'qualityCommands', 'frontendTests', 'edgeTests', 'nativeUiTest', 'P104Started']}))


if __name__ == '__main__':
    try:
        main()
    except Exception:
        print(json.dumps(dict(passed=False, stage=STAGE, error='P103_EVIDENCE_FAILED_NO_PRIVATE_ECHO')), file=sys.stderr)
        raise SystemExit(1) from None
