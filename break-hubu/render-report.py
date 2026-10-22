#!/usr/bin/env python3
"""Render the v0.2.4 website card from a clean, retained run-all evidence directory.

Offline presentation only: no server calls, verdict changes, database access or hashes.
Usage: python3 break-hubu/render-report.py /tmp/break-hubu-public-v024
"""
import argparse
from datetime import datetime
import json
from pathlib import Path
import re
import shutil
import tempfile
import zipfile

from verification import Evidence

EPISODES = ('ep01-runaway', 'ep02-last-cent', 'ep03-crash-recovery', 'ep04-many-clients')
RELEASE = 'v0.2.4'
SOURCE = '5344ab565171e1aa3d3ec628641cdbfce18372af'
BASE = '/break-hubu/evidence/' + RELEASE
CAVEATS = (
    'The budget and policy stay fixed throughout. These results do not establish authority boundaries for a caller with the broad administrative bearer.',
    'Full overrun accounting requires explicit owner reconciliation after lease expiry. A negative remaining budget correctly records the confirmed charge above the estimate.',
    'The fault is a Hubu process kill after a confirmed claim and synthetic billed marker, before settlement. Provider execution, disk loss, and Gongbu recovery are outside this test.',
    'Ten client processes create the network burst. The released server handles connections serially; this does not measure simultaneous SQLite writers, throughput, or fairness.',
)


def read(path):
    return json.loads(path.read_text())


def micros(value):
    amount = Evidence.money(value) * 1_000_000
    if amount.denominator != 1:
        raise ValueError('Plot would lose monetary precision')
    return int(amount)


def usd(value):
    sign = '−' if value < 0 else ''
    whole, fraction = divmod(abs(value), 1_000_000)
    return f'{sign}${whole}.{str(fraction).zfill(6).rstrip("0").ljust(2, "0")}'


def link(episode, name, label):
    return {'label': label, 'href': f'{BASE}/{episode}/{name}'}


def outcomes(records):
    # Retries belong to their original logical attempt. Unresolved outcomes are never denials.
    latest = {r['label']: r for r in records if r.get('route') == '/spend/authorize'}
    allowed = sum(r.get('status') == 200 and r.get('body', {}).get('decision') == 'allow' for r in latest.values())
    denied = sum(r.get('status') == 200 and r.get('body', {}).get('decision') == 'deny' for r in latest.values())
    return allowed, denied, len(latest) - allowed - denied


