#!/usr/bin/env python3
"""Post-hoc parser diagnosis only; original freeze/audit/outcomes remain immutable.

Reads public request tool evidence/initial bodies and typed terminal API records.
Never invokes a daemon, grader, model, profile reader, response-SSE parser or key.
No endpoint or admission criterion is changed by this supplemental diagnosis.
"""
import argparse
import collections
import hashlib
import importlib.util
import json
from pathlib import Path
import sys
sys.dont_write_bytecode=True
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'.work'))
EXPECTED_ANALYZER='86e4b4583b372ea294f4363af2f3baf8d4235ab0bf16ba30b9be685bb85c6954'


def sha(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def read(path):return json.loads(path.read_text())
def digest(value):return hashlib.sha256(json.dumps(value,sort_keys=True,ensure_ascii=False,separators=(',',':'),allow_nan=False).encode()).hexdigest()

def scope_equivalent(value,expected):
    return (isinstance(value,dict) and set(expected) <= set(value)
        and set(value) <= set(expected)|{'goal_id','task_id'}
        and all(value.get(k)==v for k,v in expected.items())
        and value.get('goal_id') is None and value.get('task_id') is None)


def protocol_equivalent(a,b):
    if not isinstance(a,dict) or not isinstance(b,dict) or set(a)!=set(b):return False
    if any(a[k]!=b[k] for k in a if k!='task_manifest'):return False
    for value in (a,b):
        rows=value['task_manifest']
        if not isinstance(rows,list) or len(rows)!=12 or len({r['task_id'] for r in rows})!=12:return False
    return sorted(a['task_manifest'],key=lambda r:r['task_id'])==sorted(b['task_manifest'],key=lambda r:r['task_id'])


def literal_observation(lesson,source,A):
    """No claim about which internal tool produced these bytes or when."""
    try:
        obs=lesson['source_observation']
        if set(obs)!={'path','sha256','start_line','end_line','fragments','truncated'}:return False
        name=A.relative_source(obs['path']);path=source/name
        if path.is_symlink() or not path.is_file():return False
        data=path.read_bytes();text=data.decode('utf8');lines=A.literal_lines(text)
        if sha(path)!=obs['sha256'] or lesson['files']!=[{'path':name,'sha256':obs['sha256']}]:return False
        start,end=obs['start_line'],obs['end_line']
        if type(start) is not int or type(end) is not int or not 1<=start<=end<=len(lines) or type(obs['truncated']) is not bool:return False
        if not isinstance(obs['fragments'],list) or not 1<=len(obs['fragments'])<=2:return False
        last=start-1
        for part in obs['fragments']:
            if not isinstance(part,dict) or set(part)!={'start_line','text'}:return False
            first,content=part['start_line'],part['text']
            if type(first) is not int or not isinstance(content,str) or not content:return False
            final=first+len(A.literal_lines(content))-1
            if not last<first<=final<=end or content!=''.join(lines[first-1:final]):return False
            last=final
        return True
    except (ValueError,TypeError,KeyError,OSError):return False


def outbound_tools(bodies):
    calls,results,ambiguous={}, {}, set()
    for body in bodies:
        for message in body.get('messages',[]):
            if message.get('role')=='assistant':
                for call in message.get('tool_calls',[]):
                    try:value={'tool':call['function']['name'],'arguments':json.loads(call['function']['arguments'])}
                    except (ValueError,KeyError,TypeError):continue
                    key=call['id']
                    if key in calls and calls[key]!=value:ambiguous.add(key)
                    calls[key]=value
            elif message.get('role')=='tool':
                try:value=json.loads(message['content']);key=message['tool_call_id']
                except (ValueError,KeyError,TypeError):continue
                if key in results and results[key]!=value:ambiguous.add(key)
                results[key]=value
    return {k:{**calls[k],'result':results[k]} for k in calls.keys()&results.keys()-ambiguous}


def public_read_matches(lesson,tools,source,A):
    """Existence of matching public read content; never an internal-ID mapping."""
    count=0;obs=lesson['source_observation'];lines=A.literal_lines((source/obs['path']).read_bytes().decode('utf8'))
    for call in tools.values():
        if call['tool']!='read_file' or not isinstance(call['result'],dict):continue
        value=call['result']
        try:
            if A.relative_source(call['arguments']['path'],canonical=False)!=obs['path'] or A.relative_source(value['path'],canonical=False)!=obs['path'] or value['sha256']!=obs['sha256']:continue
            if value['start_line']!=obs['start_line'] or value['end_line']!=obs['end_line']:continue
            text=value['content'];expected=''.join(lines[obs['start_line']-1:obs['end_line']])
            if not isinstance(text,str) or not expected.startswith(text):continue
            if all(text[len(''.join(lines[obs['start_line']-1:p['start_line']-1])):][:len(p['text'])]==p['text'] for p in obs['fragments']):count+=1
        except (KeyError,TypeError,ValueError):continue
    return count


def internal_chronology(lesson,snapshot,A):
    """Exact internal-ID timestamps only; command output cannot be linked here."""
    tools={item['content']['tool_call_id']:item for item in snapshot.get('items',[]) if item.get('content',{}).get('type')=='tool_call'}
    try:
        read_id,verify_id=lesson['evidence_tool_call_ids'];r,v=tools[read_id],tools[verify_id]
        if r['content']['tool']!='read_file' or v['content']['tool']!='run_command' or r['status']!='completed' or v['status']!='completed':return False
        if A.instant(r['created_at'])>=A.instant(v['created_at']) or A.instant(r['completed_at'])>A.instant(v['created_at']):return False
        for key,call in tools.items():
            if key==verify_id or call['content']['tool'] in {'read_file','list_files','search_text','git_diff','git_status'}:continue
            if (A.instant(call['created_at']),key)>(A.instant(v['created_at']),verify_id) or call.get('completed_at') is None or A.instant(call['completed_at'])>A.instant(v['created_at']):return False
        return True
    except (ValueError,TypeError,KeyError):return False


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--output-dir',type=Path,required=True);args=parser.parse_args()
    if args.output_dir.exists():parser.error('New supplement output directory required')
    analyzer=ROOT/'.work/analyze-quality08.py'
    if sha(analyzer)!=EXPECTED_ANALYZER:raise ValueError('Frozen analyzer differs')
    spec=importlib.util.spec_from_file_location('original_quality08_auditor',analyzer);A=importlib.util.module_from_spec(spec);spec.loader.exec_module(A)
    freeze=read(ROOT/A.FREEZE);A.verify_freeze(ROOT,freeze);A.approval(ROOT,ROOT/A.FREEZE)
    original=ROOT/'.work/quality08-reviewed';original_files={p.name:sha(p) for p in original.iterdir() if p.is_file()}
    report=read(original/'quality08.json');measurements=read(ROOT/A.TRANSFER/'measurements.json');projected=read(original/'measurements.json')
    comparisons={'all_108_attempt_records_equal':measurements['attempts']==projected['attempts'],
        'all_12_training_components_equal':measurements['training']==projected['training'],
        'protocol_equal_after_task_metadata_order_only':protocol_equivalent(measurements['protocol'],projected['protocol']),
        'protocol_raw_order_equal':measurements['protocol']==projected['protocol']}
    contexts={};families=[];scopes=[];input_hashes={}
    for family in A.FAMILIES:
        directory=ROOT/'.work'/f'training-quality-{family}-08';snapshot=read(directory/'training/snapshot.json');turn=read(directory/'training/turn.json')
        result=read(directory/'training/result.json');frozen=read(directory/'frozen.json');lessons=frozen['lessons']
        bodies=[read(directory/'requests'/f'{i:04d}'/'request.json') for i in result['requests']]
        public=outbound_tools(bodies)
        internal={item['content']['tool_call_id'] for item in snapshot['items'] if item.get('content',{}).get('type')=='tool_call'}
        literal=[literal_observation(lesson,directory/'trained-source',A) for lesson in lessons]
        matches=[public_read_matches(lesson,public,directory/'trained-source',A) for lesson in lessons]
        chronology=[internal_chronology(lesson,snapshot,A) for lesson in lessons]
        row=dict(family=family,saved_observation_count=len(lessons),literal_source_hash_and_fragments_valid=sum(literal),
            matching_public_read_exists=sum(x>0 for x in matches),matching_public_read_candidate_counts=matches,
            internal_evidence_chronology_valid=sum(chronology),internal_tool_count=len(internal),public_provider_tool_count=len(public),
            direct_id_intersection_count=len(internal&set(public)),closed_checkpoint_present=turn.get('checkpoint') is not None,
            exact_cross_namespace_linkage_established=False,
            limitation='No retained explicit daemon tool ID to provider call ID correlation; order/display similarity is not treated as proof.')
        families.append(row);contexts[family]=dict(root=directory,lessons=lessons,literal_ids=[l['id'] for l,v in zip(lessons,literal) if v],corpus=read(directory/'raw-corpus.json'))
        scopes.append(scope_equivalent(snapshot['session']['scope'],{'organization_id':'bench','team_id':'bench','actor_id':'bench'}))
        for name in ('training/snapshot.json','training/turn.json','frozen.json','raw-corpus.json'):
            input_hashes[family+'/'+name]=sha(directory/name)
    slots=[]
    transfer=ROOT/A.TRANSFER
    for plan in A.plans()[3:]:
        context=contexts[plan['family']];attempt=transfer/'attempts'/plan['attempt_directory'];result=read(attempt/'result.json');snapshot=read(attempt/'snapshot.json')
        scopes.append(scope_equivalent(snapshot['session']['scope'],{'organization_id':'bench','team_id':'bench','actor_id':'bench'}))
        counts=0;initial=False;invalid=0;common=None
        for pos,index in enumerate(result['requests']):
            body=read(transfer/'requests'/f'{index:04d}'/'request.json')
            diagnosis=A.experience(body,plan['arm'],context['literal_ids'],context['lessons'],context['corpus'])
            if diagnosis['issues']:invalid+=1
            elif diagnosis['source_observations']:
                counts+=1
                if pos==0:initial=True
            if pos==0:common=A.common_initial_request(body,plan['arm'],context['literal_ids'],context['lessons'],context['corpus'])
        slots.append(dict(task_id=plan['task_id'],family=plan['family'],seed=plan['seed'],arm=plan['arm'],negative_control=plan['negative_control'],
            literal_frozen_observation_delivery_requests=counts,initial_literal_delivery=initial,invalid_frozen_envelopes=invalid,
            common_initial_request_sha256=common,fully_verified_preverifier_exposure=None))
    triplets=[]
    for block in A.schedule():
        rows=[r for r in slots if r['task_id']==block['task_id'] and r['seed']==block['seed']]
        values=[r['common_initial_request_sha256'] for r in rows]
        triplets.append(dict(task_id=block['task_id'],seed=block['seed'],equal_after_exact_frozen_treatment_removal=len(values)==3 and None not in values and len(set(values))==1))
    output=dict(schema_version=1,round='quality08',phase='posthoc-parser-diagnosis',evidence_class='exposed-development',
        frozen_report_unchanged=True,original_screen_reclassified=False,confirmatory_claim=False,holdout_reveal_authorized=False,
        exact_preverifier_source_provenance_established=False,
        findings=[{'id':'scope_optional_null_fields','classification':'parser_false_negative','affected_slots':len(scopes),
            'valid_after_narrow_normalization':sum(scopes),'rule':'Only goal_id/task_id absent versus null are equivalent; all organization/team/actor fields still match exactly.'},
            {'id':'task_manifest_display_order','classification':'parser_false_negative',**comparisons},
            {'id':'tool_id_join','classification':'parser_assumption_and_missing_correlation_evidence',
             'rule':'Frozen tests incorrectly used one ID namespace. Literal file bytes and faithful injection are independently diagnosable, but this supplement does not invent the missing ID correlation.'}],
        family_diagnostics=families,delivery_diagnostics=slots,initial_triplets=triplets,
        original_outcomes={'successes':{a:report['arms'][a]['qualified_successes'] for a in A.ARMS},'unknown_token_slots':{a:report['arms'][a]['unknown_token_slots'] for a in A.ARMS},
            'prospective_screen_passed':report['development_readiness']['prospective_screen_passed']},
        interpretation=['All original 111 slots, grading failures and unknown accounting remain. No model/grader rerun or candidate mutation.',
            'Source-fragment equality and exact frozen payload delivery do not by themselves prove the required pre-verifier origin or utility.',
            'The complete-readiness and no-success-loss criteria are not met independently of these parser diagnoses; no advance/reveal or efficacy claim follows.',
            'This is train-once/frozen-reuse exposed development, not continuous learning or a fresh holdout.'],
        bindings={'source_revision':freeze['revision'],'development_freeze_sha256':sha(ROOT/A.FREEZE),'approval_sha256':sha(ROOT/A.APPROVAL),
            'frozen_analyzer_sha256':sha(analyzer),'original_public_files':original_files,'actual_measurements_sha256':sha(transfer/'measurements.json'),
            'supplement_source_sha256':sha(Path(__file__)),'training_metadata_sha256':input_hashes})
    A.public_safe(output)
    A.verify_freeze(ROOT,freeze)
    if {p.name:sha(p) for p in original.iterdir() if p.is_file()}!=original_files:raise ValueError('Original audit changed')
    args.output_dir.mkdir(mode=0o700)
    (args.output_dir/'parser-diagnosis.json').write_text(json.dumps(output,indent=2,sort_keys=True)+'\n')
    paragraphs=['# Quality08 post-hoc parser diagnosis','','The frozen report is retained unchanged. This supplement changes no criterion, grade, token total, failure, or unknown value. It makes no confirmatory or advancement claim.','',
        f'All {sum(scopes)}/{len(scopes)} API scope records match after accepting only the two optional null fields. All 108 measured attempt rows and 12 training components equal the published projection; the protocol discrepancy is only the original R/Q/F versus synthesized F/Q/R task-list order.','',
        'The saved observations use internal daemon tool IDs, while outbound tool calls use provider IDs. The public artifacts retain no explicit ID correlation and closed checkpoints are absent. The synthetic test fixture incorrectly equated these namespaces. We do not substitute order or display similarity for exact linkage.','',
        '| Family | Saved | Literal source valid | Matching public read exists | Internal chronology valid | Exact cross-ID link |','| --- | ---: | ---: | ---: | ---: | --- |']
    for f in families:paragraphs.append(f"| {f['family']} | {f['saved_observation_count']} | {f['literal_source_hash_and_fragments_valid']} | {f['matching_public_read_exists']} | {f['internal_evidence_chronology_valid']} | Unproven |")
    paragraphs += ['',f"Faithful frozen literal-observation delivery appeared in {sum(r['literal_frozen_observation_delivery_requests'] for r in slots)} requests. {sum(r['initial_literal_delivery'] for r in slots if r['arm']=='learned' and not r['negative_control'])}/27 positive learned initial requests contained it. {sum(t['equal_after_exact_frozen_treatment_removal'] for t in triplets)}/36 initial triplets match after removing only exact frozen treatment envelopes. These are limited delivery diagnostics, not fully verified provenance or usefulness.",'',
        'Original outcomes remain off 26/36, raw 26/36, learned 25/36, with one off attempt having unknown token accounting. The original screen remains false. Even resolving all parsing issues cannot erase the quality loss or missing accounting.','',
        'No model call, grader replay, profile decryption, raw response/SSE reading, private reasoning inspection, candidate edit, original report replacement, or new sealed-task access occurred.']
    (args.output_dir/'parser-diagnosis.md').write_text('\n'.join(paragraphs)+'\n')
    print(json.dumps(dict(scope_valid=sum(scopes),total_slots=len(scopes),literal_saved=sum(f['literal_source_hash_and_fragments_valid'] for f in families),
        literal_delivery_requests=sum(r['literal_frozen_observation_delivery_requests'] for r in slots),matching_initial_triplets=sum(r['equal_after_exact_frozen_treatment_removal'] for r in triplets),
        exact_provenance_established=False,original_screen_reclassified=False)))

if __name__=='__main__':main()
