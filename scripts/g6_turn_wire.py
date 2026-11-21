"""Operator-only, bounded G6 STUN/TURN test. No capture, media or input injection.

The private input contains short-lived tickets, never the shared TURN secret.
Only --approved-target explicitly authorizes packets to its exact IPv4 endpoint.
RFC 5389 MESSAGE-INTEGRITY and RFC 5769 vectors are checked independently.
"""
import argparse
import hashlib
import hmac
import ipaddress
import json
import os
from pathlib import Path
import secrets
import socket
import stat
import struct
import time
import zlib

COOKIE = 0x2112A442


def attribute(kind, value):
    return struct.pack('!HH', kind, len(value)) + value + bytes((-len(value)) % 4)


def packet(kind, transaction, attributes, key=None):
    body = b''.join(attribute(k, v) for k, v in attributes)
    header = struct.pack('!HHI12s', kind, len(body) + (24 if key else 0), COOKIE, transaction)
    if key:
        body += attribute(0x0008, hmac.digest(key, header + body, 'sha1'))
    return header + body


def parse(data):
    if not 20 <= len(data) <= 4096:
        raise ValueError('STUN_SIZE')
    kind, length, cookie, transaction = struct.unpack('!HHI12s', data[:20])
    if kind & 0xC000 or cookie != COOKIE or length % 4 or length + 20 != len(data):
        raise ValueError('STUN_HEADER')
    values = {}
    offset = 20
    while offset < len(data):
        if offset + 4 > len(data) or len(values) >= 64:
            raise ValueError('STUN_ATTRIBUTES')
        code, size = struct.unpack('!HH', data[offset:offset + 4])
        end = offset + 4 + size
        if end > len(data) or code in values:
            raise ValueError('STUN_ATTRIBUTE_LENGTH_OR_DUPLICATE')
        values[code] = (data[offset + 4:end], offset)
        offset = end + (-size) % 4
    if offset != len(data):
        raise ValueError('STUN_PADDING')
    if 0x8028 in values:
        value, position = values[0x8028]
        if len(value) != 4 or struct.unpack('!I', value)[0] != (zlib.crc32(data[:position]) ^ 0x5354554E):
            raise ValueError('STUN_FINGERPRINT')
    return kind, transaction, values


def integrity(data, key):
    _, _, values = parse(data)
    if 0x0008 not in values:
        return False
    value, position = values[0x0008]
    prefix = data[:2] + struct.pack('!H', position - 20 + 24) + data[4:position]
    return hmac.compare_digest(value, hmac.digest(key, prefix, 'sha1'))


def error_code(values):
    value = values.get(0x0009, (b'', 0))[0]
    return (value[2] & 7) * 100 + value[3] if len(value) >= 4 else None


def credential_denied(label, kind, values, allocated):
    if kind != 0x0113 or allocated:
        return False
    if label == 'wrong_realm':
        # coturn 4.6.1 check_stun_auth uses 437 for an Allocate realm mismatch.
        # Check its specific reason as well; an unrelated allocation error is not a pass.
        return error_code(values) == 437 and b'the realm value is incorrect' in values.get(0x0009, (b'', 0))[0][4:]
    return error_code(values) == 401


def xor_address(value):
    if len(value) != 8 or value[:2] != b'\0\1':
        raise ValueError('STUN_IPV4_REQUIRED')
    port, address = struct.unpack('!HI', value[2:])
    return str(ipaddress.IPv4Address(address ^ COOKIE)), port ^ (COOKIE >> 16)


class Client:
    def __init__(self, target, local, username='', credential=''):
        self.target = target
        self.socket = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.socket.bind((local, 0))
        self.username = username
        self.credential = credential
        self.realm = ''
        self.nonce = b''
        self.allocated = False

    def key(self):
        # MD5 here is RFC 5389 long-term TURN compatibility, never login hashing.
        return hashlib.md5(f'{self.username}:{self.realm}:{self.credential}'.encode()).digest()

    def exchange(self, kind, attributes, authenticated=False):
        if authenticated:
            attributes = attributes + [(0x0006, self.username.encode()), (0x0014, self.realm.encode()), (0x0015, self.nonce)]
        transaction = secrets.token_bytes(12)
        data = packet(kind, transaction, attributes, self.key() if authenticated else None)
        started = time.monotonic()
        for timeout in (0.5, 1.0, 2.0):
            self.socket.sendto(data, self.target)
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                self.socket.settimeout(max(0.001, deadline - time.monotonic()))
                try:
                    response, source = self.socket.recvfrom(4097)
                except socket.timeout:
                    break
                if source != self.target or len(response) < 20 or response[8:20] != transaction:
                    continue
                response_type, _, values = parse(response)
                if response_type == 0x0103:
                    self.allocated = True
                return response, response_type, values, round((time.monotonic() - started) * 1000, 3)
        raise TimeoutError('AUTHORIZED_ENDPOINT_NO_RESPONSE')

    def challenge(self):
        response = self.exchange(3, [(0x0019, b'\x11\0\0\0')])
        if response[1] != 0x0113 or error_code(response[2]) != 401:
            raise ValueError('AUTH_CHALLENGE_REQUIRED')
        self.realm = response[2][0x0014][0].decode()
        self.nonce = response[2][0x0015][0]
        return response

    def allocate(self):
        return self.exchange(3, [(0x0019, b'\x11\0\0\0'), (0x000D, struct.pack('!I', 60))], True)

    def close(self):
        released = not self.allocated
        try:
            if self.allocated:
                data, kind, values, _ = self.exchange(4, [(0x000D, bytes(4))], True)
                released = kind == 0x0104 and integrity(data, self.key()) and values.get(0x000D, (b'', 0))[0] == bytes(4)
        finally:
            self.socket.close()
        return released


