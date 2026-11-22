"""Offline evidence-gate regression. Mutations are in memory, never native UI results."""
import copy
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('manual_gate', Path(__file__).with_name('p106-manual-evidence.py'))
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


class EvidenceGateTests(unittest.TestCase):
    def setUp(self): self.bundle = copy.deepcopy(gate.load_bundle())
    def rejected(self):
        with self.assertRaises(ValueError): gate.validate(self.bundle)
    def test_real_recorded_bundle_and_unknown_exit_code_are_not_zero(self):
        result = gate.validate(self.bundle)
        self.assertFalse(result['exitCodeZeroClaimed'])
        self.assertTrue(all(r['exitCodeVerdict'] == 'UNAVAILABLE' for r in result['instances'].values()))
    def test_missing_user_step_rejected(self):
        self.bundle['users'].pop(); self.rejected()
    def test_duplicate_user_response_rejected(self):
        self.bundle['users'][-1] = copy.deepcopy(self.bundle['users'][0]); self.rejected()
    def test_error_report_cannot_be_treated_as_pass(self):
        self.bundle['users'][-1]['answer'] = 'FAILURE_REPORTED'; self.rejected()
    def test_simulated_response_cannot_replace_user(self):
        self.bundle['users'][0]['source'] = 'EDGE_FIXTURE'; self.rejected()
    def test_hidden_instance_must_remain_unique(self):
        row = next(r for r in self.bundle['processes']['B'] if r.get('stage') == 'hidden')
        row['namedInstanceCount'] = 2; self.rejected()
    def test_process_identity_switch_rejected(self):
        self.bundle['processes']['B'][-1]['processId'] += 1; self.rejected()
    def test_forced_termination_cannot_prove_exit(self):
        row = next(r for r in self.bundle['processes']['A'] if r['kind'] == 'EXIT_OBSERVED')
        row['agentTerminatedApplication'] = True; self.rejected()
    def test_known_nonzero_exit_and_residue_rejected(self):
        for field, value in [('exitCode', 3), ('remainingNamedInstanceCount', 1)]:
            with self.subTest(field=field):
                self.bundle = copy.deepcopy(gate.load_bundle())
                row = next(r for r in self.bundle['processes']['C'] if r['kind'] == 'EXIT_OBSERVED')
                row[field] = value; self.rejected()
    def test_experimental_feature_rejected(self):
        self.bundle['build']['desktopCargoFeatures'].append('p004-probe'); self.rejected()
    def test_wrong_binary_hash_rejected(self):
        self.bundle['processes']['A'][0]['sha256'] = '0'*64; self.rejected()
    def test_observer_timeout_is_not_exit_evidence(self):
        self.bundle['processes']['A'].append(dict(kind='OBSERVER_TIMEOUT')); self.rejected()


if __name__ == '__main__': unittest.main()
