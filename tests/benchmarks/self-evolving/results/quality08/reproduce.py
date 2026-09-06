#!/usr/bin/env python3
"""Reproduce public arithmetic and verify file bindings; no private inputs/models."""
import hashlib
import importlib.util
import json
from pathlib import Path, PurePosixPath
import sys
sys.dont_write_bytecode=True
ROOT=Path(__file__).resolve().parent


def read(name):return json.loads((ROOT/name).read_text())


def verify_files():
    manifest=read('reproduction-files.json')
    expected={r['file'] for r in manifest['files']}
    if len(expected)!=len(manifest['files']):raise ValueError('Duplicate package file')
    actual={p.relative_to(ROOT).as_posix() for p in ROOT.rglob('*') if p.is_file() and '__pycache__' not in p.parts}
    if actual != expected|{'reproduction-files.json'}:raise ValueError('Unexpected or missing package files')
    for row in manifest['files']:
        name=row['file'];relative=PurePosixPath(name)
        if relative.is_absolute() or '..' in relative.parts or str(relative)!=name:raise ValueError('Unsafe package path')
        path=ROOT/name
        if any(p.is_symlink() for p in [path,*path.parents] if p.is_relative_to(ROOT)):raise ValueError('Symlink package file')
        value=path.read_bytes()
        if hashlib.sha256(value).hexdigest()!=row['sha256'] or len(value)!=row['bytes']:raise ValueError('Package hash mismatch: '+name)
    for name,expected_hash in read('public-files.json')['files'].items():
        if hashlib.sha256((ROOT/name).read_bytes()).hexdigest()!=expected_hash:raise ValueError('Original frozen output changed: '+name)
    return manifest


def load(name,path):
    spec=importlib.util.spec_from_file_location(name,ROOT/path)
    module=importlib.util.module_from_spec(spec);sys.modules[name]=module;spec.loader.exec_module(module);return module


def load_audit():
    verify_files()
    numerical=load('quality08_public_numerical','analysis.py')
    common=load('quality08_common','quality08_common.py')
    # Packaging adapter only: the exact private common helper locates analysis.py
    # inside its original checkout. Resolve the identical copied implementation
    # here; the protocol, task metadata and schedule algorithm remain unchanged.
    common.schedule=lambda:numerical.schedule({'protocol':{**common.protocol(),'task_manifest':common.task_metadata()}})
    return load('quality08_public_audit','audit.py'),numerical


def main():
    audit,numerical=load_audit()
    report=read('quality08.json')
    recomputed=audit.build_report(report['slots'],report['training_components'],report['family_audits'],report['ledger_audits'],report['integrity_issues'])
    for key,value in recomputed.items():
        if report[key]!=value:raise ValueError('Public aggregate mismatch: '+key)
    calculated=numerical.analyze(read('measurements.json'))
    if calculated!=read('numerical-diagnostics.json'):raise ValueError('Numerical diagnostic mismatch')
    review=read('independent-review.json');diagnosis=read('posthoc/parser-diagnosis.json')
    if diagnosis['original_screen_reclassified'] or diagnosis['exact_preverifier_source_provenance_established']:raise ValueError('Unexpected upgraded post-hoc claim')
    for arm in ('off','raw','learned'):
        if report['arms'][arm]['qualified_successes']!=review['quality'][arm]['successes']:raise ValueError('Quality cross-review mismatch')
    print(json.dumps({'passed':True,'scope':'public aggregate arithmetic and immutable-package hashes; not private-ledger or provenance reconstruction',
        'physical_requests':review['accounting']['physical_requests'],'known_usage_calls':review['accounting']['known_usage_calls'],
        'known_tokens_subtotal':review['accounting']['known_tokens_subtotal'],'known_cost_usd_subtotal':review['accounting']['known_cost_usd_subtotal'],
        'complete_total_tokens':None,'complete_cost_usd':None,'prospective_screen_passed':False,
        'literal_delivery_requests':review['source_delivery']['literal_delivery_requests'],
        'exact_preverifier_linkage':'unknown','confirmatory_claim':False}))


if __name__=='__main__':main()
