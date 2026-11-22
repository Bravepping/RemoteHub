"""Offline P1-02 evidence/scope/privacy checks. Does not open a client, capture or connection."""
import datetime
import hashlib
import json
import pathlib
import re
import subprocess
import sys
from network_privacy import check_files,check_text

ROOT=pathlib.Path(__file__).resolve().parent.parent
OUT=ROOT/'artifacts/p102'
BASE='fdfb9e359ee9a69ab46746ca2d97c2a8a1b99991'
DOCS=['docs/P102_PLAN.md','docs/P102_TEST_REPORT.md','docs/ADR/0017-p102-local-navigation-shell.md',
      'docs/DEVELOPMENT_GUIDE.md','docs/PHASE1_DESKTOP_IMPLEMENTATION_PLAN.md','docs/PHASE0_RISK_REGISTER.md',
      'README.md','TASKS.md','.ai-memory/handoff.md','.ai-memory/project_memory.md']
STAGE='start'


def git(*args):return subprocess.check_output(['git',*args],cwd=ROOT,stderr=subprocess.PIPE)


def main():
    global STAGE
    def read(name):return json.loads((OUT/name).read_text(encoding='utf-8-sig'))
    STAGE='quality'
    q=read('quality-results.json');m=read('build-matrix.json')
    assert q['passed'] and q['state']=='complete' and len(q['inherited'])==41 and len(q['foundation'])==6
    assert all(r['exitCode']==r.get('expectedExitCode',0) for r in q['inherited']+q['foundation'])
    assert m['passed'] and len(m['profiles'])==8 and not m['profiles'][0]['optionalWebRtcDependency']
    def log(name):return (OUT/'automatic'/name).read_text(encoding='utf-8-sig')
    def counts(name):
        rows=re.findall(r'test result: ok\. (\d+) passed; 0 failed; (\d+) ignored',log(name));assert rows
        return dict(passed=sum(int(a) for a,_ in rows),ignored=sum(int(b) for _,b in rows))
    assert counts('test.txt')==dict(passed=82,ignored=1)
    assert counts('default-unit.txt')==dict(passed=56,ignored=1)
    assert re.search(r'Tests\s+43 passed',log('frontend-unit.txt'))
    assert re.search(r'9 passed',log('browser.txt')) and 'Ran 12 tests' in log('python-lifecycle-unit.txt')
    path_negatives=read('browser-path-negatives.json')
    assert path_negatives['passed'] and len(path_negatives['cases'])==2
    assert {r['name'] for r in path_negatives['cases']}=={'parent-traversal','cross-drive'}
    assert all(r['exitCode']!=0 and r['rejectedBeforeServerStart'] for r in path_negatives['cases'])
    browser_final=read('browser-final-guard.json')
    assert browser_final['passed'] and browser_final['exitCode']==0
    assert '9 passed' in (OUT/'browser-final-guard.txt').read_text(encoding='utf-8')
    STAGE='visual-and-native'
    visuals=read('visuals.json');assert len(visuals['screenshots'])==8
    for name,digest in visuals['screenshots'].items():
        assert hashlib.sha256((OUT/'screenshots'/name).read_bytes()).hexdigest()==digest
    native=read('native/result.json');home=read('native/home.json');settings=read('native/settings.json')
    assert native['observedPages']==['home','settings'] and not native['automaticNavigationCompleted']
    assert native['windowLeftAvailableForUser'] and not native['applicationCaptureOrRemoteControlInvoked']
    assert '本机核心已连接' in home['text'] and '远程访问关闭' in home['text']
    assert 'windows' in settings['text'] and 'x86_64' in settings['text'] and '0.1.0' in settings['text']
    assert not list((OUT/'native').glob('*.png'))
    assert read('native/screenshot-limitation.json')['imageDiscarded']
    STAGE='scope'
    core=['crates','apps/desktop/src-tauri/src','apps/desktop/src-tauri/capabilities','apps/desktop/src-tauri/build.rs',
          'Cargo.toml','Cargo.lock','pnpm-lock.yaml','pnpm-workspace.yaml','apps/desktop/src/bridge.ts',
          'apps/desktop/src/features/p004','apps/desktop/src/features/p006','apps/desktop/src/style.css']
    assert not git('diff','--name-only',BASE,'--',*core).strip()
    conf=json.loads((ROOT/'apps/desktop/src-tauri/tauri.conf.json').read_text(encoding='utf-8'))
    previous=json.loads(git('show',BASE+':apps/desktop/src-tauri/tauri.conf.json'))
    previous['app']['windows'][0]['title']='RemoteHub';assert conf==previous
    old_app=git('show',BASE+':apps/desktop/src/App.vue').decode('utf-8-sig')
    legacy=(ROOT/'apps/desktop/src/features/experiments/ExperimentWorkspace.vue').read_text(encoding='utf-8')
    assert legacy.split('<template>',1)[1].strip()==old_app.split('<template>',1)[1].strip()
    old_package=json.loads(git('show',BASE+':package.json'))
    package=json.loads((ROOT/'package.json').read_text(encoding='utf-8'))
    assert old_package['devDependencies']==package['devDependencies']
    assert not git('diff','--name-only',BASE,'--','artifacts/p101','artifacts/p007-final','artifacts/r06-online').strip()
    allowed=set(DOCS+['.gitignore','package.json','apps/desktop/scripts/e2e.mjs','apps/desktop/src-tauri/tauri.conf.json',
        'apps/desktop/src/App.vue','apps/desktop/src/App.test.ts','apps/desktop/src/ExperimentWorkspace.test.ts',
        'apps/desktop/tests/preview.spec.ts','apps/desktop/tests/navigation.spec.ts',
        'scripts/p101-checks.mjs','scripts/p102.ps1','scripts/p102-checks.mjs','scripts/p102-evidence.py'])
    names=set(git('diff','--name-only',BASE).decode('utf-8').splitlines())
    names.update(git('ls-files','--others','--exclude-standard').decode('utf-8').splitlines())
    assert all(n in allowed or n.startswith(('apps/desktop/src/features/desktop/','apps/desktop/src/features/experiments/','artifacts/p102/')) for n in names)
    tasks=(ROOT/'TASKS.md').read_text(encoding='utf-8')
    p1=dict((n,m=='x') for m,n in re.findall(r'^- \[([ x])\] `P1-(0[1-6])`',tasks,re.M))
    assert p1=={'01':True,'02':True,'03':False,'04':False,'05':False,'06':False}
    STAGE='links-and-privacy'
    links=0
    for name in DOCS:
        p=ROOT/name
        for url in re.findall(r'\[[^\]]+\]\(([^)]+)\)',p.read_text(encoding='utf-8')):
            if re.match(r'^[a-z]+:',url) or url.startswith('#'):continue
            target=(p.parent/url.strip('<>').split('#')[0]).resolve();target.relative_to(ROOT)
            assert target.exists() or target==OUT/'verification.json';links+=1
    sources=[ROOT/n for n in names if pathlib.Path(n).suffix in ['.mjs','.ts','.vue','.css','.ps1'] and (ROOT/n).is_file()]
    for p in sources:check_text(p.read_text(encoding='utf-8'),payloads=False)
    check_files([pathlib.Path(__file__)],source=True);check_files([ROOT/n for n in DOCS])
    evidence=[p for p in OUT.rglob('*') if p.suffix in ['.json','.txt'] and p.name!='verification.json' and 'first-browser' not in p.parts and 'browser-results' not in p.parts]
    check_files(evidence);git('diff','--check')
    value=dict(recordedAt=datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=8))).isoformat(),baselineCommit=BASE,
        passed=True,pages=['home','devices','connect','sessions','settings'],qualityCommands=47,frontendTests=43,edgeTests=9,
        browserEvidencePathNegativeTests=2,edgeRerunAfterPathGuardPassed=True,
        rustAllFeatures=counts('test.txt'),rustDefault=counts('default-unit.txt'),pythonLifecycleTests=12,buildMatrixTests=9,
        actualCargoProfiles=8,realBrowserScreenshots=8,nativeObservedPages=native['observedPages'],
        nativeFivePageAutomaticClickTest='INCOMPLETE: concurrent human input guard; not marked PASS',
        nativePixelEvidence='NOT_AVAILABLE: helper returned non-product content; file discarded, no native screenshots committed',
        nativeWindowLeftForUser=True,applicationDesktopCaptureInvoked=False,applicationRemoteInputInvoked=False,
        cloudContacted=False,newTwoPcMediaTest='NOT_RUN',cspPermissionsAndRustMediaUnchanged=True,dependencyLocksUnchanged=True,
        legacyExperimentTemplateUnchanged=True,oldEvidenceUnchanged=True,defaultWindowTitleOnlyChanged=True,
        relativeLinksChecked=links,privacySourceFiles=len(sources)+1,privacyTextEvidenceFiles=len(evidence),
        evidenceHashFormat='SHA256 of UTF-8 LF text without BOM',textEvidenceSha256={p.relative_to(OUT).as_posix():hashlib.sha256(p.read_text(encoding='utf-8-sig').encode('utf-8')).hexdigest() for p in evidence},
        defaultNativeExeSha256=hashlib.sha256((ROOT/'target/probe/remotehub-desktop.exe').read_bytes()).hexdigest(),
        R06TechnicalStatus='OPEN_NOT_REPRODUCED',implementedTask='P1-02',P103Started=False,P104Started=False)
    check_text(json.dumps(value));(OUT/'verification.json').write_text(json.dumps(value,indent=2)+'\n',encoding='utf-8')
    print(json.dumps({k:value[k] for k in ['passed','qualityCommands','frontendTests','edgeTests','nativeObservedPages','P103Started']}))


if __name__=='__main__':
    try:main()
    except Exception:
        print(json.dumps(dict(passed=False,stage=STAGE,error='P102_EVIDENCE_FAILED_NO_PRIVATE_ECHO')),file=sys.stderr)
        raise SystemExit(1) from None
