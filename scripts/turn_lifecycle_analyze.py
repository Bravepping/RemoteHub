"""Classify per-allocation evidence; service shutdown is never a release success."""
import argparse
import json
from pathlib import Path


def read_jsonl(path):
    data = path.read_bytes()
    text = data.decode('utf-16') if data.startswith((b'\xff\xfe', b'\xfe\xff')) else data.decode('utf-8-sig')
    return [json.loads(line) for line in text.splitlines() if line.strip()]


def classify(events, case, grace_seconds=15):
    wire = [e for e in events if e.get('case') == case and e.get('event') == 'turn_wire']
    allocations = [e for e in wire if e['kind'] == '0x103' and e.get('lifetime', 0) > 0]
    if len(allocations) != 1:
        return dict(case=case, classification='inconclusive', reason='EXPECTED_ONE_SUCCESSFUL_ALLOCATION')
    allocated = allocations[0]
    port = allocated['relayPort']
    granted_ns = allocated['timeNs']
    all_snapshots = [e for e in events if e.get('event') == 'socket_snapshot']
    snapshots = [e for e in all_snapshots if e['timeNs'] >= granted_ns]
    pid_set = {e['servicePid'] for e in snapshots}
    if len(pid_set) != 1:
        return dict(case=case, classification='inconclusive', reason='SERVICE_PID_CHANGED_OR_NO_SNAPSHOTS')
    if not any(port in e['relayPorts'] for e in snapshots):
        return dict(case=case, classification='inconclusive', reason='ALLOCATION_SOCKET_NOT_OBSERVED')
    positive = [e for e in wire if e['kind'] in ('0x103', '0x104') and e.get('lifetime', 0) > 0]
    last = max(positive, key=lambda x: x['timeNs'])
    deadline_ns = last['timeNs'] + last['lifetime'] * 10**9
    zero_requests = {e['transactionHash'] for e in wire if e['kind'] == '0x4' and e.get('lifetime') == 0}
    zero_confirmations = [e for e in wire if e['kind'] == '0x104' and e.get('lifetime') == 0 and e['transactionHash'] in zero_requests]
    first_present = min(e['timeNs'] for e in snapshots if port in e['relayPorts'])
    absence = next((e for e in snapshots if e['timeNs'] >= first_present and port not in e['relayPorts']), None)
    base = dict(case=case, allocationTimeNs=granted_ns, grantedLifetimeSeconds=allocated['lifetime'],
                latestLeaseDeadlineNs=deadline_ns, relayPort=port, sameServicePid=True,
                firstSocketPresentNs=first_present, firstSocketAbsentNs=absence['timeNs'] if absence else None,
                refreshZeroRequestSeen=bool(zero_requests), refreshZeroSuccessSeen=bool(zero_confirmations),
                lastObservationNs=snapshots[-1]['timeNs'] if snapshots else None)
    baseline = next((e for e in reversed(all_snapshots) if e['timeNs'] < granted_ns), None)
    nominal = next((e for e in snapshots if e['timeNs'] >= deadline_ns), None)
    grace = next((e for e in snapshots if e['timeNs'] >= deadline_ns + grace_seconds * 10**9), None)
    base['checkpoints'] = {
        'baselineTimeNs': baseline['timeNs'] if baseline else None,
        'baselineSocketAbsent': (port not in baseline['relayPorts']) if baseline else None,
        'nominalLeaseExpiryTimeNs': nominal['timeNs'] if nominal else None,
        'socketAbsentAtNominalLeaseExpiry': (port not in nominal['relayPorts']) if nominal else None,
        'afterGraceTimeNs': grace['timeNs'] if grace else None,
        'socketAbsentAfterGrace': (port not in grace['relayPorts']) if grace else None,
    }
    if zero_confirmations and absence:
        zero = zero_confirmations[0]
        delay = (absence['timeNs'] - zero['timeNs']) / 10**9
        if -1 <= delay <= 3:
            return dict(base, classification='refresh_zero_immediate', zeroResponseNs=zero['timeNs'], socketAbsenceAfterResponseSeconds=delay)
        return dict(base, classification='inconclusive', reason='ZERO_ACK_AND_SOCKET_TIMING_DISAGREE')
    if absence:
        delay = (absence['timeNs'] - deadline_ns) / 10**9
        if -1 <= delay <= grace_seconds and not zero_confirmations:
            return dict(base, classification='lease_timeout', absenceRelativeToDeadlineSeconds=delay)
        return dict(base, classification='inconclusive', reason='EARLY_OR_UNCORRELATED_SOCKET_DISAPPEARANCE')
    if snapshots and snapshots[-1]['timeNs'] > deadline_ns + grace_seconds * 10**9:
        return dict(base, classification='abnormal_residual', reason='SOCKET_EXISTS_AFTER_GRANTED_LEASE_PLUS_GRACE')
    return dict(base, classification='pending', reason='WAITING_FOR_ACTUAL_GRANTED_LEASE')


def analyze(path, output):
    events = read_jsonl(path)
    cases = sorted({e['case'] for e in events if 'case' in e})
    result = [classify(events, case) for case in cases]
    output.write_text(json.dumps(result, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(result, indent=2))
    return result


if __name__ == '__main__':
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--events', required=True, type=Path)
    ap.add_argument('--out', required=True, type=Path)
    a = ap.parse_args()
    analyze(a.events, a.out)
