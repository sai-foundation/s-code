#!/usr/bin/env python3
"""Public, offline boundary regressions for the exact copied post-hoc functions."""
import copy
import json
from pathlib import Path
import tempfile
import unittest
import reproduce


class ParserDiagnosis(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.audit,_=reproduce.load_audit()
        cls.supplement=reproduce.load('quality08_public_parser_diagnosis','posthoc/parser-supplement.py')
    def test_scope_normalization_is_narrow(self):
        scope={'organization_id':'bench','team_id':'bench','actor_id':'bench'}
        self.assertTrue(self.supplement.scope_equivalent({**scope,'goal_id':None,'task_id':None},scope))
        for value in ({**scope,'actor_id':'other'},{**scope,'goal_id':'other'},{**scope,'extra':None}):
            self.assertFalse(self.supplement.scope_equivalent(value,scope))
    def test_task_order_does_not_erase_metadata_changes(self):
        a={'task_manifest':self.audit.task_metadata()};b=copy.deepcopy(a);b['task_manifest'].reverse()
        self.assertTrue(self.supplement.protocol_equivalent(a,b))
        b['task_manifest'][0]['family']='different';self.assertFalse(self.supplement.protocol_equivalent(a,b))
    def test_provider_ids_are_not_guessed_as_internal_ids(self):
        bodies=[{'messages':[{'role':'assistant','tool_calls':[{'id':'call-synthetic','function':{'name':'read_file','arguments':'{}'}}]},
            {'role':'tool','tool_call_id':'call-synthetic','content':'{"content":"text"}'}]}]
        self.assertEqual(set(self.supplement.outbound_tools(bodies)),{'call-synthetic'})
    def test_literal_fragment_position_is_verified(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);path=root/'x.py';path.write_text('first\nsecond\n');digest=self.supplement.sha(path)
            lesson={'files':[{'path':'x.py','sha256':digest}],'source_observation':{'path':'x.py','sha256':digest,'start_line':1,'end_line':2,
                'fragments':[{'start_line':2,'text':'second\n'}],'truncated':True}}
            self.assertTrue(self.supplement.literal_observation(lesson,root,self.audit))
            lesson['source_observation']['fragments'][0]['start_line']=1
            self.assertFalse(self.supplement.literal_observation(lesson,root,self.audit))
    def test_published_diagnosis_preserves_unknowns_and_failed_screen(self):
        diagnosis=reproduce.read('posthoc/parser-diagnosis.json');review=reproduce.read('independent-review.json')
        self.assertFalse(diagnosis['original_screen_reclassified']);self.assertFalse(diagnosis['exact_preverifier_source_provenance_established'])
        self.assertIsNone(review['accounting']['complete_total_tokens']);self.assertIsNone(review['accounting']['complete_cost_usd'])
        self.assertEqual(review['accounting']['unknown_usage_calls'],1)
        self.assertEqual({a:r['successes'] for a,r in review['quality'].items() if a in ('off','raw','learned')},{'off':26,'raw':26,'learned':25})
        self.assertFalse(review['prospective_screen_passed']);self.assertFalse(review['confirmatory_claim'])


if __name__=='__main__':unittest.main(verbosity=2)
