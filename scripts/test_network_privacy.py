"""Privacy regression fixtures are generated in memory; never use configured endpoints."""
import contextlib
import io
import ipaddress
import json
import pathlib
import secrets
import tempfile
import unittest
import shutil
import subprocess
import sys
from network_privacy import PrivacyViolation, addresses, check_files, check_python_source, check_text, findings


def generated_address(version=4, private=False):
    # Random nonfixture inputs, no captured/configured endpoint and no stored denylist.
    while True:
        address = (ipaddress.IPv4Address if version == 4 else ipaddress.IPv6Address)(secrets.randbits(32 if version == 4 else 128))
        if not (address.is_loopback or address.is_unspecified) and address.is_private == private:
            return str(address)


class PrivacyTests(unittest.TestCase):
    def test_documentation_and_local_fixtures_are_explicitly_allowed(self):
        check_text('example 192.0.2.7 [2001:db8::7] localhost 127.0.0.1 unspecified 0.0.0.0')

    def test_generated_public_and_private_addresses_rejected_without_echo(self):
        for version, private in [(4, False), (4, True), (6, False), (6, True)]:
            value = generated_address(version, private)
            with contextlib.redirect_stdout(io.StringIO()) as out, contextlib.redirect_stderr(io.StringIO()) as err:
                with self.assertRaises(PrivacyViolation) as caught:
                    check_text('endpoint=' + value)
            self.assertTrue(str(caught.exception) == 'NETWORK_PRIVACY_REJECTED', 'FIXED_PRIVACY_ERROR_REQUIRED')
            self.assertTrue(value not in repr(caught.exception), 'NO_EXCEPTION_INPUT_ECHO')
            self.assertTrue(out.getvalue() + err.getvalue() == '', 'NO_STDIO_ECHO')

    def test_ipv6_zone_and_bracketed_port_do_not_bypass(self):
        value = generated_address(6)
        for text in [f'[{value}]:8443', value + '%adapter']:
            with self.assertRaises(PrivacyViolation): check_text(text)

    def test_rejected_python_literals_include_escapes(self):
        value = generated_address()
        escaped = ''.join('\\x' + format(ord(c), '02x') for c in value)
        for source in ['deny = ' + repr(value), 'deny = "' + escaped + '"']:
            with self.assertRaises(PrivacyViolation) as caught: check_python_source(source)
            self.assertTrue(value not in str(caught.exception), 'NO_EXCEPTION_INPUT_ECHO')

    def test_parser_error_does_not_echo_source(self):
        with self.assertRaises(PrivacyViolation) as caught: check_python_source('unclosed = (')
        self.assertEqual(str(caught.exception), 'PRIVACY_SOURCE_PARSE_REJECTED')

    def test_payload_structure_rejected_without_specific_endpoints(self):
        for text in ['{"sdp" : "value"}', '{"sender_invitation":"value"}', 'a=candidate:fixture', '-----BEGIN PRIVATE KEY-----']:
            with self.assertRaises(PrivacyViolation) as caught: check_text(text)
            self.assertEqual(str(caught.exception), 'NETWORK_PRIVACY_REJECTED')

    def test_classification_reports_counts_only(self):
        value = generated_address()
        result = findings(value)
        self.assertEqual(result['networkLiteralCount'], 1)
        self.assertTrue(value not in json.dumps(result), 'COUNTS_ONLY')
        self.assertEqual(len(list(addresses(value))), 1)

    def test_file_error_and_sensitive_content_do_not_echo_path(self):
        value = generated_address()
        parent = pathlib.Path(__file__).resolve().parent.parent / 'artifacts/private'
        parent.mkdir(parents=True, exist_ok=True)
        folder = pathlib.Path(tempfile.mkdtemp(prefix='remotehub-privacy-', dir=parent)).resolve()
        try:
            path = folder / value
            with self.assertRaises(PrivacyViolation) as caught: check_files([path])
            self.assertTrue(value not in str(caught.exception), 'NO_PATH_ECHO')
            path.write_text(value, encoding='utf-8')
            with self.assertRaises(PrivacyViolation) as caught: check_files([path])
            self.assertTrue(value not in str(caught.exception), 'NO_INPUT_ECHO')
        finally:
            assert folder.parent == parent.resolve() and folder.name.startswith('remotehub-privacy-')
            shutil.rmtree(folder)

    def test_scanners_tests_and_audit_source_are_self_scanned(self):
        root = pathlib.Path(__file__).resolve().parent
        paths = [root / name for name in ['network_privacy.py', 'test_network_privacy.py', 'p007b-evidence.py',
                                          'p006c-diag-evidence.py', 'p007b-privacy-audit.py']]
        self.assertEqual(check_files(paths, source=True), len(paths))

    def test_cli_stderr_does_not_echo_sensitive_argument_or_bad_source(self):
        value = generated_address()
        root = pathlib.Path(__file__).resolve().parent
        for name in ['network_privacy.py', 'p007b-evidence.py']:
            p = subprocess.run([sys.executable, str(root / name), '--invalid-' + value], capture_output=True, text=True)
            self.assertNotEqual(p.returncode, 0)
            self.assertTrue(value not in p.stdout + p.stderr, 'NO_CLI_ARGUMENT_ECHO')
            self.assertNotIn('Traceback', p.stderr)

    def test_source_scan_process_returns_only_fixed_error(self):
        value = generated_address()
        root = pathlib.Path(__file__).resolve().parent
        parent = root.parent / 'artifacts/private'
        parent.mkdir(parents=True, exist_ok=True)
        with tempfile.NamedTemporaryFile(prefix='remotehub-privacy-', suffix='.py', dir=parent, delete=False) as f:
            path = pathlib.Path(f.name)
            f.write(('denylist = ' + repr([value])).encode())
        try:
            p = subprocess.run([sys.executable, str(root / 'network_privacy.py'), '--source', str(path)], capture_output=True, text=True)
            self.assertNotEqual(p.returncode, 0)
            self.assertTrue(value not in p.stdout + p.stderr, 'NO_CLI_SOURCE_ECHO')
            self.assertTrue(p.stderr.strip() == 'NETWORK_PRIVACY_CHECK_FAILED_NO_INPUT_ECHO', 'FIXED_CLI_ERROR_REQUIRED')
        finally:
            assert path.resolve().parent == parent.resolve()
            path.unlink()


if __name__ == '__main__':
    unittest.main()
