"""Check the limited Phase3 delivery; preserve failed attempts and incomplete coverage."""
from pathlib import Path
import datetime,hashlib,json,zipfile
from network_privacy import check_files
ROOT=Path(__file__).resolve().parent.parent
OUT=ROOT/'artifacts/p301'

def run():
    read=lambda name:json.loads((OUT/name).read_text(encoding='utf-8-sig'))
    q=read('quality-results.json');products=read('product-checks.json');package=read('package.json')
    assert q['passed']and q['state']=='complete'and len(q['foundation'])+len(q['inherited'])==47
    assert len(products)==5 and all(v['exitCode']==0 for v in products)
    assert read('permissions.json')['addedAllowedCommands']==0
    assert read('privacy/results.json')['passed']
    assert 'Ran 5 tests' in(OUT/'v3-summary-privacy.txt').read_text(encoding='utf-8')
    assert set(package['desktopFeatures'])=={'p3-lan','p005-control','p004-lan','p004-probe','custom-protocol'}
    for name,digest in package['sourceFilesSha256'].items():
        assert hashlib.sha256((ROOT/name).read_text(encoding='utf-8').encode()).hexdigest()==digest
    archive=ROOT/package['archiveRelativePath'];zip_checked=False
    if archive.exists():
        assert hashlib.sha256(archive.read_bytes()).hexdigest()==package['archiveSha256']
        with zipfile.ZipFile(archive)as z:
            assert z.namelist()==['RemoteHub-P3-LAN.exe','开始这里.txt','SHA256SUMS.txt']and z.testzip()is None
            assert hashlib.sha256(z.read('RemoteHub-P3-LAN.exe')).hexdigest()==package['programSha256']
        zip_checked=True
    rows=[v for v in read('local-summaries.json')if v['independentReceiverFinal']]
    assert len(rows)==3
    for row in rows:
        assert row['buildTag']=='p3-lan-v3'and row['authenticated']and row['resourcesReleased']and row['browserClosed']
        assert row['browserVideoWidth']==1280 and row['browserVideoHeight']==720
        assert row['browserFramesDecoded']>0 and row['browserPeerMatches']
        assert row['browserInputSent']==row['browserAckReceived']and row['browserPending']==0
        assert row['browserLastTxSeq']==row['browserLastRxSeq']==row['browserInputSent']
    assert any(v['browserAnomalies']==0 for v in rows)
    assert sum(v['browserControlError']=='CONTROL_CHANNEL_ERROR'for v in rows)==2
    assert read('first-attempt-local.json')[0]['browserFramesRendered']==0
    assert read('second-attempt-host-user-diagnostic.json')['sourceError']=='SPS_PROFILE_INCOMPATIBLE_42c01f'
    events=[json.loads(v)for v in(OUT/'user-results.jsonl').read_text(encoding='utf-8').splitlines()]
    assert any(v['stage']=='v3-first-real-input'and v['source']=='USER_RESPONSE'for v in events)
    assert any(v['stage']=='v3-cleanup-user-response'and v['answer']=='正常'for v in events)
    checked=check_files(p for p in OUT.rglob('*')if p.suffix in('.json','.jsonl','.txt'))
    source_checked=check_files([ROOT/'scripts'/name for name in('p3-package.py','p3-summary.py','p3-summary-test.py','p3-evidence.py')],source=True)
    return dict(recordedAt=datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=8))).isoformat(),
        limitedGoalEvidenceConsistent=True,applicationSourceMatchesPackage=True,privateArchiveCheckedLocally=zip_checked,
        independentReceiverSessions=3,observedFrames=[r['browserFramesDecoded']for r in rows],observedSentAck=[[r['browserInputSent'],r['browserAckReceived']]for r in rows],
        receiverResourcesReleasedAll=True,userConfirmedRealEnglishInput=True,userConfirmedFourCleanupScenarios=True,
        independentPerScenarioCleanupCoverage='LIMITED_NOT_FOUR_MAPPED_SHORT_ROUNDS',
        shutdownChannelErrorSessions=2,shutdownChannelErrorRisk='OPEN',hostIndependentFullReports='NOT_COLLECTED',
        fullPhase3Exit='NOT_CLAIMED',multiDpiPhysicalMatrix='NOT_RUN',historicalR06='OPEN',
        privacyEvidenceFilesChecked=checked,privacyToolSourcesChecked=source_checked,cloudStarted=False)

if __name__=='__main__':
    try:
        result=run()
        (OUT/'verification.json').write_text(json.dumps(result,indent=2)+'\n',encoding='utf-8',newline='\n')
        print(json.dumps(result))
    except Exception:raise SystemExit('PHASE3_EVIDENCE_CHECK_FAILED_NO_PRIVATE_ECHO')from None
