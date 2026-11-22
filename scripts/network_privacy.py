"""Generic address/payload privacy checks. Never embed a real endpoint denylist."""
import ast
import ipaddress
import re
import argparse
import json
import pathlib


class PrivacyViolation(ValueError):
    """Fixed error codes only: input, paths and endpoints must not reach diagnostics."""


class SafeArgumentParser(argparse.ArgumentParser):
    def error(self, message):
        raise PrivacyViolation('PRIVACY_ARGUMENT_REJECTED')


_V4 = re.compile(r'(?<![\w.])(?:[0-9]{1,3}\.){3}[0-9]{1,3}(?![\w.])')
_V6 = re.compile(r'(?<![\w:])(?:[0-9a-fA-F]{0,4}:){2,}[0-9a-fA-F:.]*(?:%[\w.-]+)?')
_DOCUMENTATION = tuple(map(ipaddress.ip_network, ('192.0.2.0/24', '198.51.100.0/24', '203.0.113.0/24', '2001:db8::/32')))
_PAYLOAD = re.compile(r'''["'](?:sdp|sender_invitation|receiver_invitation)["']\s*:|a=candidate:|BEGIN (?:[A-Z]+ )?PRIVATE KEY''', re.I)


def addresses(text):
    """Internal iterator. Callers must never emit its values or exception inputs."""
    seen = set()
    for pattern in (_V4, _V6):
        for match in pattern.finditer(text):
            candidate = match.group().split('%', 1)[0]
            try:
                address = ipaddress.ip_address(candidate)
            except ValueError:
                continue
            if address not in seen:
                seen.add(address)
                yield address


def findings(text):
    """Return classes/counts only. IPv4 versions that parse as addresses fail closed."""
    counts = {'networkLiteralCount': 0, 'ipv4Count': 0, 'ipv6Count': 0, 'sensitivePayloadCount': 0}
    for address in addresses(text):
        # Publicly defined fixtures only; no machine-specific/private endpoint exemptions.
        if address.is_loopback or address.is_unspecified or any(address in n for n in _DOCUMENTATION if n.version == address.version):
            continue
        counts['networkLiteralCount'] += 1
        counts['ipv4Count' if address.version == 4 else 'ipv6Count'] += 1
    counts['sensitivePayloadCount'] = len(_PAYLOAD.findall(text))
    return counts


def check_text(text, *, payloads=True):
    result = findings(text)
    if result['networkLiteralCount'] or (payloads and result['sensitivePayloadCount']):
        raise PrivacyViolation('NETWORK_PRIVACY_REJECTED')
    return result


def check_python_source(text):
    # Parse escaped string literals as well as the visible source. Do not echo SyntaxError.
    check_text(text, payloads=False)
    try:
        tree = ast.parse(text)
    except (SyntaxError, ValueError, RecursionError):
        raise PrivacyViolation('PRIVACY_SOURCE_PARSE_REJECTED') from None
    for node in ast.walk(tree):
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            check_text(node.value, payloads=False)


def check_files(paths, *, source=False):
    """Fixed errors on read/decode/policy failure. No offending path or data in errors."""
    count = 0
    try:
        for path in paths:
            text = path.read_text(encoding='utf-8-sig')
            (check_python_source if source else check_text)(text)
            count += 1
    except PrivacyViolation:
        raise
    except (OSError, UnicodeError, ValueError):
        raise PrivacyViolation('PRIVACY_FILE_READ_REJECTED') from None
    return count


if __name__ == '__main__':
    try:
        parser = SafeArgumentParser(description=__doc__)
        parser.add_argument('--source', action='store_true')
        parser.add_argument('files', nargs='+')
        args = parser.parse_args()
        count = check_files([pathlib.Path(p) for p in args.files], source=args.source)
        print(json.dumps({'filesChecked': count, 'passed': True}))
    except Exception:
        raise SystemExit('NETWORK_PRIVACY_CHECK_FAILED_NO_INPUT_ECHO') from None
