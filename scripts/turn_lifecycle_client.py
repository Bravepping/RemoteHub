"""Bounded operator-only protocol lifecycle cases. No screen, media or input API."""
import argparse
import json
import ipaddress
from pathlib import Path
import struct
import time

from g6_turn_wire import Client, integrity, xor_address


def run(path, case):
    c = json.loads(path.read_text())
    assert set(c) == {'target', 'local', 'tickets', 'approval_expires_at'}
    assert time.time() < c['approval_expires_at'] <= time.time() + 3600
    address = ipaddress.IPv4Address(c['target'])
    assert address.is_global and not address.is_multicast and not address.is_reserved
    assert any(ipaddress.IPv4Address(c['local']) in ipaddress.IPv4Network(n) for n in ('10.0.0.0/8','172.16.0.0/12','192.168.0.0/16'))
    ticket = c['tickets'][case]
    client = Client((c['target'], 3478), c['local'], ticket['username'], ticket['credential'])
    def emit(event, **extra):
        print(json.dumps(dict(case=case, event=event, timeNs=time.time_ns(), monotonicNs=time.monotonic_ns(), **extra)), flush=True)
    emit('client_start')
    try:
        client.challenge()
        data, kind, values, elapsed = client.allocate()
        assert kind == 0x103 and integrity(data, client.key()), 'ALLOCATE_NOT_VERIFIED'
        lifetime = struct.unpack('!I', values[0xD][0])[0]
        ip, port = xor_address(values[0x16][0])
        assert ip == c['target'] and 49160 <= port <= 49259 and 1 <= lifetime <= 600
        emit('allocation_established', grantedLifetime=lifetime, relayPort=port, transactionElapsedMs=elapsed)
        time.sleep(2)
        if case == 'protocol-release':
            emit('refresh_zero_call')
            assert client.close(), 'REFRESH_ZERO_NOT_VERIFIED'
            emit('refresh_zero_confirmed')
        else:
            # Deliberately stop all refreshes; do not send zero-lifetime Refresh.
            client.socket.close()
            emit('socket_closed_without_refresh')
    except Exception:
        try:
            client.close()
        finally:
            emit('failed')
        raise


if __name__ == '__main__':
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--approved-target', required=True, type=Path)
    ap.add_argument('--case', required=True, choices=['protocol-release', 'protocol-expiry'])
    args = ap.parse_args()
    run(args.approved_target, args.case)
