#!/usr/bin/env python3
"""Run a frozen, independently revealed transfer experiment through the daemon.

No task or grading answer is embedded here. The external task manifest contains
id/family/negative_control/prompt, plus optional seed preparation after reveal.
"""
from __future__ import annotations
import argparse
import hashlib
import http.server
import json
from pathlib import Path
import shutil
import subprocess
import threading
import time

from analysis import schedule, validate, token_complete
from pilot import grade, raw_retrieve
from run import Daemon, HERE, Meter, QUERY, ROOT, copy_tree, provider_totals, snapshot_source, tree_hash, write_json


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def experiment_phase(freeze):
    """Already exposed tasks remain development, even in the same paired runner."""
    if "phase" not in freeze or freeze["phase"] == "confirmatory-pre-reveal":
        return "holdout"
    if freeze["phase"] == "exposed-development":
        return "exposed-development"
    raise ValueError("Unknown evaluation phase; do not relabel exposed tasks as holdout")


def training_components(family, root):
    result = json.loads((root/'training/result.json').read_text())
    common, learning = [], []
    for index in result['requests']:
        directory = root/'requests'/f'{index:04d}'
        body = json.loads((directory/'request.json').read_text())
        record = json.loads((directory/'meter.json').read_text())
        # generate_title is false. Reflection is the only tool-less request.
        (common if body.get('tools') else learning).append(record)
    rows = []
    for component, records in [('common',common),('learned',learning),('off',[]),('raw',[])]:
        totals = provider_totals(records) if records else {'input_tokens':0,'output_tokens':0,'cost_usd':0}
        rows.append({'family':family,'component':component,'usage_complete':totals is not None,
                     'input_tokens':totals['input_tokens'] if totals else None,
                     'output_tokens':totals['output_tokens'] if totals else None,
                     'cost_usd':totals['cost_usd'] if totals else None,
                     'elapsed_seconds':None if component in ('common','learned') else 0,
                     'provider_seconds':sum(r['elapsed_seconds'] for r in records),
                     'training_agent_seconds':result['elapsed_seconds'],
                     'budget_denied':result['budget_denied']})
    return rows


def verify_training(family):
    root = Path(family['pilot_root']).resolve()
    frozen = json.loads((root/'frozen.json').read_text())
    if tree_hash(root/'trained-source') != family['source_hash'] or digest(root/'frozen.json') != family['frozen_file_hash'] or digest(root/'raw-corpus.json') != family['raw_corpus_hash']:
        raise RuntimeError('Frozen training artifacts changed')
    if frozen['source_hash'] != family['source_hash']:
        raise RuntimeError('Training source mismatch')
    # Older freezes did not retain correlation. They remain readable, but
    # absence is not a claim of complete source provenance for new audits.
    if 'tool_call_links_sha256' in frozen:
        links = root/'training/tool-call-links.json'
        if not links.is_file() or digest(links) != frozen['tool_call_links_sha256']:
            raise RuntimeError('Frozen training tool-call links changed')
    if tree_hash(root/'requests') != family['requests_hash'] or tree_hash(root/'training-profile') != family['profile_hash'] or digest(root/'training/result.json') != family['training_result_hash']:
        raise RuntimeError('Frozen training trajectory/profile changed')
    result = json.loads((root/'training/result.json').read_text())
    if result['status'] != 'completed' or not result['grade']['passed']:
        raise RuntimeError('Training source did not pass the independent training grader')
    return root, frozen


def prepare_attempt(family, profile, arm):
    # Revalidate before every arm, since the original pilot artifacts remain on
    # disk throughout the experiment. Do not silently accept mid-run changes.
    root, frozen = verify_training(family)
    workspace = root/'workspace'
    copy_tree(root/'trained-source', workspace)
    if tree_hash(workspace) != family['source_hash']:
        raise RuntimeError('Copied training source differs from the freeze')
    corpus = json.loads((root/'raw-corpus.json').read_text())
    if digest(root/'raw-corpus.json') != family['raw_corpus_hash']:
        raise RuntimeError('Raw corpus changed while preparing an attempt')
    if arm == 'learned':
        shutil.copytree(root/'training-profile', profile)
        if tree_hash(profile) != family['profile_hash']:
            raise RuntimeError('Copied training profile differs from the freeze')
    return frozen, workspace, corpus


