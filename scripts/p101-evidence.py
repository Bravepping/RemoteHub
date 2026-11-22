"""Validate only P1-01 offline build evidence and approved scope; no runtime/network action."""
import datetime
import hashlib
import json
import pathlib
import re
import subprocess
import sys
from network_privacy import check_files, check_text

ROOT=pathlib.Path(__file__).resolve().parent.parent
OUT=ROOT/'artifacts/p101'
BASE='767ff44cba1690fce69c417f82c6531ce3fcee14'
DOCS=['docs/PHASE0_EXIT_DECISION.md','docs/P101_PLAN.md','docs/P101_DESKTOP_CONTRACT.md',
      'docs/P101_TEST_REPORT.md','docs/ADR/0016-p101-foundation-and-quality-gates.md',
      'docs/DEVELOPMENT_GUIDE.md','docs/PHASE0_RISK_REGISTER.md','docs/PHASE1_DESKTOP_IMPLEMENTATION_PLAN.md',
      'README.md','TASKS.md','.ai-memory/handoff.md','.ai-memory/project_memory.md']


def git(*args):return subprocess.check_output(['git',*args],cwd=ROOT,stderr=subprocess.PIPE)


def main():
    def read(path):return json.loads((OUT/path).read_text(encoding='utf-8-sig'))
    q=read('quality-results.json'); m=read('build-matrix.json')
    assert q['passed'] and q['state']=='complete' and len(q['foundation'])==6 and len(q['inherited'])==41
    assert all(r['exitCode']==r.get('expectedExitCode',0) for r in q['foundation']+q['inherited'])
    assert not q['applicationMediaStarted'] and q['newPhysicalTwoPcTest']=='NOT_RUN'
    assert m['passed'] and len(m['profiles'])==8 and not m['applicationStarted']
    assert m['profiles'][0]['desktopFeatures']==[] and not m['profiles'][0]['optionalWebRtcDependency']
    assert m['profiles'][-1]['syntheticStream']
    def log(name):return (OUT/'automatic'/name).read_text(encoding='utf-8-sig')
    def rust_counts(name):
        rows=re.findall(r'test result: ok\. (\d+) passed; 0 failed; (\d+) ignored',log(name))
        assert rows
        return dict(passed=sum(int(r[0]) for r in rows),ignored=sum(int(r[1]) for r in rows))
    all_rust=rust_counts('test.txt');default_rust=rust_counts('default-unit.txt')
    assert all_rust==dict(passed=82,ignored=1) and default_rust==dict(passed=56,ignored=1)
    assert re.search(r'Tests\s+34 passed',log('frontend-unit.txt'))
    assert re.search(r'pass 9',log('build-matrix-unit.txt'))
    assert 'Ran 12 tests' in log('python-lifecycle-unit.txt')
    browser=read('browser-isolation/result.json');assert browser['exitCode']==0
    assert '3 passed' in (OUT/'browser-isolation/browser.txt').read_text(encoding='utf-8-sig')
    screenshot=OUT/'browser-isolation/browser-preview.png';assert screenshot.is_file()
    assert (ROOT/'artifacts/browser-preview.png').read_bytes()==git('show',BASE+':artifacts/browser-preview.png')
    core=['apps/desktop/src','apps/desktop/src-tauri','crates','Cargo.toml','Cargo.lock','pnpm-lock.yaml','pnpm-workspace.yaml']
    assert not git('diff','--name-only',BASE,'--',*core).strip()
    previous=json.loads(git('show',BASE+':package.json'))
    package=json.loads((ROOT/'package.json').read_text(encoding='utf-8-sig'))
    assert package['devDependencies']==previous['devDependencies'] and package['packageManager']==previous['packageManager']
    tasks=(ROOT/'TASKS.md').read_text(encoding='utf-8')
    p0=re.findall(r'^- \[([ x])\] `P0-(0[1-7])`',tasks,re.M)
    assert len(p0)==7 and all(mark=='x' for mark,_ in p0)
    p1=dict((num,mark=='x') for mark,num in re.findall(r'^- \[([ x])\] `P1-(0[1-6])`',tasks,re.M))
    assert p1=={'01':True,'02':False,'03':False,'04':False,'05':False,'06':False}
    allowed=set(DOCS+['package.json','apps/desktop/tests/preview.spec.ts',
        'scripts/p101.ps1','scripts/p101-checks.mjs','scripts/p101-evidence.py',
        'scripts/quality/desktop-builds.json','scripts/quality/build-matrix.mjs','scripts/quality/build-matrix.test.mjs'])
    names=set(git('diff','--name-only',BASE).decode('utf-8').splitlines())
    names.update(git('ls-files','--others','--exclude-standard').decode('utf-8').splitlines())
    assert all(n in allowed or n.startswith('artifacts/p101/') for n in names)
    links=0
    for name in DOCS:
        p=ROOT/name
        for url in re.findall(r'\[[^\]]+\]\(([^)]+)\)',p.read_text(encoding='utf-8')):
            if re.match(r'^[a-z]+:',url) or url.startswith('#'):continue
            target=(p.parent/url.strip('<>').split('#')[0]).resolve();target.relative_to(ROOT)
            assert target.exists() or target==OUT/'verification.json'
            links+=1
    sources=[ROOT/'scripts/p101-checks.mjs',ROOT/'scripts/p101.ps1']+list((ROOT/'scripts/quality').glob('*'))
    for p in sources:check_text(p.read_text(encoding='utf-8'),payloads=False)
    check_files([pathlib.Path(__file__)],source=True)
    check_files([ROOT/n for n in DOCS])
    text_files=[p for p in OUT.rglob('*') if p.suffix in ['.json','.txt'] and p.name!='verification.json']
    check_files(text_files)
    git('diff','--check')
    exe=ROOT/'target/probe/remotehub-desktop.exe';assert exe.is_file()
    value=dict(recordedAt=datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=8))).isoformat(),
        baselineCommit=BASE,passed=True,qualityCommands=47,reusedSafetyCommands=41,foundationCommands=6,
        buildConfigurations=8,buildMatrixTests=9,rustAllFeatures=all_rust,rustDefault=default_rust,
        frontendTests=34,edgeCases=3,edgeAfterEvidenceIsolationRerun=True,pythonLifecycleTests=12,
        privateMfTest='ignored in each Rust mode; no real desktop sample used',
        defaultWindowsNativeExeSha256=hashlib.sha256(exe.read_bytes()).hexdigest(),
        defaultWindowsNativeExeBytes=exe.stat().st_size,newDistributablePackage=False,
        relativeLinksChecked=links,privacyTextEvidenceFiles=len(text_files),privacyScannerSelfChecked=True,
        evidenceHashFormat='SHA256 of UTF-8 LF text without BOM',
        textEvidenceSha256={p.relative_to(OUT).as_posix():hashlib.sha256(p.read_text(encoding='utf-8-sig').encode('utf-8')).hexdigest() for p in text_files},
        oldPreviewUnchanged=True,oldReportsAndEvidenceUnchanged=True,mediaRustVueCodeAndPermissionsUnchanged=True,
        dependencyLocksUnchanged=True,cloudContacted=False,realDesktopCaptured=False,inputInjected=False,
        newTwoPcTest='NOT_RUN',nativeGuiStarted='NOT_RUN; UI code unchanged',
        phase0Exit='APPROVED_LIMITED',R06RiskAcceptance='ACCEPTED_FOR_LIMITED_EXIT',R06TechnicalStatus='OPEN_NOT_REPRODUCED',
        phase1ImplementedTask='P1-01',P102Started=False)
    check_text(json.dumps(value))
    (OUT/'verification.json').write_text(json.dumps(value,indent=2)+'\n',encoding='utf-8')
    print(json.dumps({k:value[k] for k in ['passed','qualityCommands','buildConfigurations','buildMatrixTests','rustAllFeatures','rustDefault','frontendTests','edgeCases','P102Started']}))


if __name__=='__main__':
    try:main()
    except Exception:raise SystemExit('P101_EVIDENCE_CHECK_FAILED_NO_PRIVATE_ECHO') from None
