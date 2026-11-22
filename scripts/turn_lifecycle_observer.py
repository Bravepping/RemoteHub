"""Explicit operator-only coturn observer; reads no media and exports no addresses/tickets."""
import argparse
import datetime
import hashlib
import json
from pathlib import Path
import socket
import struct
import subprocess
import time

from g6_turn_wire import parse, xor_address


def timestamp(ns):
    return datetime.datetime.fromtimestamp(ns / 1e9, datetime.timezone.utc).isoformat()


def read_capture(path):
    data = path.read_bytes() if path.exists() else b''
    if len(data) < 24:
        return []
    formats = {b'\xd4\xc3\xb2\xa1': ('<', 1000), b'\xa1\xb2\xc3\xd4': ('>', 1000),
               b'\x4d\x3c\xb2\xa1': ('<', 1), b'\xa1\xb2\x3c\x4d': ('>', 1)}
    endian, scale = formats[data[:4]]
    link = struct.unpack(endian + 'I', data[20:24])[0]
    assert link in (1, 113, 276), 'UNSUPPORTED_CAPTURE_LINK'
    result = []
    offset = 24
    while offset + 16 <= len(data):
        sec, sub, size, _ = struct.unpack(endian + 'IIII', data[offset:offset + 16])
        if offset + 16 + size > len(data):
            break
        frame = data[offset + 16:offset + 16 + size]
        offset += 16 + size
        start = {1: 14, 113: 16, 276: 20}[link]
        if len(frame) < start + 20 or frame[start] >> 4 != 4:
            continue
        ihl = (frame[start] & 15) * 4
        if frame[start + 9] != 17 or len(frame) < start + ihl + 8:
            continue
        udp = start + ihl
        src_port, dst_port, udp_len = struct.unpack('!HHH', frame[udp:udp + 6])
        if udp_len < 28 or len(frame) < udp + udp_len:
            continue
        payload = frame[udp + 8:udp + udp_len]
        try:
            kind, transaction, values = parse(payload)
        except ValueError:
            continue
        if kind not in (3, 0x103, 0x113, 4, 0x104, 0x114):
            continue
        src = socket.inet_ntoa(frame[start + 12:start + 16])
        dst = socket.inet_ntoa(frame[start + 16:start + 20])
        result.append((sec * 10**9 + sub * scale, kind, transaction, values,
                       (src, src_port), (dst, dst_port)))
    return result


def lifecycle_events(packets, labels, split_allocations=False):
    transactions = {}
    output = []
    seen = set()
    allocations = {}
    allocation_cases = {}
    for ns, kind, tid, values, src, dst in packets:
        direction = 'request' if kind in (3, 4) else 'response'
        ident = (kind, tid, src, dst)
        if ident in seen:
            continue
        seen.add(ident)
        key = (tid, src, dst) if direction == 'request' else (tid, dst, src)
        if direction == 'request':
            username = values.get(6, (b'', 0))[0].decode('ascii', errors='replace')
            case = labels.get(username)
            if case is None:
                continue
            if split_allocations:
                # Both WSS peers receive one session username. Keep their actual
                # client/server UDP tuples separate without exporting addresses.
                route = (case, src, dst)
                if route not in allocation_cases:
                    number = 1 + sum(k[0] == case for k in allocation_cases)
                    allocation_cases[route] = f'{case}.allocation-{number}'
                case = allocation_cases[route]
            transactions[key] = case
        else:
            case = transactions.get(key)
            if case is None:
                continue
        lifetime_bytes = values.get(0xD, (b'', 0))[0]
        lifetime = struct.unpack('!I', lifetime_bytes)[0] if len(lifetime_bytes) == 4 else None
        event = dict(timeNs=ns, utc=timestamp(ns), case=case, event='turn_wire',
                     kind=hex(kind), direction=direction, lifetime=lifetime,
                     transactionHash=hashlib.sha256(tid).hexdigest()[:16])
        if kind == 0x103 and 0x16 in values:
            _, port = xor_address(values[0x16][0])
            assert 49160 <= port <= 49259, 'RELAY_RANGE_CHANGED'
            allocations[case] = port
            event['relayPort'] = port
        output.append(event)
    return output, allocations


