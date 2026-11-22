"""Offline Phase 0 final review: immutable Git evidence, pending decisions, docs and privacy."""
import datetime
import hashlib
import json
import pathlib
import re
import subprocess
import sys
from network_privacy import SafeArgumentParser, check_files, check_text

ROOT=pathlib.Path(__file__).resolve().parent.parent
BASE='7fde54e6d94f2eb9ef2697e437cccfd42f4f67c9'
OUT=ROOT/'artifacts/p007-final'
DOCS=['docs/P007_FINAL_REVIEW_PLAN.md','docs/PHASE0_FINAL_EVIDENCE_MATRIX.md',
      'docs/PHASE0_RISK_REGISTER.md','docs/PHASE0_FINAL_EXIT_REVIEW.md',
      'docs/COMPATIBILITY_INTEGRATION_ASSESSMENT.md','docs/PHASE1_DESKTOP_IMPLEMENTATION_PLAN.md',
      'docs/DEVELOPMENT_GUIDE.md','docs/ADR/0014-phase0-final-review-and-risk-gates.md',
      'docs/ADR/0015-client-backend-and-compatibility-plan.md']
CURRENT=['README.md','TASKS.md','.ai-memory/handoff.md','.ai-memory/project_memory.md']
STAGE='arguments'


def git(*args):
    return subprocess.check_output(['git',*args],cwd=ROOT,stderr=subprocess.PIPE)


def text_sha(text):return hashlib.sha256(text.replace('\r\n','\n').lstrip('\ufeff').encode('utf-8')).hexdigest()


