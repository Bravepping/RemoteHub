"""Recompute a limited R06 verdict from real sanitized evidence; never activates services."""
import datetime
import hashlib
import json
import pathlib
from network_privacy import check_files,check_text

ROOT=pathlib.Path(__file__).resolve().parent.parent
OUT=ROOT/'artifacts/r06-online'


def main():
    def read(name):return json.loads((OUT/name).read_text(encoding='utf-8-sig'))
    def text_sha(path):return hashlib.sha256(path.read_text(encoding='utf-8-sig').encode('utf-8')).hexdigest()
    a=read('activation.json'); g=read('guard-proof.json'); c=read('cleanup.json')
    pre=read('pre-start.json'); post=read('after-stop.json')
    sender=read('sender-summary.json'); receiver=read('receiver-summary.json')
    verdict=read('allocation-verdict-before-stop.json'); human=read('operator-confirmation.json')
    events=[json.loads(s) for s in (OUT/'turn-events-final.jsonl').read_text().splitlines()]
    snapshots=[e for e in events if e['event']=='socket_snapshot']
    expected='7deac839ee2e8fcc0e3ad47433f2431a0ce242bd3a297e90da0ef717effd79c0'
    assert sender['programSha256']==receiver['programSha256']==expected
    assert a['guardSmokePassed'] and a['guardTimerVerifiedBeforeCredentials'] and a['publicTlsVerified'] and a['nonRootWss']
    assert g['guard-verified.json']['timeNs'] < a['activatedAtNs']
    assert not g['guard-smoke-receipt.json']['failures'] and g['sourceMatches']
    assert a['authorizationDeadline']-a['authorizationStart']==1200
    assert a['guardTarget']-a['authorizationStart']==1140
    assert a['invitationExpiresAt']-a['authorizationStart']==480
    assert c['finishedAtNs']/1e9 < a['authorizationDeadline']
    assert not c['failures'] and c['temporarySecretFilesAbsent'] and all(c['dedicatedServicesStopped'].values())
    assert c==read('cleanup-server-receipt.json')
    assert not post['dedicatedListeners'] and post['units']['remotehub-turn-g6.service']['UnitFileState']=='disabled'
    assert all(post['units'][u]['LoadState']=='not-found' for u in ['remotehub-p007b-r06-wss.service','remotehub-p007b-r06-observer.service','remotehub-p007b-r06-expiry.timer','remotehub-p007b-r06-expiry.service'])
    baseline={k:pre[k]==post[k] for k in ['protectedFiles','firewallPolicySha256','businessContainerStartTimesSha256','runningServiceSetSha256']}
    assert all(baseline.values())
    assert sender['role']=='sender' and receiver['role']=='receiver'
    assert sender['nativeResourcesReleased'] and receiver['nativeResourcesReleased'] and sender['error'] is None and receiver['error'] is None
    source=sender['source'];d=source['directPairDiagnostic']
    assert source['sourceKind']=='synthetic_nv12' and source['frames_sent']==240 and source['resourcesReleased']
    assert d['approved'] and d['privateLocalPublicationApproved']
    assert all(d[s]['reason']=='matched' and d[s]['exact_match_count']==1 for s in ['local','remote'])
    for name,stage in [('receiverPreClose','pre_close'),('receiverFinal','cleanup')]:
        e=receiver[name];s=e['snapshot']
        assert e['stage']==stage and e['capture']=='live' and e['sampledAtMs']<=e['recordedAtMs'] and not e['errors']
        assert (s['framesReceived'],s['framesDecoded'],s['framesRendered'],s['videoWidth'],s['videoHeight'])==(240,240,240,1280,720)
        assert s['peerMatches'] and s['connectionPath']=='direct'
    assert all(receiver['receiverFinal']['cleanup'].values())
    control=receiver['receiverFinal']['snapshot']['control']
    assert (source['control']['accepted_moves'],source['control']['acks_sent'],control['sentMoves'],control['ackReceived'])==(100,100,100,100)
    assert not source['control']['rejected'] and control['pending']==control['anomalies']==0
    peak=max(len(x['relayPorts']) for x in snapshots)
    assert peak==2 and len({x['servicePid'] for x in snapshots})==1 and snapshots[0]['relayPorts']==snapshots[-1]['relayPorts']==[]
    assert len(verdict)==2 and all(v['classification']=='refresh_zero_immediate' and v['sameServicePid'] for v in verdict)
    assert max(v['firstSocketAbsentNs'] for v in verdict)<c['requestedAtNs']
    classification_record=read('classification-record.json')
    assert classification_record['classificationStoredTextSha256']==text_sha(OUT/'allocation-verdict-before-stop.json')
    classified_at=classification_record['classificationOriginalFileMtimeNs']
    assert classified_at<c['requestedAtNs']
    assert human['firstRoundSucceeded'] and human['otherMachineAndOtherCopiesDeleted'] and human['roundsExecuted']==1
    local_root=ROOT.parent
    remaining=list(local_root.rglob('r06-private.json'))
    assert not remaining
    assert not (ROOT/'artifacts/private/r06-online/RemoteHub-R06-v2-本轮限时配置.zip').exists()
    result=dict(version=1,recordedAt=datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=8))).isoformat(),
        scope='one real Windows two-PC 8-second synthetic diagnostic, not full streaming regression',
        roundsExecuted=1,secondRoundNotNeeded=True,syntheticVideoFrames=240,syntheticVideoNominalSeconds=8,
        senderReportedElapsedMs=source['elapsed_ms'],senderElapsedIncludesSetupAndCleanup=True,
        strictIceLocalReason=d['local']['reason'],strictIceRemoteReason=d['remote']['reason'],
        receiverPreClose='live',receiverFinal='live',receiverFinalCleanup=receiver['receiverFinal']['cleanup'],
        videoReceivedDecodedRendered=[240,240,240],backgroundControlSentReceivedAck=[100,100,100],
        observedConcurrentAllocationPeak=peak,turnPid=a['turnPid'],finalSocketSnapshotCount=len(snapshots),
        lastSocketSnapshotNs=snapshots[-1]['timeNs'],allocationRelease=[v['classification'] for v in verdict],
        allocationClassifiedAtFileMtimeNs=classified_at,classificationPrecededServiceStop=True,
        cloudWindowSeconds=(c['finishedAtNs']/1e9-a['authorizationStart']),guardSmokeBeforeStartup=True,
        natural19MinuteExpiryWithActiveMedia='NOT_RUN: manual early cleanup',leaseTimeoutCase='NOT_RUN: both Refresh(0) released',
        baselineUnchanged=baseline,cloudStoppedNoAutostart=True,temporaryCredentialsCleaned=True,
        otherMachineCleanupEvidence='explicit operator confirmation; not remotely inspected',
        historicalFailure='NOT_REPRODUCED',historicalRootCause='NOT_DETERMINED',R06='AWAITING_REVIEW',R06Closed=False,phase0='HOLD',phase1Started=False,
        evidenceHashFormat='SHA256 of UTF-8 LF text without BOM; original classification byte hash retained separately',
        evidenceSha256={p.name:text_sha(p) for p in OUT.iterdir() if p.suffix in ['.json','.jsonl','.txt'] and p.name not in ['verdict.json','privacy.json']})
    check_text(json.dumps(result));check_files(p for p in OUT.iterdir() if p.suffix in ['.json','.jsonl','.txt'])
    (OUT/'verdict.json').write_text(json.dumps(result,indent=2)+'\n',encoding='utf-8')
    print(json.dumps({k:result[k] for k in ['roundsExecuted','observedConcurrentAllocationPeak','classificationPrecededServiceStop','historicalFailure','cloudStoppedNoAutostart','temporaryCredentialsCleaned','phase0']}))


if __name__=='__main__':
    try:main()
    except Exception:raise SystemExit('R06_EVIDENCE_CHECK_FAILED_NO_PRIVATE_ECHO') from None