def verify_grading_files(expected):
    for path, expected_hash in expected.items():
        if digest(path) != expected_hash:
            raise RuntimeError('Frozen grader or helper changed during the experiment')


def grade_frozen(result_dir, task, baseline, *, grader, trusted_helpers, expected_hashes):
    verify_grading_files(expected_hashes)
    try:
        return grade(result_dir, task, baseline, grader=grader, trusted_helpers=trusted_helpers)
    finally:
        verify_grading_files(expected_hashes)


def normalized(task, seed, arm, position, result, records):
    totals = result['provider_usage']
    details_complete = totals is not None and all(isinstance(r.get('usage'),dict) for r in records)
    def detail(section,key):
        values=[]
        if details_complete:
            for record in records:
                section_value=record['usage'].get(section)
                values.append(section_value.get(key) if isinstance(section_value,dict) else None)
        return sum(values) if values and all(type(v) is int for v in values) else None
    return {'task_id':task['id'],'seed':seed,'arm':arm,'order_position':position,
            'raw_grader_pass':result['grade']['passed'] if result['grade'].get('grading_complete') is True else None,
            'verified_success':(result['grade']['passed'] and result['status']=='completed') if result['grade'].get('grading_complete') is True else None,
            'usage_complete':totals is not None,'input_tokens':totals['input_tokens'] if totals else None,
            'output_tokens':totals['output_tokens'] if totals else None,
            'cost_usd':result['cost_usd'],'elapsed_seconds':result['elapsed_seconds'],
            'cached_input_tokens':detail('prompt_tokens_details','cached_tokens'),
            'reasoning_output_tokens':detail('completion_tokens_details','reasoning_tokens'),
            'budget_denied':result['budget_denied'],'status':result['status']}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--freeze',type=Path,required=True)
    parser.add_argument('--tasks',type=Path,required=True)
    parser.add_argument('--grader',type=Path,required=True)
    parser.add_argument('--key-file',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args()
    freeze=json.loads(args.freeze.read_text())
    phase=experiment_phase(freeze)
    tasks=json.loads(args.tasks.read_text())['tasks']
    output=args.output.resolve()
    if output.exists(): parser.error('output must be new; all attempts must be retained')
    output.mkdir(parents=True,mode=0o700)
    revision=subprocess.check_output(['git','-C',str(ROOT),'rev-parse','HEAD'],text=True).strip()
    if revision!=freeze['revision'] or digest(ROOT/'target/debug/s-code-daemon')!=freeze['daemon_sha256']:
        raise RuntimeError('Executable/revision differs from the preregistered freeze')
    source_hash=snapshot_source(output)
    if source_hash!=freeze['source_sha256']:
        raise RuntimeError('Source differs from the preregistered freeze')
    if digest(HERE/'analysis.py')!=freeze['analysis_sha256']:
        raise RuntimeError('Analysis changed after preregistration')
    protocol={**freeze['protocol'],'task_manifest':[{'task_id':t['id'],'family':t['family'],'negative_control':t['negative_control']} for t in tasks]}
    data={'schema_version':1,'protocol':protocol,'training':[],'attempts':[]}
    if phase == 'exposed-development':
        data.update(evidence_class=phase, confirmatory_claim=False)
    if len(freeze['families']) != 3 or {f['id'] for f in freeze['families']} != {'report','queue','flow'}:
        raise RuntimeError('Freeze must contain exactly the three project families')
    for family in freeze['families']:
        root,_=verify_training(family)
        pilot_manifest=json.loads((root/'manifest.json').read_text())
        if any(pilot_manifest.get(key)!=freeze.get(key) for key in ('model','provider','reasoning_effort','daemon_sha256')):
            raise RuntimeError('Training used a different model/provider/configuration or executable')
        data['training'].extend(training_components(family['id'],root))
    validate(data)
    if any(not token_complete(row) or row.get('budget_denied') for row in data['training']):
        raise RuntimeError('Training accounting is incomplete; efficiency evaluation cannot begin')
    blocks=schedule(data)
    grader=args.grader.resolve()
    trusted_helpers=(HERE/'fixtures/grade.py', HERE/'bounded_process.py')
    grading_hashes={path:digest(path) for path in (grader,*trusted_helpers)}
    verify_grading_files(grading_hashes)
    write_json(output/'schedule.json',blocks)
    write_json(output/'measurements.json',data)
    write_json(output/'freeze.json',freeze)
    write_json(output/'manifest.json',{'revision':revision,'source_sha256':source_hash,'tasks_sha256':digest(args.tasks),'grader_sha256':grading_hashes[grader],'grading_file_hashes':{str(path):value for path,value in grading_hashes.items()},'started_at':time.time(),'max_cost_usd':freeze['max_cost_usd'],'per_attempt_cost_cap':freeze['per_attempt_cost_cap'], 'phase':phase})
    meter=Meter(args.key_file.read_text().strip(),output,freeze['model'],freeze['max_cost_usd'],21600,freeze['provider'])
    if digest(meter.binary)!=freeze['daemon_sha256']:
        raise RuntimeError('Copied executable changed during preflight')
    meter.phase_max_cost=freeze['per_attempt_cost_cap']
    server=http.server.ThreadingHTTPServer(('127.0.0.1',0),meter.handler())
    threading.Thread(target=server.serve_forever,daemon=True).start()
    proxy=f'http://127.0.0.1:{server.server_port}/v1'
    daemons={}
    try:
        for block_index,block in enumerate(blocks):
            task=next(t for t in tasks if t['id']==block['task_id'])
            family=next(f for f in freeze['families'] if f['id']==task['family'])
            for position,arm in enumerate(block['arms']):
                profile=output/'profiles'/f'{block_index:03d}-{arm}'
                frozen,workspace,corpus=prepare_attempt(family,profile,arm)
                # Revealed controls may change a dependency before the task. Only
                # explicit file replacements from the immutable external manifest.
                for path,content in task.get('preparation',{}).items():
                    target=(workspace/path).resolve()
                    if not target.is_relative_to(workspace) or not target.is_file():
                        raise RuntimeError('Invalid declared seed preparation')
                    target.write_text(content)
                # Fresh state snapshot for every attempt. No task output, durable
                # cache, conversation, pending approval or preference can carry over.
                daemon=Daemon(profile,meter,proxy)
                daemons[task['family'],arm]=daemon
                cache=daemon.root/'tool-cache'
                if cache.exists(): shutil.rmtree(cache)
                check_id=daemon.session(workspace,'Frozen experience check','reuse' if arm=='learned' else 'off')
                if arm=='learned':
                    current=daemon.api('GET',f'/v1/sessions/{check_id}/lessons?{QUERY}')
                    setting=daemon.api('GET',f'/v1/sessions/{check_id}/learning?{QUERY}')
                    if current!=frozen['lessons'] or setting!=frozen['settings']:
                        raise RuntimeError('Frozen lesson set/generation changed')
                meter.raw=(lambda:raw_retrieve(corpus,workspace,task['prompt'])) if arm=='raw' else None
                attempt_dir=output/'attempts'/f'{block_index:03d}-{arm}'
                result=daemon.run(workspace,{'id':task['id'],'phase':phase,'prompt':task['prompt']},'reuse' if arm=='learned' else 'off',attempt_dir,seed=block['seed'],arm=arm)
                daemon.close()
                del daemons[task['family'],arm]
                result['grade']=grade_frozen(attempt_dir,task['id'],attempt_dir/'initial',grader=grader,trusted_helpers=trusted_helpers,expected_hashes=grading_hashes)
                write_json(attempt_dir/'result.json',result)
                records=[meter.records[index] for index in result['requests']]
                data['attempts'].append(normalized(task,block['seed'],arm,position,result,records))
                write_json(output/'measurements.json',data)
                print(json.dumps({'block':block_index,'task':task['id'],'seed':block['seed'],'arm':arm,'passed':result['grade']['passed'],'tokens':result['provider_usage'],'status':result['status']}),flush=True)
        write_json(output/'completed.json',{'finished_at':time.time(),'attempts':len(data['attempts'])})
    finally:
        # Preserve provider accounting even when the supervisor is interrupted.
        for daemon in daemons.values(): daemon.close()
        meter.settle()
        server.shutdown();server.server_close()


if __name__=='__main__':
    main()