def observations(episode, evidence, records):
    initial, final = evidence.snapshot('initial'), evidence.snapshot('final')
    budget = final['budget']
    consumed, remaining = (micros(budget[key]) for key in ('consumed_amount', 'remaining_amount'))
    allowed, denied, errors = outcomes(records)
    postings = len(final['transactions'])
    metrics = [{'label': 'Ledger postings', 'value': str(postings)},
               {'label': 'Consumed', 'value': usd(consumed)},
               {'label': 'Remaining', 'value': usd(remaining)}]
    trace = link(episode, 'requests.jsonl', 'Captured request trace')
    if episode == EPISODES[0]:
        points = [{'attempt': 0, 'consumedMicrousd': micros(initial['budget']['consumed_amount'])}]
        seen, settled = set(), set()
        for record in records:
            if record.get('route') == '/spend/authorize':
                seen.add(record['label'])
            body = record.get('body') or {}
            settlement = body.get('settlement_id')
            if record.get('route') == '/spend/executor/settle' and record.get('status') == 200 and settlement not in settled:
                points.append({'attempt': len(seen), 'consumedMicrousd': micros(body['spend']['budget_hold']['consumed_amount'])})
                settled.add(settlement)
        if points[-1]['attempt'] != len(seen):
            points.append({'attempt': len(seen), 'consumedMicrousd': consumed})
        visual = dict(kind='consumption', budgetMicrousd=micros(initial['budget']['amount_limit']), points=points, denied=denied, source=trace)
        summary = f'{postings} ledger postings consumed {usd(consumed)}. {denied} authorization attempts were denied; {errors} remained unresolved.'
    elif episode == EPISODES[1]:
        if len(final['authorizations']) != 1:
            raise ValueError('Cost comparison requires exactly one public authorization')
        receipt = final['authorizations'][0]['receipt']
        authorized, recorded, overrun = (micros(receipt[key]) for key in ('authorized_max', 'budget_charge', 'overrun_amount'))
        visual = dict(kind='costs', authorizedMicrousd=authorized, recordedMicrousd=recorded, overrunMicrousd=overrun,
                      reconciled=final['authorizations'][0]['claim']['reconciliation_outcome'] == 'vendor_billed',
                      source=link(episode, 'snapshots/final/authorizations-001.json', 'Public authorization and receipt'))
        summary = f'{usd(recorded)} recorded against {usd(authorized)} authorized; {usd(overrun)} overrun visible. {denied} new requests were denied.'
        if visual['reconciled']:
            summary += ' The public claim records owner reconciliation.'
    elif episode == EPISODES[2]:
        claim = evidence.snapshot('before-crash')['authorizations'][0]['claim']
        start = datetime.fromisoformat(claim['claimed_at'])
        events = [(claim['claimed_at'], 'Claim confirmed', 'Public claim state before the crash.')]
        for event in evidence.jsonl('process-events.jsonl'):
            if datetime.fromisoformat(event['at']) < start:
                continue
            if event['event'] == 'kill':
                detail = f'{event["signal"]}; process exit {event["exit_code"]}.'
            elif event['event'] == 'fixture_saved':
                detail = 'Durable synthetic receipt; no provider call.'
            else:
                detail = 'Same released binary and database.'
            events.append((event['at'], {'kill': 'Hubu process killed', 'restart': 'Hubu restarted', 'fixture_saved': 'Fixture saved'}[event['event']], detail))
        for record in records:
            if record['label'] in ('snapshot-after-restart-budgets', 'snapshot-expired-budgets', 'snapshot-final-budgets'):
                state = record['body']['budgets'][0]
                detail = f'Frozen {usd(micros(state["frozen_amount"]))}; consumed {usd(micros(state["consumed_amount"]))}.'
                events.append((record['ended_at'], {'snapshot-after-restart-budgets': 'Public state after restart', 'snapshot-expired-budgets': 'Public state after lease expiry', 'snapshot-final-budgets': 'Final public state'}[record['label']], detail))
            elif record['type'] == 'owner_cli':
                events.append((record['at'], 'Owner reconciliation' if record['label'] == 'owner-first' else 'Identical owner retry', f'CLI exit {record["exit_code"]}.'))
        visual = dict(kind='timeline', events=[dict(label=label, elapsedSeconds=(datetime.fromisoformat(at) - start).total_seconds(), detail=detail) for at, label, detail in sorted(events)],
                      source={'label': 'Process events and public state exports (ZIP)', 'href': BASE + '/evidence.zip'})
        frozen = micros(evidence.snapshot('after-restart')['budget']['frozen_amount'])
        summary = f'{usd(frozen)} remained frozen after restart. Final accounting contains {postings} posting totaling {usd(consumed)}, with owner and retry checks recorded separately.'
    else:
        frozen = micros(evidence.snapshot('burst')['budget']['frozen_amount'])
        visual = dict(kind='burst', allowed=allowed, denied=denied, errors=errors, reservedMicrousd=frozen,
                      consumedMicrousd=consumed, remainingMicrousd=remaining, source=trace)
        summary = f'{allowed} allowed, {denied} denied, {errors} unresolved. {usd(frozen)} reserved during the burst; {usd(consumed)} consumed after settlement.'
    return summary, metrics, visual


def episode_report(directory, episode, caveat):
    evidence = Evidence(directory)
    result = read(directory / 'result.json') if (directory / 'result.json').exists() else {'verdict': 'not tested', 'execution': 'incomplete'}
    verdict = result['verdict'].replace(' ', '-')
    if verdict not in ('holds', 'holds-with-caveat', 'does-not-hold', 'not-tested'):
        raise ValueError('Unknown claim verdict')
    row = dict(verdict=verdict, execution=result.get('execution', 'incomplete'), caveat=caveat,
               summary='', metrics=[], visual=None, terminal=None, evidence=[])
    failures = [check['name'] for check in result.get('checks', []) if check['status'] != 'supported']
    try:
        row['summary'], row['metrics'], row['visual'] = observations(episode, evidence, evidence.jsonl('requests.jsonl'))
    except (OSError, ValueError, KeyError, IndexError, TypeError) as error:
        row['summary'] = 'Report observations incomplete: ' + str(error) + '.'
        row['execution'] = 'incomplete'
        if verdict != 'does-not-hold':
            row['verdict'] = 'not-tested'
    if failures:
        row['summary'] = '; '.join(failures[:2]) + '. ' + row['summary']
    transcript = directory / 'transcript.txt'
    if transcript.exists():
        excerpts = re.findall(r'^\[[^\]]+\] \$ [^\n]+ budget list\n.*?^CLI exit=\d+\n', transcript.read_text(), re.M | re.S)
        row['terminal'] = excerpts[-1] if excerpts else None
    for name, label in [('transcript.txt', 'Full transcript'), ('requests.jsonl', 'Request trace'), ('result.json', 'Verification result'),
                        ('snapshots/final/ledger-001.json', 'Final ledger (page 1)'), ('snapshots/final/budgets.json', 'Final budget'), ('run.json', 'Run provenance')]:
        if (directory / name).exists():
            row['evidence'].append(link(episode, name, label))
    return row


