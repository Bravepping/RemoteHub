"""Offline P2-01 evidence/scope audit. Never creates an identity or launches UI/network."""
import datetime
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tomllib
from network_privacy import check_files, check_text

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT/'artifacts/p201'
BASE = 'd7ce2e0ffd580a9bb29a7252311c4f3e2676cfbd'
DOCS = ['README.md','TASKS.md','THIRD_PARTY_NOTICES.md','docs/PHASE1_EXIT_DECISION.md',
        'docs/P201_PLAN.md','docs/P201_IDENTITY_CONTRACT.md','docs/P201_IDENTITY_GUIDE.md','docs/P201_TEST_REPORT.md',
        'docs/ADR/0022-local-ed25519-dpapi-identity.md','docs/DEVELOPMENT_GUIDE.md',
        'docs/PHASE1_DESKTOP_IMPLEMENTATION_PLAN.md','.ai-memory/handoff.md','.ai-memory/project_memory.md']
SOURCES = ['Cargo.toml','Cargo.lock','package.json','apps/desktop/src-tauri/Cargo.toml',
           'apps/desktop/src-tauri/src/main.rs','apps/desktop/src-tauri/src/identity.rs',
           'apps/desktop/src/ipc/contracts.ts','apps/desktop/src/ipc/identity.test.ts',
           'apps/desktop/src/features/desktop/IdentityReadout.vue','apps/desktop/src/features/desktop/IdentityReadout.test.ts',
           'apps/desktop/src/features/desktop/LocalDeviceCard.vue','apps/desktop/src/features/desktop/pages/SettingsPage.vue',
           'apps/desktop/tests/p201.spec.ts','scripts/p201-checks.mjs','scripts/p201.ps1',
           'scripts/p201-native.ps1','scripts/p201-native.py','scripts/p201-evidence.py']
STAGE = 'start'


def git(*args):
    return subprocess.check_output(['git','-c','safe.directory='+ROOT.as_posix(),*args],cwd=ROOT,stderr=subprocess.PIPE)