def relay_sockets(pid):
    p = subprocess.run(['ss', '-H', '-lunp'], capture_output=True, text=True, timeout=5, check=True)
    ports = []
    for line in p.stdout.splitlines():
        if f'pid={pid},' not in line:
            continue
        fields = line.split()
        try:
            port = int(fields[3].rsplit(':', 1)[1])
        except (ValueError, IndexError):
            continue
        if 49160 <= port <= 49259:
            ports.append(port)
    return sorted(set(ports))


def run(config_path, folder, seconds, max_allocations=2, split_allocations=False):
    config = json.loads(config_path.read_text())
    assert set(config) == {'labels', 'turn_pid'}
    assert 1 <= seconds <= 5400
    assert 1 <= max_allocations <= 4
    pid = config['turn_pid']
    assert int(subprocess.check_output(['systemctl', 'show', 'remotehub-turn-g6.service', '-p', 'MainPID', '--value'])) == pid
    output = folder / 'events.jsonl'
    assert not output.exists()
    capture = folder / 'control-private.pcap'
    kinds = ' or '.join(f'udp[8:2] = {x}' for x in ['0x0003', '0x0103', '0x0113', '0x0004', '0x0104', '0x0114'])
    error_file = (folder / 'capture-stderr-private.txt').open('wb')
    child = subprocess.Popen(['tcpdump', '-U', '-i', 'any', '-s', '1024', '-w', str(capture),
                              f'udp port 3478 and ({kinds})'], stdout=subprocess.DEVNULL, stderr=error_file)
    started = time.monotonic()
    written = set()
    try:
        with output.open('w', buffering=1) as out:
            out.write(json.dumps(dict(event='observer_start', timeNs=time.time_ns(), monotonicNs=time.monotonic_ns(), servicePid=pid)) + '\n')
            while time.monotonic() - started <= seconds and not (folder / 'stop-observer').exists():
                assert child.poll() is None, 'CAPTURE_EXITED'
                assert Path(f'/proc/{pid}').exists(), 'TURN_STOPPED_DURING_OBSERVATION'
                current = json.loads(config_path.read_text())
                assert current['turn_pid'] == pid, 'OBSERVER_SERVICE_CONTEXT_CHANGED'
                events, allocations = lifecycle_events(read_capture(capture), current['labels'], split_allocations)
                for e in events:
                    ident = (e['kind'], e['transactionHash'], e['case'])
                    if ident not in written:
                        out.write(json.dumps(e) + '\n')
                        written.add(ident)
                ports = relay_sockets(pid)
                out.write(json.dumps(dict(event='socket_snapshot', timeNs=time.time_ns(), monotonicNs=time.monotonic_ns(), servicePid=pid,
                                          relayPorts=ports, allocationPorts=allocations)) + '\n')
                out.flush()
                if len(ports) > max_allocations:
                    raise RuntimeError('ALLOCATION_BUDGET_EXCEEDED')
                if capture.exists() and capture.stat().st_size > 5 * 1024 * 1024:
                    raise RuntimeError('CAPTURE_BUDGET_EXCEEDED')
                time.sleep(.5)
            out.write(json.dumps(dict(event='observer_end', timeNs=time.time_ns())) + '\n')
    finally:
        child.terminate()
        try:
            child.wait(timeout=3)
        except subprocess.TimeoutExpired:
            child.kill()
        error_file.close()


if __name__ == '__main__':
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--private-config', type=Path, required=True)
    ap.add_argument('--private-dir', type=Path, required=True)
    ap.add_argument('--seconds', type=int, default=3600)
    ap.add_argument('--max-allocations',type=int,default=2)
    ap.add_argument('--split-shared-credentials',action='store_true')
    args = ap.parse_args()
    run(args.private_config, args.private_dir, args.seconds,args.max_allocations,args.split_shared_credentials)
