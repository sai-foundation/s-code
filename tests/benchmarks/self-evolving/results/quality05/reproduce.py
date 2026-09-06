#!/usr/bin/env python3
"""Recompute published aggregate arithmetic, without models or private artifacts."""
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent


def main():
    for manifest_name in ('publishable-files.json', 'reproduction-files.json'):
        manifest = json.loads((ROOT / manifest_name).read_text())
        for row in manifest['files']:
            name = row['file']
            if not isinstance(name, str) or Path(name).name != name:
                raise ValueError('Expected a local evidence filename')
            contents = (ROOT / name).read_bytes()
            if hashlib.sha256(contents).hexdigest() != row['sha256'] or len(contents) != row['bytes']:
                raise ValueError(f'Evidence hash mismatch: {name}')
    # Import only after validating the exact published audit sources.
    from audit import build_report
    report = json.loads((ROOT / 'quality05.json').read_text())
    recomputed = build_report(report['slots'], report['training_components'], report['family_audits'])
    for field, value in recomputed.items():
        if report[field] != value:
            raise ValueError(f'Aggregate reproduction mismatch: {field}')
    print(json.dumps({'passed': True, 'scope': 'public aggregate arithmetic only',
                      'provider_calls': recomputed['physical_experiment_accounting']['provider_calls'],
                      'prospective_screen_passed': recomputed['development_readiness']['prospective_screen_passed'],
                      'confirmatory_claim': False}))


if __name__ == '__main__':
    main()
