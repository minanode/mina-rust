"""Fixture control checks on the captured decoded metadata; no OCaml run."""
import copy
import json
from pathlib import Path
import unittest

from verify_full_proof import check_unrelated_work_pair, prepare

ROOT = Path(__file__).resolve().parent


class CorpusTests(unittest.TestCase):
    def setUp(self):
        self.pair = [json.loads((ROOT / 'full-proof' / name / 'decoded.json').read_text())
                     for name in ('devnet-581348-coinbase', 'devnet-581351-signed')]

    def test_captured_pair_and_all_controls(self):
        check_unrelated_work_pair(*self.pair)
        cases = prepare(ROOT / 'full-proof')
        self.assertEqual(len(cases), 8)
        unrelated = next(case for case in cases if case['case'] == 'control-unrelated-work')
        self.assertEqual(unrelated['snark_work'], cases[1]['snark_work'])

    def test_mismatched_submitter_fee_and_sok_are_rejected(self):
        for field in ('submitter', 'fee', 'sok_message', 'proof_sok_digest_hex',
                      'expected_sok_digest_hex'):
            pair = copy.deepcopy(self.pair)
            target = pair[1] if field == 'submitter' else pair[1]['snark_work']
            target[field] = 'different'
            with self.subTest(field=field), self.assertRaises(ValueError):
                check_unrelated_work_pair(*pair)

    def test_equal_targets_or_inconsistent_metadata_are_rejected(self):
        pair = copy.deepcopy(self.pair)
        pair[1]['staged_ledger_ledger_hash'] = pair[0]['staged_ledger_ledger_hash']
        pair[1]['snark_work']['statement']['target']['second_pass_ledger'] = pair[0]['staged_ledger_ledger_hash']
        with self.assertRaisesRegex(ValueError, 'different block ledger targets'):
            check_unrelated_work_pair(*pair)
        for field in ('sok_matches', 'target_matches_block'):
            pair = copy.deepcopy(self.pair)
            pair[0]['snark_work'][field] = False
            with self.subTest(field=field), self.assertRaises(ValueError):
                check_unrelated_work_pair(*pair)

    def test_missing_work_is_rejected(self):
        pair = copy.deepcopy(self.pair)
        pair[1]['snark_work'] = None
        with self.assertRaisesRegex(ValueError, 'two work proofs'):
            check_unrelated_work_pair(*pair)


if __name__ == '__main__':
    unittest.main()
