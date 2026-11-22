"""Offline guard/manifest checks. All service actions use an in-memory fake runner."""
import ast
import importlib.util
import json
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest
from network_privacy import check_files

ROOT = pathlib.Path(__file__).resolve().parent.parent
OPS = ROOT / 'deploy/rendezvous-probe/r06-ops'


def load(name):
    spec = importlib.util.spec_from_file_location(name, OPS / (name + '.py'))
    module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
    return module


guard = load('expiry_cleanup'); manifest = load('execution_plan')
sys.modules['expiry_cleanup'] = guard
observer = load('r06_observer')


class PreflightTests(unittest.TestCase):
    def test_read_only_collector_has_no_mutating_calls_or_service_commands(self):
        tree = ast.parse((OPS / 'read_only_preflight.py').read_text())
        for node in ast.walk(tree):
            if not isinstance(node, ast.Call): continue
            if isinstance(node.func, ast.Attribute):
                self.assertNotIn(node.func.attr, ['write_text', 'write_bytes', 'mkdir', 'unlink', 'chmod', 'chown', 'Popen'])
            if isinstance(node.func, ast.Name) and node.func.id == 'run':
                head = [x.value for x in node.args[0].elts[:2] if isinstance(x, ast.Constant)]
                self.assertIn(head, [['systemctl','show'],['ss','-H'],['systemctl','list-units'],
                                     ['docker','inspect'],['iptables','-S'],['nft','list']])

    def test_plan_is_bounded_and_does_not_run_anything(self):
        p = manifest.plan('20261122T000000Z', 1000, 1000)
        self.assertEqual((p['authorizationDeadline'],p['guardTarget'],p['lastNewAllocationAdmission']), (2200,2140,1480))
        self.assertEqual((p['maxConcurrentAllocations'],p['maxConcurrentSessions'],p['maxRounds'],p['syntheticSeconds']), (4,1,2,8))
        self.assertTrue(p['commandsAreNotExecuted']); self.assertTrue(p['regenerateRemainingBeforeEachStart'])
        self.assertEqual(p['expiryTimerCommand'][0], 'systemd-run')
        self.assertIn('--on-active=1140s', p['expiryTimerCommand'])
        p2 = manifest.plan('20261122T000000Z', 1100, 1000)
        self.assertEqual(p2['authorizationDeadline'], 2200)
        self.assertIn('RuntimeMaxSec=1040s', p2['startWssCommand'])

    def test_invalid_scope_or_expired_manifest_refuses(self):
        for run_id, now, start in [('bad/../scope',1000,1000), ('20261122T000000Z',2200,1000), ('20261122T000000Z',999,1000)]:
            with self.assertRaises(ValueError): manifest.plan(run_id,now,start)
        for value in ['/', '/etc', '/run/../etc', '/run/remotehub-p007b-r06-not-a-timestamp']:
            with self.assertRaises(ValueError): guard.checked_directory(value)

    def run_cleanup(self, stuck=False, query_fails=False, missing=False):
        parent = ROOT / 'artifacts/private'; parent.mkdir(exist_ok=True)
        folder = pathlib.Path(tempfile.mkdtemp(prefix='r06-guard-test-',dir=parent)).resolve()
        calls = []; tick = [0]
        def call(args):
            calls.append(args)
            self.assertIn(args[2] if args[1]=='show' else args[-1], guard.SERVICES)
            if args[1] == 'show':
                if query_fails: raise RuntimeError('FIXTURE_COMMAND_FAILURE')
                return subprocess.CompletedProcess(args,0,'MainPID='+('42' if stuck else '0')+'\nActiveState='+('active' if stuck else 'inactive')+'\n','')
            return subprocess.CompletedProcess(args,5 if missing else 0,'','')
        def sleep(seconds): tick[0] += seconds
        try:
            (folder/'server.json').write_text('private fixture')
            (folder/'control-private.pcap').write_bytes(b'private fixture')
            (folder/'unrelated.txt').write_text('keep')
            value = guard.cleanup(folder,reason='deadline',call=call,clock=lambda:tick[0],sleep=sleep)
            self.assertTrue((folder/'unrelated.txt').exists())
            self.assertFalse((folder/'server.json').exists()); self.assertFalse((folder/'control-private.pcap').exists())
            return value,calls
        finally:
            self.assertEqual(folder.parent,parent.resolve())
            self.assertTrue(folder.name.startswith('r06-guard-test-'))
            shutil.rmtree(folder)

    def test_cleanup_stops_only_dedicated_units_before_any_release_verdict(self):
        value,calls = self.run_cleanup()
        self.assertEqual([c[1] for c in calls[:3]], ['stop']*3)
        self.assertTrue(all(value['dedicatedServicesStopped'].values()))
        self.assertEqual(value['allocationReleaseVerdict'],'not_inferred_by_cleanup')
        self.assertTrue(value['temporarySecretFilesAbsent']); self.assertFalse(value['forceKillRequested'])

    def test_stuck_unit_kill_does_not_masquerade_as_release_or_success(self):
        value,calls = self.run_cleanup(stuck=True)
        self.assertTrue(value['forceKillRequested'])
        self.assertFalse(any(value['dedicatedServicesStopped'].values()))
        self.assertTrue(any(c[1]=='kill' for c in calls))
        self.assertEqual(value['allocationReleaseVerdict'],'not_inferred_by_cleanup')

    def test_already_collected_transient_units_are_idempotent(self):
        value,calls=self.run_cleanup(missing=True)
        self.assertTrue(all(value['dedicatedServicesStopped'].values()))
        self.assertEqual(value['failures'],[])

    def test_query_failure_still_attempts_stops_and_secret_cleanup(self):
        value,calls = self.run_cleanup(query_fails=True)
        self.assertEqual([c[1] for c in calls[:3]],['stop']*3)
        self.assertTrue(value['temporarySecretFilesAbsent'])
        self.assertTrue(value['failures'])
        self.assertTrue(any(c[1]=='kill' for c in calls))
        self.assertNotIn('FIXTURE_COMMAND_FAILURE',json.dumps(value))

    def test_sources_and_sanitized_readonly_evidence_pass_privacy_scan(self):
        self.assertEqual(check_files(OPS.glob('*.py'),source=True),5)
        check_files([ROOT/'artifacts/r06-preflight/server-readonly.json'])

    def test_observer_rejects_extended_malformed_or_expired_windows(self):
        p = manifest.plan('20261122T000000Z',1000,1000)
        self.assertEqual(observer.remaining_window(p,1100),1040)
        for key, value in [('guardTarget',2141),('authorizationDeadline',2201),('authorizationStart',True),('lastNewAllocationAdmission',1481)]:
            with self.assertRaises(ValueError): observer.remaining_window({**p,key:value},1100)
        with self.assertRaises(ValueError): observer.remaining_window(p,2140)

    def test_observer_invalid_scope_cannot_stop_any_unit(self):
        calls=[]
        def invalid(value): raise ValueError('UNTRUSTED_SCOPE_FIXTURE')
        for args in [[],['--execute-after-authorization','/etc']]:
            with self.assertRaises(SystemExit) as caught:
                observer.main(args,root_allowed=True,validate=invalid,stop_call=lambda *a,**k:calls.append(a))
            self.assertEqual(str(caught.exception),'R06_OBSERVER_FAILED_NO_PRIVATE_ECHO')
        self.assertEqual(calls,[])

    def test_observer_validated_failure_stops_only_dedicated_media_units(self):
        calls=[]
        def fail(folder): raise ValueError('PRIVATE_ERROR_FIXTURE')
        with self.assertRaises(SystemExit):
            observer.main(['--execute-after-authorization','scoped-fixture'],root_allowed=True,
                          validate=lambda p:p,observe_run=fail,stop_call=lambda args,**k:calls.append(args))
        self.assertEqual([x[-1] for x in calls],guard.SERVICES[:2])

    def test_no_argument_scripts_are_fail_closed_without_actions(self):
        for name in ['expiry_cleanup','execution_plan','read_only_preflight','r06_observer']:
            p = subprocess.run([sys.executable,str(OPS/(name+'.py'))],capture_output=True,text=True)
            self.assertNotEqual(p.returncode,0)
            self.assertNotIn('Traceback',p.stderr)
            self.assertIn('R06_',p.stderr)


if __name__=='__main__': unittest.main()
