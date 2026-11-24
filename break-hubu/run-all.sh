#!/usr/bin/env bash
# Each episode owns a fresh server, database and credentials. Continue after failures.
set -u
kit_root="$(cd "$(dirname "$0")" && pwd)"
exec python3 - "$kit_root" "$@" <<'PY'
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile

root = Path(sys.argv[1])
sys.path.insert(0, str(root))
from bootstrap import candidate_version
from catalog import load_catalog

parser = argparse.ArgumentParser(description='Run every Break Hubu public proof kit')
source = parser.add_mutually_exclusive_group(required=True)
source.add_argument('--release', help='published stable tag, such as v0.2.5')
source.add_argument('--commit', help='full SHA of a commit in the local Hubu repository')
parser.add_argument('--output')
parser.add_argument('--cache')
parser.add_argument('--kits', help='comma-separated kit subset, such as ep05-denied-operation,ep06-approval-reuse')
args = parser.parse_args(sys.argv[2:])
try:
    version = candidate_version(args.commit) if args.commit else args.release
except (ValueError, subprocess.CalledProcessError) as error:
    parser.error(str(error))
output = Path(args.output).resolve() if args.output else Path(tempfile.mkdtemp(prefix='break-hubu-all-'))
if output.exists() and any(output.iterdir()):
    parser.error('--output must name a fresh empty directory')
output.mkdir(parents=True, exist_ok=True)
results = []
catalog = load_catalog(root.parent, version)
kits = list(catalog['kits'])
if args.kits:
    chosen = [kit.strip() for kit in args.kits.split(',') if kit.strip()]
    unknown = sorted(set(chosen) - set(kits))
    if unknown: parser.error('Unknown kits: ' + ', '.join(unknown))
    kits = [kit for kit in kits if kit in chosen]
for episode in kits:
    source_args = ['--commit', args.commit] if args.commit else ['--release', args.release]
    command = [str(root / episode / 'run.sh'), *source_args, '--output', str(output / episode)]
    if args.cache:
        command += ['--cache', args.cache]
    print('\nRUN', episode, flush=True)
    try:
        completed = subprocess.run(command, check=False)
        result_path = output / episode / 'result.json'
        result = json.loads(result_path.read_text()) if result_path.exists() else {
            'episode': episode, 'verdict': 'not tested', 'execution': 'incomplete',
            'execution_error': f'Runner exited {completed.returncode} without result.json'}
    except (OSError, ValueError) as error:
        result = {'episode': episode, 'verdict': 'not tested', 'execution': 'incomplete', 'execution_error': str(error)}
    results.append(result)
summary = {'release': version, 'build_mode': 'commit' if args.commit else 'release', 'episodes': results,
           'documentary_identity_gap': f'Does not hold in {version}: runtime/thread-bound account authority (HUB-162).',
           'authority_boundary': 'Executor routes still use a bearer (HUB-32); the test configuration stays fixed.',
           'scope': 'Hubu public HTTP/CLI accounting with synthetic executor receipts; no provider billing or Gongbu execution.'}
if args.commit:
    summary['source_commit'] = args.commit
(output / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
print('\nPER-CLAIM RESULTS')
for result in results:
    print(result['episode'] + ': ' + result['verdict'] + ' (' + result.get('execution', 'unknown') + ')')
print(summary['documentary_identity_gap'])
print(summary['authority_boundary'])
print('Evidence:', output)
# A proved contradiction wins over a later incomplete episode.
sys.exit(1 if any(r['verdict'] == 'does not hold' for r in results) else
         2 if any(r['verdict'] == 'not tested' or r.get('execution') != 'complete' for r in results) else 0)
PY
