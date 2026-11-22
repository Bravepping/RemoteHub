"""Verify retained manual two-PC evidence offline. Never opens a session or captures a desktop."""
import json
from pathlib import Path
from turn_lifecycle_analyze import classify,read_jsonl


def verify(root):
    folder=root/'artifacts/p006c-manual';results=[]
    for file,path in [('lan/desktop-direct.json','direct'),('hotspot/desktop-relay.json','relay')]:
        record=json.loads((folder/file).read_text(encoding='utf-8'));native=record['native'];source=native['source']
        receiver=native['receiver_samples'][-1];control=receiver['control'];accepted=source['control']
        assert native['role']=='sender' and native['authenticated'] and native['resources_released'] and not native['error']
        assert source['source_kind']=='desktop' and source['ice_transport_policy']=='all' and source['resources_released'] and not source['error']
        assert source['stream']['frames_input']==source['stream']['frames_encoded']==source['frames_sent']==900
        assert source['stream']['missed_ticks']==0
        assert source['native_path']['path']==receiver['connectionPath']==path and receiver['peerMatches'] is True
        if path=='relay':
            assert source['native_path']['local_type']==source['native_path']['remote_type']==receiver['localCandidateType']==receiver['remoteCandidateType']=='relay'
        assert receiver['videoWidth']==1280 and receiver['videoHeight']==720 and 0<receiver['framesDecoded']<=900
        assert control['sentMoves']==control['ackReceived']==accepted['accepted_moves']==accepted['acks_sent']==100
        assert control['pending']==control['anomalies']==control['errorEvents']==accepted['rejected']==accepted['error_events']==0
        assert accepted['last_rx_seq']==accepted['last_tx_seq']==control['lastRxSeq']==control['lastTxSeq']==101
        assert accepted['pings_received']==accepted['pongs_sent']==control['pingsSent']==control['pongsReceived']==1
        assert accepted['task_joined'] and accepted['state']=='closed'
        results.append(dict(path=path,source='desktop',senderFrames=900,
            nativeSelectedPair=source['native_path'],browserPeriodicPair=[receiver['localCandidateType'],receiver['remoteCandidateType']],
            receiverLastPeriodicFrames={k:receiver[k] for k in ['framesReceived','framesDecoded','framesRendered']},
            receiverLastPeriodicPacketsLost=receiver['packetsLost'],receiverLastPeriodicFreezeCount=receiver['freezeCount'],
            simulatedEventsSentReceivedAckConfirmed=100,resourcesReleased=True,
            receiverIndependentFinalCollected=False,receiverFinalFrameCount=None))
    events=read_jsonl(folder/'cloud/events.jsonl');allocations=json.loads((folder/'cloud/allocation-results.json').read_text())
    assert len(allocations)==8
    assert all(classify(events,a['case'])==a and a['classification']=='refresh_zero_immediate' for a in allocations)
    shutdown=json.loads((folder/'cloud/shutdown.json').read_text());assert not shutdown['relaySocketsBeforeStop'] and not shutdown['remainingDedicatedPortListeners']
    for k in ['coturnInactiveDisabled','wssInactive','noPersistentWssUnit','oldWssInactiveDisabled','expiryTimerCancelled',
              'protectedHashesUnchanged','firewallPoliciesUnchanged','businessContainerStartTimesUnchanged',
              'temporaryServerCredentialsAndRawCaptureDeleted','allAllocationsReleaseConfirmedBeforeServiceStop']:
        assert shutdown[k],k
    assert shutdown['observerExit']=='0'
    assert not shutdown['additionalRunningUnits'] and not shutdown['missingBaselineUnits']
    snapshots=[e for e in events if e['event']=='socket_snapshot']
    assert len({e['servicePid'] for e in snapshots})==1
    maximum=max(len(e['relayPorts']) for e in snapshots);assert maximum<=4
    summary=dict(scope='two physical Windows PCs, operator-approved DXGI/MF/Tauri/WebView2 video and control.v1',
        successfulRounds=results,earlierRejectedAttempts=2,
        earlierFailure='DIRECT_SELECTED_PAIR_NOT_APPROVED; zero source frames; exact endpoint guard stayed closed; cause not yet attributed',
        cloudAllocationCount=8,cloudAllocationClassification='refresh_zero_immediate',maximumConcurrentAllocations=maximum,
        releaseObservationDelaySeconds=[min(a['socketAbsenceAfterResponseSeconds'] for a in allocations),max(a['socketAbsenceAfterResponseSeconds'] for a in allocations)],
        cloudServicesStopped=True,receiverIndependentFinalCollected=False,
        limitations='Source final/periodic authenticated receiver metrics plus human observation; no receiver final 900 claim, NAT compatibility and early rejected attempts remain limited evidence.',
        phase='P0-06c; stop for review, no input injection or Phase1')
    (folder/'summary.json').write_text(json.dumps(summary,indent=2)+'\n',encoding='utf-8')
    print(json.dumps(summary,indent=2))


if __name__=='__main__':verify(Path(__file__).resolve().parent.parent)
