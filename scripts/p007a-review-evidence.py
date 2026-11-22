"""Re-read committed Phase 0 evidence, without sockets, capture, or old-file writes."""
import argparse
import hashlib
import json
import pathlib
import subprocess
from collections import Counter

BASELINE = '246dcea74f120897b47c2b6f00a7980f266afb69'
ROOT = pathlib.Path(__file__).resolve().parent.parent
OUTPUT = ROOT / 'artifacts/p007a/evidence-index.json'


def collect():
    sources = {}
    rows = []

    def blob(path):
        data = subprocess.check_output(['git', 'show', f'{BASELINE}:{path}'], cwd=ROOT)
        sources[path] = hashlib.sha256(data).hexdigest()
        return data.decode('utf-8-sig')

    def read(path):
        return json.loads(blob(path))

    def row(key, scope, paths, observed, limits):
        for path in paths:
            blob(path)
        rows.append(dict(id=key, scope=scope, sources=paths, observed=observed, limitations=limits))

    # UI evidence is an archived human/tool observation, not a new GUI execution.
    row('P001', 'historical native Windows 10 Tauri/WebView2 observation',
        ['artifacts/native-window.txt', 'artifacts/environment.json'],
        {'source': 'archived native UI/IPC observation'}, 'No new GUI or Windows 11 test.')
    cap = read('artifacts/capture-report.json')
    assert cap['frames'] == 30 and cap['timeouts'] == 0
    row('P002', 'historical real DXGI desktop', ['artifacts/capture-report.json'],
        {k: cap[k] for k in ['frames', 'width', 'height', 'elapsed_ms', 'observed_fps']},
        'Desktop-update observation FPS; not encoded or rendered FPS.')
    enc = read('artifacts/p003/hardware-encode-report.json')
    dec = read('artifacts/p003/decoder-validation.json')
    assert enc['hardware'] and enc['encoded_frames'] == dec['decoded_frames'] == 300
    assert enc['missed_ticks'] == 0 and dec['status'] == 'PASS'
    row('P003', 'historical real desktop / MF hardware encode / independent decode',
        ['artifacts/p003/hardware-encode-report.json', 'artifacts/p003/decoder-validation.json',
         'artifacts/p003/software-encode-report.json'],
        {'encoder': enc['encoder'], 'frames': enc['encoded_frames'], 'missedTicks': enc['missed_ticks'],
         'inputToAuTiming': enc['encode_completion_time'], 'cpuMachinePercent': enc['cpu_machine_percent'],
         'peakWorkingSetBytes': enc['peak_working_set_bytes']},
        'Single hardware environment; CPU/encode metrics are not glass-to-glass latency.')
    loop = read('artifacts/p004a/native-session-summary.json')['sessions']
    assert [s['frames_decoded'] for s in loop[:6]] == [450] * 5 + [900]
    row('P004a', 'historical single-PC native WebView2 real desktop video',
        ['artifacts/p004a/native-session-summary.json', 'artifacts/p004a/run-08-window-close.json'],
        {'completeDecodedFrames': [s['frames_decoded'] for s in loop[:6]],
         'resourcesReleased': [s['resources_released'] for s in loop]},
        'Not physical two-PC; close-time JS counters absent, no recording.')

    def desktop(path):
        value = read(path)
        n = value['native']; s = n['source']; b = n['receiver_samples'][-1]
        assert n['authenticated'] and n['resources_released'] and s['resources_released']
        assert s['frames_sent'] == s['stream']['frames_encoded'] == s['stream']['frames_input']
        out = dict(sourceFrames=s['frames_sent'], lastReceived=b['framesReceived'],
                   lastDecoded=b['framesDecoded'], lastRendered=b['framesRendered'],
                   browserPair=[b['localCandidateType'], b['remoteCandidateType']],
                   peerMatches=b['peerMatches'], resourcesReleased=n['resources_released'])
        if 'control' in b:
            c = b['control']; a = s['control']
            assert c['sentMoves'] == c['ackReceived'] == a['accepted_moves'] == a['acks_sent'] == 100
            assert c['pending'] == c['anomalies'] == a['rejected'] == 0
            out['controlSentReceivedAckConfirmed'] = [c['sentMoves'], a['accepted_moves'], a['acks_sent'], c['ackReceived']]
            out['nativePair'] = s.get('native_path', s.get('relay_pair'))
        return out

    lan = desktop('artifacts/p004b/full-30s.json')
    assert lan['sourceFrames'] == lan['lastDecoded'] == lan['lastRendered'] == 900
    assert lan['browserPair'] == ['host', 'host'] and lan['peerMatches']
    row('P004b', 'historical physical two-PC native WebView2 wired LAN desktop',
        ['artifacts/p004b/full-30s.json', 'artifacts/p004b/cleanup-runs.json'], lan,
        'Original recording missing; receiver independent close report not collected.')
    controls = [desktop(f'artifacts/p005/{p}.json') for p in ['dual-pc-full', 'dual-pc-repeat']]
    row('P005', 'historical physical two-PC desktop + real control.v1 SCTP',
        ['artifacts/p005/dual-pc-full.json', 'artifacts/p005/dual-pc-repeat.json',
         'artifacts/p005/dual-pc-cleanup.json'], {'completeRounds': controls},
        'Receiver last periodic 893 is not receiver final 900; simulated inputs only.')
    row('G6', 'historical real cloud UDP STUN / TURN protocol and Windows gather',
        ['artifacts/p006a/g6/native-allocate-first.json', 'artifacts/p006a/g6/native-allocate.json',
         'artifacts/p006a/g6/wire-results.json', 'artifacts/p006a/g6/server-summary.json'],
        {'source': 'real protocol and gather records; 11-check conclusion in G6 report'},
        'Allocate/gather alone is not a media selected-pair or physical two-PC test.')
    relay = read('artifacts/p006b/two-pc/statistics.json')
    assert len(relay) == 5
    for r in relay:
        assert r['nativeAccepted'] == r['nativeAcks'] == r['browserSent'] == r['browserAcks'] == 100
        assert r['nativeSelectedPair']['local_type'] == r['nativeSelectedPair']['remote_type'] == 'relay'
        assert r['resourcesReleased'] and r['controlTaskJoined']
    assert all(r['sourceFrames'] == r['lastReceiverFrames'] == r['lastReceiverRendered'] == 900 for r in relay[:2])
    paths = ['artifacts/p006b/two-pc/statistics.json', 'artifacts/p006b/two-pc/user-observations.json']
    paths += [f'artifacts/p006b/two-pc/round-{i:02}-native.json' for i in range(1, 6)]
    row('P006b', 'historical physical two-PC broadband / cellular hotspot, forced TURN', paths,
        {'completeRounds': relay[:2], 'earlyStopSourceFrames': [r['sourceFrames'] for r in relay[2:]]},
        'Receiver-stop retest explicitly waived by user; independent early-stop log not fabricated. Old unassociated socket remains historical.')

    # Reclassify server control/socket observations; never contact that server.
    from turn_lifecycle_analyze import classify

    def allocations(prefix, event_file, wrapped=False):
        records = read(prefix + '/allocation-results.json' if wrapped else prefix + '/results.json')
        if wrapped and isinstance(records, dict):
            records = records['allocations']
        events = [json.loads(s) for s in blob(prefix + '/' + event_file).splitlines() if s.strip()]
        for a in records:
            assert classify(events, a['case']) == a
        return records

    life = allocations('artifacts/turn-lifecycle', 'observer-events.jsonl')
    assert Counter(a['classification'] for a in life) == {'refresh_zero_immediate': 5, 'lease_timeout': 1}
    row('TURN-LIFECYCLE', 'historical real cloud + Windows protocol/data-only PC tests',
        ['artifacts/turn-lifecycle/results.json', 'artifacts/turn-lifecycle/observer-events.jsonl',
         'artifacts/turn-lifecycle/shutdown.json'],
        {'classifications': dict(Counter(a['classification'] for a in life)),
         'grantedLeaseSeconds': sorted({a['grantedLifetimeSeconds'] for a in life})},
        'Same-PID single-allocation judgments precede shutdown; not a new media or two-PC test.')
    network_cases = []
    for name in ['direct', 'fallback', 'turn-unavailable', 'all-unavailable']:
        p = read(f'artifacts/p006c/cloud/{name}.json')
        assert p['expectedOutcomePassed'] and p['resources_released']
        if name == 'all-unavailable':
            assert p['selected_a'] is None and p['selected_b'] is None and p['received'] == 0
        else:
            assert p['sent'] == p['received'] == p['acknowledged'] == 100
        network_cases.append({k: p[k] for k in ['case', 'elapsed_ms', 'selected_a', 'selected_b',
                                               'sent', 'received', 'acknowledged', 'error']})
    row('P006c-network', 'historical one Windows / two native PCs / real cloud TURN / data-only',
        [f'artifacts/p006c/cloud/{n}.json' for n in ['direct', 'fallback', 'turn-unavailable', 'all-unavailable']],
        {'cases': network_cases},
        'Process-scoped packet faults; neither video nor physical two-PC. Expected timeout is not connection success.')
    cloud = []
    for case, expected in [('auto', 'direct'), ('fallback', 'relay')]:
        path = f'artifacts/p006c-video/cloud/{case}.json'
        v = read(path); s = v['native']['sender']['source']; b = v['browserFinal']['receiver']
        assert v['passed'] and s['source_kind'] == 'synthetic_nv12'
        assert s['native_path']['path'] == b['connectionPath'] == expected and b['peerMatches']
        assert s['frames_sent'] == b['framesReceived'] == b['framesDecoded'] == b['framesRendered'] == 300
        assert s['control']['accepted_moves'] == s['control']['acks_sent'] == b['control']['ackReceived'] == 100
        cloud.append({'case': case, 'nativePair': s['native_path'], 'decodedFrames': b['framesDecoded'],
                      'controlAck': b['control']['ackReceived'], 'startedAt': v['startedAt'], 'finishedAt': v['finishedAt']})
    ca = allocations('artifacts/p006c-video/cloud', 'events.jsonl', True)
    assert len(ca) == 9 and all(a['classification'] == 'refresh_zero_immediate' for a in ca)
    row('P006c-cloud-video', 'historical one physical Windows / real cloud WSS+TURN / generated video + Edge',
        ['artifacts/p006c-video/cloud/auto.json', 'artifacts/p006c-video/cloud/fallback.json',
         'artifacts/p006c-video/cloud/allocation-results.json', 'artifacts/p006c-video/cloud/events.jsonl',
         'artifacts/p006c-video/cloud/shutdown.json'], {'cases': cloud, 'allocations': len(ca)},
        'Direct blocking is process-scoped real packet loss, not physical NAT/ISP evidence; no desktop or embedded WebView2.')
    manual = [desktop(f'artifacts/p006c-manual/{p}.json') for p in ['lan/desktop-direct', 'hotspot/desktop-relay']]
    assert [r['lastDecoded'] for r in manual] == [893, 880]
    rejected = []
    for i in [1, 2]:
        n = read(f'artifacts/p006c-manual/development/lan-attempt-{i}.json')['native']
        assert n['error'] == 'DIRECT_SELECTED_PAIR_NOT_APPROVED' and n['source']['frames_sent'] == 0
        rejected.append({'error': n['error'], 'sourceFrames': n['source']['frames_sent'], 'resourcesReleased': n['resources_released']})
    ma = allocations('artifacts/p006c-manual/cloud', 'events.jsonl', True)
    assert len(ma) == 8 and all(a['classification'] == 'refresh_zero_immediate' for a in ma)
    row('P006c-physical', 'historical physical two-PC DXGI/MF/Tauri/WebView2, LAN and cellular',
        ['artifacts/p006c-manual/lan/desktop-direct.json', 'artifacts/p006c-manual/hotspot/desktop-relay.json',
         'artifacts/p006c-manual/development/lan-attempt-1.json', 'artifacts/p006c-manual/development/lan-attempt-2.json',
         'artifacts/p006c-manual/cloud/allocation-results.json', 'artifacts/p006c-manual/cloud/events.jsonl',
         'artifacts/p006c-manual/cloud/shutdown.json', 'artifacts/p006c-manual/local-cleanup.json'],
        {'successfulRounds': manual, 'earlierRejectedAttempts': rejected, 'allocations': len(ma),
         'receiverIndependentFinalCollected': False},
        'Two rejections remain unattributed; no independent receiver final and no inference of lost/missing final frames.')
    diag = read('artifacts/p006c-diag/summary.json')
    audit = read('artifacts/p006c-diag/historical-audit.json')
    assert not audit['rootCauseLocated']
    for c in diag['localCases']:
        assert c['passed'] and len(c['persisted']) == 2
        v = read(f'artifacts/p006c-diag/local/{c["scenario"]}.json')
        assert v['passed'] and v['receiverEvidence'][1]['cleanup']['peerClosed']
    row('P006c-DIAG', 'historical latest-code offline regression and one-PC local TLS generated video',
        ['artifacts/p006c-diag/summary.json', 'artifacts/p006c-diag/historical-audit.json',
         'artifacts/p006c-diag/automatic/results.json', 'artifacts/p006c-diag/development/wait-for-ack-timeout.txt'] +
        [f'artifacts/p006c-diag/local/{c["scenario"]}.json' for c in diag['localCases']],
        {'regressionCommands': diag['regressionCommands'], 'rustPassed': diag['rustPassed'],
         'frontendPassed': diag['frontendPassed'], 'localCases': diag['localCases'],
         'historicalRootCauseLocated': audit['rootCauseLocated']},
        'Not a new cloud/physical two-PC/embedded WebView2 close test; local 100-ACK development timeout also retained.')

    # Trace the original exit contract and each dated report; these are immutable history.
    docs = ['docs/RemoteHub_完整开发文档_v1.0.md', 'docs/PHASE0_TEST_REPORT.md', 'docs/PHASE0_P003_TEST_REPORT.md',
            'docs/PHASE0_P004_TEST_REPORT.md', 'docs/PHASE0_P004B_TEST_REPORT.md', 'docs/PHASE0_P005_TEST_REPORT.md',
            'docs/PHASE0_G6_TEST_REPORT.md', 'docs/PHASE0_P006B_TEST_REPORT.md',
            'docs/TURN_ALLOCATION_LIFECYCLE_TEST_REPORT.md', 'docs/PHASE0_P006C_TEST_REPORT.md',
            'docs/PHASE0_P006C_VIDEO_TEST_REPORT.md', 'docs/PHASE0_P006C_VIDEO_CLOUD_TEST_REPORT.md',
            'docs/PHASE0_P006C_MANUAL_TEST_REPORT.md', 'docs/PHASE0_P006C_DIAG_TEST_REPORT.md',
            'Cargo.toml', 'Cargo.lock', 'pnpm-lock.yaml', 'crates/hub-webrtc/Cargo.toml', 'THIRD_PARTY_NOTICES.md']
    for path in docs:
        blob(path)
    # Includes staged/unstaged tracked changes; permits only NEW review docs, not rewritten history.
    subprocess.run(['git', 'diff', '--exit-code', BASELINE, '--', *sources], cwd=ROOT, check=True, stdout=subprocess.PIPE)
    return dict(version=1, baselineCommit=BASELINE,
                scope='offline re-read and consistency checks of committed historical evidence; no new runtime/cloud/capture tests',
                historicalFilesUnchanged=True, observations=rows,
                reviewJudgments={'minimumTechnicalConditions': 'evidenced_on_tested_topologies',
                                 'formalPhase0Exit': 'hold_for_review', 'recordingExceptionApproved': False,
                                 'connectionReliabilityClosed': False, 'phase1Authorized': False},
                sourceHashFormat='SHA256 of exact Git blobs at baseline (portable across checkout line endings)',
                sources=dict(sorted(sources.items())))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true', help='Compare retained review index; do not write any file')
    args = parser.parse_args()
    result = collect()
    if args.check:
        assert json.loads(OUTPUT.read_text(encoding='utf-8')) == result, 'REVIEW_INDEX_MISMATCH'
    else:
        OUTPUT.parent.mkdir(parents=True, exist_ok=True)
        OUTPUT.write_text(json.dumps(result, indent=2, ensure_ascii=False) + '\n', encoding='utf-8')
    print(json.dumps({'mode': 'check' if args.check else 'generate', 'evidenceGroups': len(result['observations']),
                      'sourceFiles': len(result['sources']), 'historicalFilesUnchanged': True,
                      'newRuntimeTests': False, 'formalPhase0Exit': 'hold_for_review'}))
