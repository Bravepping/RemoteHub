"""Offline delivery/privacy negatives; no cloud, desktop or native window start."""
import hashlib
import json
import pathlib
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import importlib.util

ROOT = pathlib.Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('p007b_package', ROOT / 'scripts/package-p007b.py')
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


class DeliveryTests(unittest.TestCase):
    def setUp(self):
        self.parent = (ROOT / 'artifacts/private').resolve()
        self.parent.mkdir(parents=True, exist_ok=True)
        self.folder = pathlib.Path(tempfile.mkdtemp(prefix='p007b-delivery-test-', dir=self.parent)).resolve()
        self.owner = '11111111-2222-4333-8444-555555555555'
        self.reports = self.folder / 'artifacts/private/p007b-r06'
        self.reports.mkdir(parents=True)
        for source, name in [('Export-R06Summary.ps1', 'Export-R06Summary.ps1'), ('Verify-R06SHA256.ps1', 'Verify-SHA256.ps1')]:
            shutil.copyfile(ROOT / 'scripts' / source, self.folder / name)
        exe = self.folder / 'RemoteHub-R06.exe'; exe.write_bytes(b'fixture-not-executed')
        (self.folder / 'BUILD_INFO.json').write_text(json.dumps(dict(buildTag='p007b-r06-v1',
            activeCredentialsIncluded=False, desktopCapture=False, programSha256=hashlib.sha256(exe.read_bytes()).hexdigest())), encoding='utf-8')

    def tearDown(self):
        # Resolve and check the absolute temporary target before recursive cleanup.
        target = self.folder.resolve()
        assert target.parent == self.parent and target.name.startswith('p007b-delivery-test-')
        shutil.rmtree(target)

    def write(self, suffix, value):
        (self.reports / f'{self.owner}-{suffix}.json').write_text(json.dumps(value), encoding='utf-8')

    def export(self):
        p = subprocess.run(['powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File',
                            str(self.folder / 'Export-R06Summary.ps1')], capture_output=True, text=True, timeout=15)
        files = list(self.folder.glob('R06-summary-*.json'))
        return p, json.loads(files[0].read_text(encoding='utf-8-sig')) if files else None

    def native(self):
        return dict(buildTag='p007b-r06-v1', recordedAtMs=1000, native=dict(role='sender', resources_released=True,
            error=None, source=dict(source_kind='synthetic_nv12', frames_sent=45, resources_released=True,
                                    sensitive='secret 192.168.1.2', control=dict(accepted_moves=10, acks_sent=10))))

    def test_redaction_and_missing_final_stay_truthful(self):
        v = self.native(); v['native']['error'] = 'SECRET_TOKEN 192.168.1.2'
        self.write('final', v)
        p, result = self.export()
        self.assertEqual(p.returncode, 0, p.stderr)
        text = json.dumps(result)
        self.assertNotIn('SECRET_TOKEN', text); self.assertNotIn('192.168.1.2', text)
        self.assertIsNone(result['receiverFinal']); self.assertIsNone(result['receiverPreClose'])
        self.assertEqual(result['error'], 'REDACTED_UNKNOWN_ERROR')
        self.assertEqual(result['source']['frames_sent'], 45)

    def test_last_known_timestamp_and_unknown_cleanup_preserved(self):
        self.write('native-final', self.native())
        e = dict(version=1, stage='pre_close', capture='last_known', sampledAtMs=500, recordedAtMs=1000,
                 snapshot=dict(framesDecoded=19, peerMatches=False, private='secret'), errors=['P006_WSS_CLOSED'],
                 cleanup=dict(nativeResourcesReleased=True, peerClosed=False))
        self.write('receiver-pre-close', e)
        e = dict(e, stage='cleanup', recordedAtMs=1100, cleanup=dict(peerClosed=True, tracksStopped=True))
        self.write('receiver-final', e)
        p, r = self.export(); self.assertEqual(p.returncode, 0, p.stderr)
        self.assertEqual(r['receiverFinal']['sampledAtMs'], 500)
        self.assertFalse(r['receiverFinal']['snapshot']['peerMatches'])
        self.assertIsNone(r['receiverFinal']['cleanup']['nativeResourcesReleased'])
        self.assertNotIn('secret', json.dumps(r))

    def test_private_value_in_diagnostic_enum_refused(self):
        v = self.native(); v['native']['source']['direct_pair_diagnostic'] = dict(version=1,
            local=dict(reason='private 192.168.1.2'))
        self.write('final', v)
        p, result = self.export(); self.assertNotEqual(p.returncode, 0); self.assertIsNone(result)

    def test_allowed_pair_diagnosis_retains_side_and_reason_only(self):
        v = self.native()
        side = dict(reason='port_changed', candidate_type='host', protocol='udp', address_class='private',
                    approved_count=1, same_address_count=1, same_port_count=0, same_type_count=1,
                    same_endpoint_count=0, exact_match_count=0, address='private-secret')
        v['native']['source']['direct_pair_diagnostic'] = dict(version=1, approved=False,
            private_local_publication_approved=True, local_set_source='validated_published_answer',
            remote_set_source='authenticated_validated_offer', local=side, remote=side)
        self.write('final', v)
        p, r = self.export(); self.assertEqual(p.returncode, 0, p.stderr)
        d = r['source']['directPairDiagnostic']
        self.assertFalse(d['approved']); self.assertEqual(d['local']['reason'], 'port_changed')
        self.assertNotIn('private-secret', json.dumps(r))

    def test_numeric_string_cannot_smuggle_private_payload(self):
        v = self.native(); v['native']['source']['frames_sent'] = 'secret-address'
        self.write('final', v)
        p, result = self.export(); self.assertNotEqual(p.returncode, 0); self.assertIsNone(result)

    def test_old_or_desktop_report_refused(self):
        for field, value in [('buildTag', 'p006c-v1'), ('sourceKind', 'desktop')]:
            v = self.native()
            if field == 'buildTag': v[field] = value
            else: v['native']['source']['source_kind'] = value
            self.write('final', v)
            p, result = self.export(); self.assertNotEqual(p.returncode, 0); self.assertIsNone(result)

    def test_sha_check_detects_tampering(self):
        exe = self.folder / 'RemoteHub-R06.exe'; exe.write_bytes(b'not-executed-test-bytes')
        info = dict(buildTag='p007b-r06-v1', activeCredentialsIncluded=False, desktopCapture=False,
                    programSha256=hashlib.sha256(exe.read_bytes()).hexdigest())
        (self.folder / 'BUILD_INFO.json').write_text(json.dumps(info), encoding='utf-8')
        cmd = ['powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', str(self.folder / 'Verify-SHA256.ps1')]
        p = subprocess.run(cmd, capture_output=True, timeout=10); self.assertEqual(p.returncode, 0)
        exe.write_bytes(b'changed')
        p = subprocess.run(cmd, capture_output=True, timeout=10); self.assertNotEqual(p.returncode, 0)

    def test_packager_refuses_non_diagnostic_identity(self):
        # Mock process output, never execute an unknown binary.
        for info in [{}, dict(buildTag='p007b-r06-v1', desktopCapture=True), dict(sourceKind='desktop')]:
            with patch.object(package.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, json.dumps(info), '')):
                with self.assertRaises(ValueError): package.verify_identity(self.folder / 'not-executed.exe')

    def test_packager_refuses_existing_or_external_destination(self):
        for target in [self.folder, ROOT / 'artifacts/private/wrong-parent']:
            with self.assertRaises(ValueError): package.package(ROOT, target)


if __name__ == '__main__':
    unittest.main()