def main():
    global STAGE
    def text(name): return (ROOT/name).read_text(encoding='utf-8-sig')
    def read(name): return json.loads((OUT/name).read_text(encoding='utf-8-sig'))
    def log(name): return (OUT/'automatic'/name).read_text(encoding='utf-8-sig')
    def counts(name):
        rows=re.findall(r'test result: ok\. (\d+) passed; 0 failed; (\d+) ignored',log(name))
        assert rows
        return dict(passed=sum(int(a) for a,_ in rows),ignored=sum(int(b) for _,b in rows))
    STAGE='actual-checks'
    q=read('quality-results.json')
    assert q['passed'] and q['state']=='complete'
    assert len(q['foundation'])==6 and len(q['inherited'])==41
    assert all(r['exitCode']==r.get('expectedExitCode',0) for r in q['foundation']+q['inherited'])
    assert counts('test.txt')==dict(passed=95,ignored=1)
    assert counts('default-unit.txt')==dict(passed=77,ignored=1)
    assert re.search(r'Tests\s+100 passed',log('frontend-unit.txt'))
    assert '20 passed' in log('browser.txt') and 'Ran 12 tests' in log('python-lifecycle-unit.txt')
    assert read('build-matrix.json')['passed'] and len(read('build-matrix.json')['profiles'])==8
    permissions=read('permissions.json')
    assert permissions['passed'] and permissions['addedAllowedCommands']==0 and permissions['jsTrayAndMenuCommandsAllowed']==0
    assert 'pass 7' in (OUT/'permission-tests.txt').read_text(encoding='utf-8-sig')
    privacy=read('privacy/results.json')
    assert privacy['passed'] and all(r['passed'] and r['exitCode']==0 for r in privacy['results'])
    assert [(r['name'],r['expectedTests']) for r in privacy['results']]==[('privacy',12),('delivery',9)]
    STAGE='real-dpapi-and-regression'
    native=read('native-restart.json')
    assert native['passed'] and native['ciphertextUnchanged'] and native['temporaryStoreRemoved']
    assert [r['stage'] for r in native['results']]==['prepare','restore']
    assert all(r['exitCode']==0 and r['native']['passed'] and not r['native']['identityOrKeyPrinted'] for r in native['results'])
    assert len({r['native']['processId'] for r in native['results']})==2
    assert not native['productionIdentityDirectoryAccessed'] and not native['guiStarted'] and not native['cloud']
    native_log=(OUT/'identity-final-native.txt').read_text(encoding='utf-8-sig')
    assert '13 passed; 0 failed' in native_log and 'EXIT=0' in native_log
    before=(OUT/'marker-before.txt').read_text(encoding='utf-8-sig')
    assert 'missing_confirmed_key_is_not_silently_regenerated ... FAILED' in before and '0 passed; 1 failed' in before
    assert 'test windows::tests::missing_confirmed_key_is_not_silently_regenerated ... ok' in log('test.txt')
    assert 'test windows::tests::substituted_valid_dpapi_identity_is_rejected ... ok' in log('test.txt')
    cleanup=read('cleanup.json')
    assert cleanup['remainingWorkspaceTestStores']==0 and cleanup['remainingProbeProcesses']==0 and not cleanup['productionIdentityTouched']
    STAGE='dependencies-and-source-scope'
    deps=read('dependencies.json')
    assert deps['existingLockedPackageVersionsUnchanged'] and deps['newRegistryPackageCount']==11 and deps['curveVersion']=='4.1.3'
    assert deps['osv']['status']=='COMPLETED' and deps['osv']['matchedAdvisoryCount']==0
    assert 'zeroize' in deps['ed25519DalekResolvedFeatures']
    assert not set(deps['ed25519DalekResolvedFeatures'])&{'hazmat','legacy_compatibility','serde','pkcs8'}
    old=tomllib.loads(git('show',BASE+':Cargo.lock').decode('utf-8'))['package']
    new=tomllib.loads(text('Cargo.lock'))['package']
    key=lambda p:(p['name'],p['version'],p.get('source',''))
    assert {key(p) for p in old}<={key(p) for p in new}
    unchanged=['crates/hub-capture-win','crates/hub-encode-win','crates/hub-webrtc','crates/hub-turn-credentials',
               'crates/hub-probe-wire','crates/hub-rendezvous-probe','pnpm-lock.yaml',
               'apps/desktop/src-tauri/capabilities','apps/desktop/src-tauri/permissions','apps/desktop/src-tauri/build.rs',
               'apps/desktop/src-tauri/tauri.conf.json','apps/desktop/src-tauri/src/lifecycle.rs','apps/desktop/src-tauri/src/lifecycle_policy.rs',
               'apps/desktop/src-tauri/src/experiment_origin.rs','apps/desktop/src-tauri/src/p004.rs',
               'apps/desktop/src-tauri/src/p004_lan.rs','apps/desktop/src-tauri/src/p006.rs','apps/desktop/src-tauri/src/p007b_diag.rs',
               'apps/desktop/src/features/experiments','apps/desktop/src/features/p004','apps/desktop/src/features/p006',
               'apps/desktop/src/state','apps/desktop/src/ipc/tauriDesktopApi.ts',
               'docs/P106_TEST_REPORT.md','docs/P106_MANUAL_TEST_REPORT.md','docs/PHASE1_FINAL_EXIT_REVIEW.md']
    assert not git('diff','--name-only',BASE,'--',*unchanged).strip()
    changes=set(git('diff','--name-only',BASE).decode('utf-8').splitlines())
    changes.update(git('ls-files','--others','--exclude-standard').decode('utf-8').splitlines())
    assert all(n in DOCS+SOURCES or n.startswith('crates/hub-identity/') or n.startswith('artifacts/p201/') for n in changes)
    assert all(n.startswith('artifacts/p201/') for n in git('diff','--name-only',BASE,'--','artifacts').decode('utf-8').splitlines())
    main_source=text('apps/desktop/src-tauri/src/main.rs')
    assert '#[cfg(not(any(feature = "p004-probe", feature = "p006-turn-probe")))]\n    let builder = builder.manage(identity::DesktopIdentity::initialize());' in main_source
    core=text('crates/hub-identity/src/lib.rs')
    assert re.search(r'pub struct Identity\s*\{\s*key: SigningKey,\s*\}',core)
    assert 'impl Serialize for Identity' not in core and 'impl fmt::Debug for Identity' not in core
    windows=text('crates/hub-identity/src/windows.rs')
    assert 'CRYPTPROTECT_LOCAL_MACHINE' not in windows and 'CRYPTPROTECT_UI_FORBIDDEN' in windows
    assert 'COMMIT_MAGIC' in windows and 'verify_or_commit' in windows and 'if committed.is_some()' in windows
    tasks=text('TASKS.md')
    assert '- [x] `P2-01`' in tasks and not re.search(r'^- \[x\] `P2-0[2-6]`',tasks,re.M)
    assert '批准 Phase 1 限定退出' in text('docs/PHASE1_EXIT_DECISION.md')
    STAGE='privacy-links-and-hashes'
    source_files=[ROOT/n for n in changes if (ROOT/n).is_file() and Path(n).suffix in ['.rs','.ts','.vue','.mjs','.ps1']]
    for path in source_files: check_text(path.read_text(encoding='utf-8'),payloads=False)
    python_files=[ROOT/n for n in SOURCES if n.endswith('.py')]
    check_files(python_files,source=True)
    check_files([ROOT/n for n in DOCS])
    evidence=[p for p in OUT.rglob('*') if p.suffix in ['.txt','.json'] and p.name!='verification.json' and 'browser-results' not in p.parts]
    check_files(evidence)
    links=0
    for name in DOCS:
        path=ROOT/name
        for url in re.findall(r'\[[^\]]+\]\(([^)]+)\)',path.read_text(encoding='utf-8')):
            if re.match(r'^[a-z]+:',url) or url.startswith('#'):continue
            target=(path.parent/url.strip('<>').split('#')[0]).resolve();target.relative_to(ROOT)
            assert target.exists() or target==OUT/'verification.json';links+=1
    bundles=list((ROOT/'apps/desktop/dist/assets').glob('*.js'))
    assert bundles and all('P201-TEST-FIXTURE' not in p.read_text(encoding='utf-8') and 'RH-0123-4567-89AB' not in p.read_text(encoding='utf-8') for p in bundles)
    digest=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
    assert digest(ROOT/'target/probe/examples/restart_probe.exe')==native['programSha256']
    git('diff','--check')
    result=dict(recordedAt=datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=8))).isoformat(),passed=True,baselineCommit=BASE,
                task='P2-01',qualityCommands=47,frontendTests=100,edgeTests=20,rustAllFeatures=counts('test.txt'),rustDefault=counts('default-unit.txt'),
                identityCoreTests=13,realDpapi='PASS_CURRENT_WINDOWS_USER',restartSeparateProcesses=2,markerBeforeFailed=True,markerAfterPassed=True,
                capabilityAndCspUnchanged=True,newIpcCommands=0,permissions=permissions,privacyTests=12,deliveryTests=9,
                oldPackageVersionsUnchanged=True,newRegistryPackages=11,incrementalOsvMatchedAdvisories=0,historicalEvidenceUnchanged=True,
                phase1Exit='USER_APPROVED_LIMITED_EXIT',P202Started=False,R06='OPEN_NOT_REPRODUCED',c0='PENDING_USER_DECISIONS',
                guiIdentityPage='NOT_RUN',crossWindowsAccount='NOT_RUN',crossMachine='NOT_RUN',junctionCreation='NOT_RUN',
                productionIdentityAccessed=False,actualDesktopCapture=False,remoteInput=False,cloudServiceStarted=False,thirdPartyBackendStarted=False,
                defaultExeSha256=digest(ROOT/'target/probe/remotehub-desktop.exe'),nativeProbeExeSha256=native['programSha256'],relativeLinksChecked=links,
                textEvidenceSha256={p.relative_to(OUT).as_posix():hashlib.sha256(p.read_text(encoding='utf-8-sig').encode('utf-8')).hexdigest() for p in evidence},
                evidenceHashFormat='SHA256 of UTF-8 LF text without BOM')
    check_text(json.dumps(result))
    (OUT/'verification.json').write_text(json.dumps(result,indent=2)+'\n',encoding='utf-8',newline='\n')
    print(json.dumps({k:result[k] for k in ['passed','task','qualityCommands','rustAllFeatures','rustDefault','realDpapi','P202Started']}))


if __name__=='__main__':
    try:main()
    except Exception:
        print(json.dumps(dict(passed=False,stage=STAGE,error='P201_EVIDENCE_FAILED_NO_PRIVATE_ECHO')),file=sys.stderr)
        raise SystemExit(1) from None