def run(config):
    target = (config['target'], 3478)
    address = ipaddress.IPv4Address(target[0])
    if not address.is_global or address.is_multicast or address.is_reserved:
        raise ValueError('PUBLIC_APPROVED_ENDPOINT_REQUIRED')
    if not any(ipaddress.IPv4Address(config['local']) in ipaddress.IPv4Network(n) for n in ('10.0.0.0/8', '172.16.0.0/12', '192.168.0.0/16')):
        raise ValueError('PHYSICAL_INTERFACE_REQUIRED')
    rows = []
    cleanups = []
    client = Client(target, config['local'])
    try:
        _, kind, values, elapsed = client.exchange(1, [])
        mapped = xor_address(values[0x0020][0])
        rows.append({'case': 'stun_binding', 'passed': kind == 0x0101 and mapped[1] > 0, 'responseType': hex(kind), 'requestElapsedMs': elapsed})
        _, kind, values, elapsed = client.challenge()
        rows.append({'case': 'anonymous_allocate', 'passed': kind == 0x0113 and error_code(values) == 401 and not client.allocated, 'serverErrorCode': error_code(values), 'requestElapsedMs': elapsed})
    finally:
        cleanups.append(client.close())
    for label in ('correct', 'wrong_password', 'expired', 'wrong_realm'):
        ticket = config['expired' if label == 'expired' else 'valid']
        client = Client(target, config['local'], ticket['username'], ticket['credential'])
        try:
            client.challenge()
            if label == 'wrong_password':
                client.credential = ('B' if client.credential[0] == 'A' else 'A') + client.credential[1:]
            if label == 'wrong_realm':
                client.realm += '.wrong'
            data, kind, values, elapsed = client.allocate()
            row = {'case': label, 'responseType': hex(kind), 'serverErrorCode': error_code(values), 'requestElapsedMs': elapsed}
            if label == 'correct':
                relay = xor_address(values[0x0016][0]) if 0x0016 in values else ('', 0)
                row['responseIntegrityVerified'] = integrity(data, client.key())
                row['relayMatchesApprovedRange'] = relay[0] == config['target'] and 49160 <= relay[1] <= 49259
                row['passed'] = kind == 0x0103 and row['responseIntegrityVerified'] and row['relayMatchesApprovedRange']
                if not row['passed']:
                    raise ValueError('POSITIVE_ALLOCATE_NOT_VERIFIED')
                for name, address in [('loopback', '127.0.0.1'), ('private', '10.0.0.1'), ('metadata_link_local', '169.254.169.254'), ('cgnat', '100.64.0.1'), ('multicast', '224.0.0.1')]:
                    value = b'\0\1' + struct.pack('!HI', 9 ^ (COOKIE >> 16), int(ipaddress.IPv4Address(address)) ^ COOKIE)
                    denial, denial_type, denial_values, denial_ms = client.exchange(8, [(0x0012, value)], True)
                    rows.append({'case': 'deny_peer_' + name, 'passed': denial_type == 0x0118 and error_code(denial_values) == 403 and integrity(denial, client.key()), 'serverErrorCode': error_code(denial_values), 'requestElapsedMs': denial_ms})
            else:
                row['passed'] = credential_denied(label, kind, values, client.allocated)
                if label == 'wrong_realm':
                    row['realmMismatchReasonVerified'] = row['passed']
            rows.append(row)
        finally:
            cleanups.append(client.close())
    return {'transport': 'actual UDP STUN/TURN', 'cases': rows, 'allocationsReleased': all(cleanups), 'selectedPair': None, 'iceRtt': None, 'packetsLost': None, 'sharedSecretPresent': False, 'passed': all(x['passed'] for x in rows) and all(cleanups)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--approved-target', type=Path, required=True)
    args = parser.parse_args()
    try:
        path = args.approved_target
        info = path.lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_size > 8192 or (os.name != 'nt' and info.st_mode & 0o077):
            raise ValueError('PRIVATE_INPUT_REQUIRED')
        result = run(json.loads(path.read_text(encoding='utf-8')))
        print(json.dumps(result, indent=2))
        return 0 if result['passed'] else 1
    except Exception as error:
        print(json.dumps({'passed': False, 'errorType': type(error).__name__, 'privateDetailsSuppressed': True}))
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
