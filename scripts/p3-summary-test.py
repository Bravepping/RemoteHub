"""Synthetic-only privacy regressions for the Phase3 public summary boundary."""
import importlib.util,json,unittest,subprocess,sys
from pathlib import Path
spec=importlib.util.spec_from_file_location('summary',Path(__file__).with_name('p3-summary.py'))
module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)

class SummaryTest(unittest.TestCase):
    def test_cli_rejection_never_echoes_unknown_argument(self):
        secret='PRIVATE_ARGUMENT_MUST_NOT_ESCAPE'
        r=subprocess.run([sys.executable,str(Path(__file__).with_name('p3-summary.py')),'--unknown',secret],capture_output=True)
        self.assertNotEqual(r.returncode,0)
        self.assertNotIn(secret.encode(),r.stdout+r.stderr)
        self.assertIn(b'PHASE3_SUMMARY_FAILED_NO_PRIVATE_ECHO',r.stderr)
    def test_unknown_strings_and_extra_fields_never_escape(self):
        secret='PRIVATE_TOKEN_MUST_NOT_ESCAPE'
        raw={'buildTag':secret,'browserCandidateSummary':{'before':1,'mdns':1,'published':1,'mdnsPublished':1,'private':secret},
            'native':{'error':secret,'source':{'error':secret},'address':secret,'pairing_code':secret},'sdp':secret}
        encoded=json.dumps(module.summary(raw))
        self.assertNotIn(secret,encoded)
        self.assertEqual(module.summary(raw)['browserCandidateSummary']['published'],1)
        self.assertIsNone(module.summary({'receiverFinal':{'framesDecoded':float('nan')}})['browserFramesDecoded'])
    def test_fixed_errors_and_candidate_types_are_retained(self):
        row=module.summary({'buildTag':'p3-lan-v2','native':{'error':'INPUT_SELECTED_PAIR_NOT_APPROVED'},
            'receiverFinal':{'localCandidateType':'prflx','remoteCandidateType':'host'}})
        self.assertEqual(row['error'],'INPUT_SELECTED_PAIR_NOT_APPROVED')
        self.assertEqual(row['browserLocalCandidateType'],'prflx')
        row=module.summary({'native':{'source':{'error':'SPS_PROFILE_INCOMPATIBLE_42c01f'}}})
        self.assertEqual(row['sourceError'],'SPS_PROFILE_INCOMPATIBLE_42c01f')
        row=module.summary({'native':{'source':{'error':'SPS_PROFILE_INCOMPATIBLE_PRIVATE'}}})
        self.assertIsNone(row['sourceError'])
    def test_candidate_counts_are_bounded_and_not_coerced(self):
        for invalid in (-1,129,True,'1'):
            self.assertIsNone(module.count_summary(dict(before=invalid,mdns=0,published=0,mdnsPublished=0)))
        self.assertIsNone(module.count_summary(dict(before=1,mdns=1,published=0,mdnsPublished=1)))
    def test_pair_diagnostics_only_contain_approved_categories(self):
        secret='EXCLUDED'
        row=dict(reason='EXACT_MATCH',candidate_type='host',protocol='udp',approved=True,
            approved_count=1,same_address_count=1,same_port_count=1,exact_match_count=1,address=secret)
        result=module.pair_summary(dict(local=row,remote=row))
        self.assertNotIn(secret,json.dumps(result));self.assertTrue(result['local']['approved'])
        row['reason']=secret;row['approved_count']=secret
        result=module.pair_summary(dict(local=row,remote=row))
        self.assertIsNone(result['remote']['reason']);self.assertIsNone(result['remote']['approved_count'])

if __name__=='__main__':unittest.main()
