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
parser = argparse.ArgumentParser(description='Run every Break Hubu public proof kit')
parser.add_argument('--release', default='v0.2.4')
parser.add_argument('--output')
parser.add_argument('--cache')
args = parser.parse_args(sys.argv[2:])
output = Path(args.output).resolve() if args.output else Path(tempfile.mkdtemp(prefix='break-hubu-all-'))
if output.exists() and any(output.iterdir()):
    parser.error('--output must name a fresh empty directory')
output.mkdir(parents=True, exist_ok=True)
results = []
for episode in ('ep01-runaway', 'ep02-last-cent', 'ep03-crash-recovery', 'ep04-many-clients'):
    command = [str(root / episode / 'run.sh'), '--release', args.release, '--output', str(output / episode)]
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
summary = {'release': args.release, 'episodes': results,
           'documentary_identity_gap': 'Does not hold in v0.2.4: runtime/thread-bound account authority (HUB-162).',
           'authority_boundary': 'Normal bearer has broad administrative authority (HUB-32); configuration stays fixed.',
           'scope': 'Hubu public HTTP/CLI accounting with synthetic executor receipts; no provider billing or Gongbu execution.'}
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