def collect():
    global STAGE
    sources={};checks=[]
    def blob(path):
        data=git('show',BASE+':'+path);sources[path]=hashlib.sha256(data).hexdigest()
        return data.decode('utf-8-sig')
    def read(path):return json.loads(blob(path))
    def passed(name,**details):checks.append(dict(name=name,passed=True,**details))
    STAGE='historical-sources'
    old=read('artifacts/p007a/evidence-index.json')
    for name,expected in old['sources'].items():
        blob(name);assert sources[name]==expected
    assert len(old['observations'])==13 and len(old['sources'])==72
    passed('historical-git-source-hashes',groups=13,files=72)
    blob('docs/PHASE0_P007B_DECISION.md');blob('docs/ADR/0013-p007b-r06-diagnostic-package.md')
    STAGE='r06-sources'
    v=read('artifacts/r06-online/verdict.json')
    for name,expected in v['evidenceSha256'].items():
        assert text_sha(blob('artifacts/r06-online/'+name))==expected
    a=read('artifacts/r06-online/sender-summary.json');b=read('artifacts/r06-online/receiver-summary.json')
    assert a['source']['sourceKind']=='synthetic_nv12' and a['source']['frames_sent']==240
    diag=a['source']['directPairDiagnostic']
    assert diag['approved'] and all(diag[s]['reason']=='matched' and diag[s]['exact_match_count']==1 for s in ['local','remote'])
    for name in ['receiverPreClose','receiverFinal']:
        r=b[name];s=r['snapshot'];assert r['capture']=='live' and r['sampledAtMs']<=r['recordedAtMs']
        assert [s[k] for k in ['framesReceived','framesDecoded','framesRendered']]==[240]*3
    assert all(b['receiverFinal']['cleanup'].values()) and v['historicalFailure']=='NOT_REPRODUCED'
    assert v['historicalRootCause']=='NOT_DETERMINED' and not v['R06Closed']
    assert v['roundsExecuted']==1 and not v['phase1Started']
    release=read('artifacts/r06-online/allocation-verdict-before-stop.json')
    assert len(release)==2 and all(x['classification']=='refresh_zero_immediate' for x in release)
    assert v['classificationPrecededServiceStop'] and v['observedConcurrentAllocationPeak']==2
    assert v['cloudStoppedNoAutostart'] and v['temporaryCredentialsCleaned'] and all(v['baselineUnchanged'].values())
    blob('docs/P007B_R06_ONLINE_DIAGNOSTIC_REPORT.md')
    passed('r06-archived-real-evidence',frames=240,rounds=1,allocations=2,historicalRootCause='NOT_DETERMINED')
    STAGE='privacy-archive'
    privacy=read('artifacts/p007b-privacy/privacy-checks.json');regression=read('artifacts/p007b-privacy/summary.json')
    read('artifacts/p007b-privacy/published.json');blob('docs/PHASE0_P007B_PRIVACY_FIX_REPORT.md')
    assert privacy['passed'] and privacy['privacyTests']==12 and privacy['deliveryTests']==9
    assert privacy['previousGitExposureStillReachable'] and not privacy['historyRewritten']
    assert regression['automaticCommands']==41 and len(regression['localCases'])==5
    passed('accepted-privacy-delivery-evidence',newRuntimeTests=False)
    STAGE='code-boundaries'
    code_refs=['Cargo.toml','apps/desktop/package.json','apps/desktop/src/App.vue','apps/desktop/src/bridge.ts',
        'apps/desktop/src/features/p004/lan.ts','apps/desktop/src/features/p006/relay.ts',
        'apps/desktop/src-tauri/src/main.rs','crates/hub-webrtc/src/session.rs']
    for name in code_refs:blob(name)
    deps=read('apps/desktop/package.json')['dependencies']
    assert 'pinia' not in deps and 'vue-router' not in deps
    passed('planned-vs-existing-boundaries',sourceReferences=len(code_refs),phase1DependenciesAdded=False)
    STAGE='task-gates'
    tasks=(ROOT/'TASKS.md').read_text(encoding='utf-8')
    parents={}
    for mark,num in re.findall(r'^- \[([ x])\] `P0-(0[1-7])`',tasks,re.M):parents['P0-'+num]=mark=='x'
    assert len(parents)==7 and sum(parents.values())==5 and parents['P0-04']
    assert not parents['P0-06'] and not parents['P0-07']
    p1=re.findall(r'^- \[([ x])\] `P1-(0[1-6])`',tasks,re.M)
    assert len(p1)==6 and all(mark==' ' for mark,_ in p1)
    passed('e04-and-pending-exit-state',completedParents=5,totalParents=7,phase1TasksNotStarted=6)
    STAGE='local-target-metadata'
    inv_text=(OUT/'compatibility-local-inventory.json').read_text(encoding='utf-8-sig');inv=json.loads(inv_text)
    assert len(inv['packages'])==4 and not any(inv[k] for k in ['signatureVerified','targetSourceRevisionPinned','interoperabilityTested','installerExecuted','cloudContacted'])
    assert all(re.fullmatch('[0-9a-f]{64}',p['sha256']) for p in inv['packages'])
    moon=next(p for p in inv['packages'] if p['name'].startswith('Moonlight-'))
    assert moon['productVersionComponents']==[6,5,6,0]
    passed('offline-candidate-metadata-only',packages=4,verifiedInterop=False)
    STAGE='change-scope'
    allowed=set(DOCS+CURRENT+['scripts/p007-final-review.py'])
    names=set(git('diff','--name-only',BASE).decode('utf-8').splitlines())
    names.update(git('ls-files','--others','--exclude-standard').decode('utf-8').splitlines())
    assert all(n in allowed or (n.startswith('artifacts/p007-final/') and pathlib.Path(n).suffix in ['.json','.txt']) for n in names)
    assert not any(n in sources for n in names)
    git('merge-base','--is-ancestor',BASE,'HEAD')
    git('diff','--check')
    passed('docs-only-forward-change-scope',changedPaths=len(names),historicalFilesChanged=False,productCodeChanged=False)
    STAGE='relative-links'
    links=0
    pending={OUT/'evidence-index.json',OUT/'verification.json'}
    for name in DOCS+['README.md','TASKS.md']:
        p=ROOT/name
        for url in re.findall(r'\[[^\]]+\]\(([^)]+)\)',p.read_text(encoding='utf-8')):
            if re.match(r'^[a-z]+:',url) or url.startswith('#'):continue
            target=(p.parent/url.strip('<>').split('#')[0]).resolve()
            target.relative_to(ROOT)
            assert target.exists() or target in pending
            links+=1
    passed('relative-document-links',links=links)
    STAGE='privacy-self-check'
    source_count=check_files([pathlib.Path(__file__)],source=True)
    doc_count=check_files([ROOT/n for n in DOCS+CURRENT])
    check_files([OUT/'compatibility-local-inventory.json'])
    passed('privacy-and-scanner-self-check',sourceFiles=source_count,documents=doc_count)
    index=dict(version=1,reviewDate='2026-11-22',baselineCommit=BASE,
        scope='Offline final review of archived evidence, no new runtime/cloud/two-PC tests',
        historicalGroups=[{k:r[k] for k in ['id','scope','sources','limitations']} for r in old['observations']],
        historicalSourceFilesVerified=72,
        latestR06=dict(rounds=1,source='synthetic_nv12',framesSentReceivedDecodedRendered=[240]*4,
            strictLocalReason=diag['local']['reason'],strictRemoteReason=diag['remote']['reason'],
            receiverPreClose='live',receiverFinal='live',releaseClassifications=[r['classification'] for r in release],
            allocationPeak=2,classifiedBeforeStop=True,historicalFailure='NOT_REPRODUCED',historicalRootCause='NOT_DETERMINED',
            natural19MinuteExpiryTested=False,newLeaseTimeoutTested=False,newAbnormalPhysicalFinalTested=False),
        acceptedHistoricalRegression=dict(commands=41,rust=regression['rustPassed'],frontend=regression['frontendPassed'],
            privacy=privacy['privacyTests'],delivery=privacy['deliveryTests'],rerunThisReview=False),
        reviewState=dict(E04='APPROVED_EXCEPTION',R06='OPEN_NOT_REPRODUCED',R06RiskAcceptance='PENDING',
            formalPhase0Exit='HOLD_PENDING_USER_REVIEW',phase1Authorized=False,phase1Started=False,
            completedParents=5,totalParents=7,parentTasks=parents,compatibilityInterop='NOT_IMPLEMENTED_NOT_TESTED',
            decisionRecordIsNotUserSignature=True),
        localCandidateInventorySha256=text_sha(inv_text),targetSourceRevisionsPinned=False,
        sourceHashFormat='SHA256 of exact Git blobs at frozen baseline; candidate inventory uses UTF-8 LF text SHA256',
        sources=dict(sorted(sources.items())))
    check_text(json.dumps(index))
    return index,checks


