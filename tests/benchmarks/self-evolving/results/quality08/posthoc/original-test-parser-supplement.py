#!/usr/bin/env python3
"""Offline regression tests for the explicitly post-hoc diagnosis, not the frozen protocol."""
import copy
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
sys.dont_write_bytecode=True
ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('posthoc08',ROOT/'.work/quality08-parser-supplement.py');S=importlib.util.module_from_spec(spec);spec.loader.exec_module(S)
spec=importlib.util.spec_from_file_location('frozen08',ROOT/'.work/analyze-quality08.py');A=importlib.util.module_from_spec(spec);spec.loader.exec_module(A)

class DiagnosticBoundaries(unittest.TestCase):
    def test_scope_accepts_only_known_absent_or_null_optional_fields(self):
        scope={'organization_id':'bench','team_id':'bench','actor_id':'bench'}
        self.assertTrue(S.scope_equivalent(scope,scope));self.assertTrue(S.scope_equivalent({**scope,'goal_id':None,'task_id':None},scope))
        for wrong in ({**scope,'actor_id':'other'},{**scope,'task_id':'other'},{**scope,'unreviewed':None},{'organization_id':'bench'}):self.assertFalse(S.scope_equivalent(wrong,scope))
    def test_task_manifest_order_only_does_not_ignore_labels_or_duplicates(self):
        a={'task_manifest':A.task_metadata(),'arms':list(A.ARMS)};b=copy.deepcopy(a);b['task_manifest'].reverse()
        self.assertTrue(S.protocol_equivalent(a,b))
        b['task_manifest'][0]['negative_control']=False;self.assertFalse(S.protocol_equivalent(a,b))
        b=copy.deepcopy(a);b['task_manifest'][0]=b['task_manifest'][1];self.assertFalse(S.protocol_equivalent(a,b))
    def test_distinct_id_namespaces_are_not_inferred_from_same_tool_or_order(self):
        body={'messages':[{'role':'assistant','tool_calls':[{'id':'call-public','function':{'name':'read_file','arguments':'{"path":"x.py"}'}}]},
            {'role':'tool','tool_call_id':'call-public','content':'{"content":"x\\n"}'}]}
        tools=S.outbound_tools([body]);self.assertEqual(set(tools),{'call-public'});self.assertNotIn('tool-internal',tools)
    def test_ambiguous_public_result_is_not_a_content_match(self):
        body={'messages':[{'role':'assistant','tool_calls':[{'id':'c','function':{'name':'read_file','arguments':'{}'}}]},
            {'role':'tool','tool_call_id':'c','content':'{"content":"a"}'},{'role':'tool','tool_call_id':'c','content':'{"content":"b"}'}]}
        self.assertEqual(S.outbound_tools([body]),{})
    def test_literal_bytes_are_not_fabricated_or_repositioned(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);file=root/'x.py';file.write_text('first\nsecond\n');h=S.sha(file)
            lesson={'files':[{'path':'x.py','sha256':h}],'source_observation':{'path':'x.py','sha256':h,'start_line':1,'end_line':2,'fragments':[{'start_line':2,'text':'second\n'}],'truncated':True}}
            self.assertTrue(S.literal_observation(lesson,root,A))
            lesson['source_observation']['fragments'][0]['start_line']=1;self.assertFalse(S.literal_observation(lesson,root,A))
            lesson['source_observation']['fragments'][0]={'start_line':2,'text':'invented\n'};self.assertFalse(S.literal_observation(lesson,root,A))
    def test_actual_diagnosis_never_upgrades_screen_or_exact_linkage(self):
        value=S.read(ROOT/'.work/quality08-posthoc-parser-diagnosis/parser-diagnosis.json')
        self.assertFalse(value['original_screen_reclassified']);self.assertFalse(value['exact_preverifier_source_provenance_established'])
        self.assertTrue(all(f['exact_cross_namespace_linkage_established'] is False for f in value['family_diagnostics']))
        self.assertEqual(value['original_outcomes']['successes'],{'off':26,'raw':26,'learned':25})
        self.assertEqual(value['original_outcomes']['unknown_token_slots']['off'],1)
        self.assertFalse(value['confirmatory_claim']);self.assertFalse(value['holdout_reveal_authorized'])
    def test_original_artifacts_still_match_supplement_bindings(self):
        value=S.read(ROOT/'.work/quality08-posthoc-parser-diagnosis/parser-diagnosis.json');bind=value['bindings']
        self.assertEqual(S.sha(ROOT/'.work/analyze-quality08.py'),S.EXPECTED_ANALYZER)
        for name,h in bind['original_public_files'].items():self.assertEqual(S.sha(ROOT/'.work/quality08-reviewed'/name),h)
        self.assertEqual(S.sha(ROOT/'.work/quality08-transfer/measurements.json'),bind['actual_measurements_sha256'])
        A.public_safe(value)

if __name__=='__main__':unittest.main(verbosity=2)
