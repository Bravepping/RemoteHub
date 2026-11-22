"""Offline verification of retained real cloud evidence; performs no network or capture."""
import hashlib
import json
from pathlib import Path
import subprocess
from turn_lifecycle_analyze import classify,read_jsonl


def verify(repo):
    folder=repo/'artifacts/p006c-video/cloud'
    cases=[]
    for scenario,path in [('auto','direct'),('fallback','relay')]:
        value=json.loads((folder/f'{scenario}.json').read_text(encoding='utf-8'))
        source=value['native']['sender']['source'];browser=value['browserFinal']['receiver']
        control=value['native']['receiver']['browser_control']
        assert value['passed'] and value['exitCode']==0
        assert source['ice_transport_policy']=='all' and source['source_kind']=='synthetic_nv12'
        assert source['frames_sent']==source['stream']['frames_input']==source['stream']['frames_encoded']==300
        assert browser['framesReceived']==browser['framesDecoded']==browser['framesRendered']==300
        assert (browser['videoWidth'],browser['videoHeight'])==(1280,720)
        assert source['native_path']['path']==browser['connectionPath']==path and browser['peerMatches'] is True
        assert (source['native_path']['local_type'],source['native_path']['remote_type'])==(browser['remoteCandidateType'],browser['localCandidateType'])
        assert source['control']['accepted_moves']==source['control']['acks_sent']==control['sentMoves']==control['ackReceived']==100
        assert control['pending']==control['anomalies']==control['errorEvents']==0
        assert control['lastTxSeq']==control['lastRxSeq']==101 and control['pingsSent']==control['pongsReceived']==1
        assert source['resources_released'] and source['control']['task_joined'] and value['native']['receiver']['resources_released']
        assert not value['native']['sender']['error'] and not value['native']['receiver']['error'] and not value['browserFinal']['error']
        if scenario=='fallback':assert source['diagnostic_transport']['drop_direct'] and source['diagnostic_transport']['dropped_packets']>0
        cases.append(dict(scenario=scenario,startedAt=value['startedAt'],finishedAt=value['finishedAt'],path=path,
                          nativePair=source['native_path'],browserPair=[browser['localCandidateType'],browser['remoteCandidateType']],
                          frames=300,controlEvents=100,pending=0,anomalies=0,packetsLost=browser['packetsLost'],freezes=browser['freezeCount'],
                          browserFirstFrameMs=browser['firstFrameMs'],nativeConnectedMs=source['first_connected_ms'],
                          faultDroppedPackets=(source.get('diagnostic_transport') or {}).get('dropped_packets',0)))
    shutdown=json.loads((folder/'shutdown.json').read_text())
    assert shutdown['judgedBeforeServiceStop'] and shutdown['originalLeaseAndGraceAllObserved']
    assert shutdown['finalRelaySocketsBeforeServiceStop']==0 and not shutdown['remainingDedicatedPortListeners']
    for key in ['coturnInactiveDisabled','oldWssInactiveDisabled','transientWssInactive','transientWssHasNoPersistentUnit',
                'expiryTimerCancelled','protectedConfigsUnitsAndOldBinaryUnchanged','firewallPoliciesUnchanged',
                'businessContainersStartTimesUnchanged','temporaryPrivateKeysInvitationsLabelsAndCaptureDeleted']:
        assert shutdown[key],key
    assert not shutdown['realDesktopCaptureStarted'] and not shutdown['physicalTwoPcTestPerformed']
    allocations=json.loads((folder/'allocation-results.json').read_text())['allocations']
    assert len(allocations)==shutdown['allocationCount']
    assert all(a['classification']=='refresh_zero_immediate' and a['checkpoints']['socketAbsentAfterGrace'] is True for a in allocations)
    events=read_jsonl(folder/'events.jsonl')
    assert all(classify(events,a['case'])==a for a in allocations)
    assert {e['servicePid'] for e in events if e['event']=='socket_snapshot'}=={shutdown['turnPid']}
    names=subprocess.check_output(['git','ls-files','--cached','--others','--exclude-standard','-z','--','apps','crates','scripts'],cwd=repo).decode().split('\0')
    digest=hashlib.sha256()
    for name in sorted(filter(None,names)):
        digest.update(name.encode()+b'\0'+hashlib.sha256((repo/name).read_bytes()).digest())
    binaries={name:hashlib.sha256((repo/'target/probe'/name).read_bytes()).hexdigest() for name in ['remotehub-desktop.exe','authenticated-session-probe.exe']}
    summary=dict(scope='one physical Windows, real cloud WSS/TURN, real MF/RTP/SCTP/Edge, generated pixels; physical two-PC desktop acceptance pending',
                 passed=True,cases=cases,allocationCount=len(allocations),maxConcurrentAllocations=shutdown['maxConcurrentAllocations'],
                 allocationReleaseDelaySeconds=[min(a['socketAbsenceAfterResponseSeconds'] for a in allocations),max(a['socketAbsenceAfterResponseSeconds'] for a in allocations)],
                 grantedLifetimeSeconds=sorted({a['grantedLifetimeSeconds'] for a in allocations}),
                 sourceDigest=digest.hexdigest(),sourceDigestAlgorithm='same sorted apps/crates/scripts path+NUL+file SHA256 digest as local evidence',binaries=binaries)
    (folder/'summary.json').write_text(json.dumps(summary,indent=2)+'\n',encoding='utf-8')
    print(json.dumps(summary,indent=2))


if __name__=='__main__':verify(Path(__file__).resolve().parent.parent)