def main():
    global STAGE
    parser=SafeArgumentParser(description=__doc__);parser.add_argument('--check',action='store_true');args=parser.parse_args()
    index,checks=collect()
    if args.check:
        assert json.loads((OUT/'evidence-index.json').read_text(encoding='utf-8-sig'))==index
    else:
        OUT.mkdir(exist_ok=True)
        (OUT/'evidence-index.json').write_text(json.dumps(index,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
        STAGE='final-evidence-privacy'
        artifact_count=check_files(p for p in OUT.iterdir() if p.suffix in ['.json','.txt'] and p.name!='verification.json')
        checks.append(dict(name='generated-evidence-privacy',passed=True,files=artifact_count))
        value=dict(recordedAt=datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=8))).isoformat(),
            baselineCommit=BASE,scope='Offline final review consistency and documentation checks',checks=checks,
            newRuntimeTests=False,newBuildLintClippy=False,cloudContacted=False,thirdPartyInstallerExecuted=False,
            productCodeChanged=False,historicalReportsChanged=False,historyRewritten=False,
            phase0ExitApproved=False,R06RiskAccepted=False,phase1Started=False)
        check_text(json.dumps(value))
        (OUT/'verification.json').write_text(json.dumps(value,indent=2)+'\n',encoding='utf-8')
    print(json.dumps(dict(mode='check' if args.check else 'record',checks=len(checks),
        historicalSourceFilesVerified=72,frozenSourceFiles=len(index['sources']),parentTasksCompleted=5,
        phase0='HOLD',R06RiskAcceptance='PENDING',phase1Started=False,cloudContacted=False)))


if __name__=='__main__':
    try:main()
    except Exception:
        print(json.dumps(dict(passed=False,stage=STAGE,error='FINAL_REVIEW_FAILED_NO_PRIVATE_ECHO')),file=sys.stderr)
        raise SystemExit(1) from None