def public_files(directory):
    # Never recurse/copy a runtime wholesale. Private database/config/credential files are excluded.
    names = ('run.json', 'result.json', 'verification.txt', 'transcript.txt', 'requests.jsonl', 'process-events.jsonl', 'executor-guidance.json', 'clients.jsonl', 'workload.json')
    paths = [directory / name for name in names if (directory / name).is_file()]
    paths += [path for path in (directory / 'snapshots').glob('*/*') if re.fullmatch(r'(?:budgets\.json|budget\.txt|policy\.txt|(?:ledger|authorizations)-\d{3}\.json)', path.name)]
    paths += list((directory / 'fixtures').glob('*-receipt.json'))
    for path in paths:
        if path.is_symlink() or not path.is_file():
            raise ValueError('Evidence must be ordinary files: ' + str(path))
    return paths


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('evidence', type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    runs = [read(args.evidence / episode / 'run.json') for episode in EPISODES if (args.evidence / episode / 'run.json').exists()]
    if not runs or any(run.get('kits_worktree_dirty') is not False for run in runs):
        parser.error('Publication requires captured provenance from a clean committed kits checkout')
    if any(run['release'] != RELEASE or run['source_commit'] != SOURCE for run in runs):
        parser.error('This renderer is for unchanged v0.2.4 only')
    if any(run.get('binary_versions', {}).get(binary, {}).get('source_commit') != SOURCE
           or run.get('binary_versions', {}).get(binary, {}).get('product_version') != RELEASE
           for run in runs for binary in ('hubu', 'hubu-server')):
        parser.error('Captured CLI and server versions must match the released source')
    if len({run['kits_commit'] for run in runs}) != 1 or not re.fullmatch('[0-9a-f]{40}', runs[0]['kits_commit']):
        parser.error('All included episodes must use one exact kits commit')
    report = dict(release=RELEASE, sourceCommit=SOURCE, kitsCommit=runs[0]['kits_commit'],
                  recordedAt=min(run['started_at'] for run in runs).split('T')[0] + ' UTC',
                  host=', '.join(sorted({run['platform'] for run in runs})),
                  evidenceBundle={'label': 'Download all evidence (ZIP)', 'href': BASE + '/evidence.zip'}, episodes={})
    destination = root / 'site/public' / BASE.lstrip('/')
    if destination.exists():
        parser.error('Evidence destination already exists; preserve prior publication and choose an explicit replacement outside this script')
    with tempfile.TemporaryDirectory(prefix='break-hubu-report-') as temporary:
        staging = Path(temporary)
        for episode, caveat in zip(EPISODES, CAVEATS):
            directory = args.evidence / episode
            report['episodes'][episode] = episode_report(directory, episode, caveat)
            for source in public_files(directory):
                target = staging / episode / source.relative_to(directory)
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(source, target)
        if (args.evidence / 'summary.json').exists():
            shutil.copyfile(args.evidence / 'summary.json', staging / 'summary.json')
        with zipfile.ZipFile(staging / 'evidence.zip', 'w', zipfile.ZIP_DEFLATED) as archive:
            for path in sorted(staging.rglob('*')):
                if path.is_file() and path.name != 'evidence.zip':
                    archive.write(path, str(path.relative_to(staging)))
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copytree(staging, destination)
    source = 'import type { ReleaseReport } from "./types";\nexport type { EpisodeId, EpisodeResult, EvidenceVisual, Verdict } from "./types";\n\n'
    source += '// Generated from captured public evidence by break-hubu/render-report.py.\n'
    source += 'export const report: ReleaseReport = ' + json.dumps(report, ensure_ascii=False, indent=2) + ';\n'
    (root / 'site/app/break-hubu/v0.2.4/report.ts').write_text(source)
    print('Prepared website report and evidence:', destination)


if __name__ == '__main__':
    main()
