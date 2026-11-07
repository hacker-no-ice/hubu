#!/usr/bin/env python3
"""Render a release's website card from a clean, retained run-all evidence directory.

Offline presentation only: no server calls, verdict changes, database access or hashes.
The kits and the claims they measure come from site/app/releases/claims.json; the
release and source commit come from the captured run provenance. The release notes
page (site/app/releases/<release>/page.tsx) must already exist. This writes its report
data, its analysis page wrapper, an evidence bundle and archive, and a plain-text card.

Usage:
  python3 break-hubu/render-report.py EVIDENCE_DIR
  python3 break-hubu/render-report.py --supplement EVIDENCE_DIR   # add kits the release's card lacks
"""
import argparse
from datetime import datetime
import io
import json
from pathlib import Path
import re
import zipfile

from verification import Evidence

VERDICT_LABELS = {'holds': 'Holds', 'holds-with-caveat': 'Holds with caveat', 'does-not-hold': 'Does not hold', 'not-tested': 'Not tested'}
TIER_LABELS = {'verified': 'Verified', 'partially-verified': 'Partially verified', 'unverified': 'Unverified', 'does-not-hold': 'Does not hold'}
PLACEHOLDER = 'export const report = null as ReleaseReport | null;'
REPORT_PREFIX = 'export const report: ReleaseReport = '
WRAPPER = '''import { analysisMetadata, ReleaseAnalysis } from "../../components/ReleaseAnalysis";
import { report } from "../report";

export const metadata = analysisMetadata(report.release);

export default function ReleaseAnalysisPage() {
  return <ReleaseAnalysis report={report} />;
}
'''
BUNDLE_SCHEMA = 'hubu-proof-evidence-bundle-v1'
ZIP_TIME = (1980, 1, 1, 0, 0, 0)
BASE = None  # Set per release in main(): /break-hubu/evidence/<release>


def read(path):
    return json.loads(path.read_text())


def public_summary(path, release):
    summary = read(path)
    # These are runner annotations, not Hubu protocol evidence or measured outcomes.
    summary['documentary_identity_gap'] = f'Does not hold in {release}: runtime/thread-bound account authority is a known gap that will be addressed in future releases.'
    summary['authority_boundary'] = 'Normal bearer has broad administrative authority; configuration stays fixed. More narrowly scoped executor authority is planned for future releases.'
    return summary


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
    if episode == 'ep01-runaway':
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
    elif episode == 'ep02-last-cent':
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
    elif episode == 'ep03-crash-recovery':
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
    elif episode == 'ep04-many-clients':
        frozen = micros(evidence.snapshot('burst')['budget']['frozen_amount'])
        visual = dict(kind='burst', allowed=allowed, denied=denied, errors=errors, reservedMicrousd=frozen,
                      consumedMicrousd=consumed, remainingMicrousd=remaining, source=trace)
        summary = f'{allowed} allowed, {denied} denied, {errors} unresolved. {usd(frozen)} reserved during the burst; {usd(consumed)} consumed after settlement.'
    else:
        return None, metrics, None
    return summary, metrics, visual




def load_catalog(root):
    return read(root / 'site/app/releases/claims.json')


def challenge_status(challenge, report):
    if challenge['status'] == 'out-of-scope':
        return 'out-of-scope'
    episode = report['episodes'].get(challenge.get('kit') or '')
    return episode['verdict'] if episode else 'not-tested'


def claim_tier(claim_id, catalog, report):
    """Verified when every in-scope challenge holds; any failure means the claim does not hold."""
    statuses = [challenge_status(c, report) for c in catalog['challenges'] if c['claim'] == claim_id]
    in_scope = [s for s in statuses if s != 'out-of-scope']
    held = sum(s in ('holds', 'holds-with-caveat') for s in in_scope)
    if 'does-not-hold' in in_scope:
        tier = 'does-not-hold'
    elif in_scope and held == len(in_scope):
        tier = 'verified'
    else:
        tier = 'partially-verified' if held else 'unverified'
    return dict(tier=tier, held=held, total=len(in_scope), failed=in_scope.count('does-not-hold'))


def episode_report(directory, episode, caveat):
    evidence = Evidence(directory)
    result = read(directory / 'result.json') if (directory / 'result.json').exists() else {'verdict': 'not tested', 'execution': 'incomplete'}
    verdict = result['verdict'].replace(' ', '-')
    if verdict not in ('holds', 'holds-with-caveat', 'does-not-hold', 'not-tested'):
        raise ValueError('Unknown claim verdict')
    row = dict(verdict=verdict, execution=result.get('execution', 'incomplete'), caveat=caveat,
               summary='', metrics=[], visual=None, terminal=None, evidence=[])
    checks = result.get('checks', [])
    failures = [check['name'] for check in checks if check['status'] != 'supported']
    try:
        summary, row['metrics'], row['visual'] = observations(episode, evidence, evidence.jsonl('requests.jsonl'))
        if summary is None:
            held = len(checks) - len(failures)
            summary = f'{held} of {len(checks)} checks held.'
            row['metrics'] = [{'label': 'Checks held', 'value': f'{held} / {len(checks)}'}]
        row['summary'] = summary
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
    names = ('run.json', 'result.json', 'verification.txt', 'transcript.txt', 'requests.jsonl', 'process-events.jsonl',
             'executor-guidance.json', 'clients.jsonl', 'workload.json', 'victim.json')
    paths = [directory / name for name in names if (directory / name).is_file()]
    paths += [path for path in (directory / 'snapshots').glob('*/*') if re.fullmatch(r'(?:budgets\.json|budget\.txt|policy\.txt|(?:ledger|authorizations)-\d{3}\.json)', path.name)]
    paths += list((directory / 'fixtures').glob('*.json'))
    for path in paths:
        if path.is_symlink() or not path.is_file():
            raise ValueError('Evidence must be ordinary files: ' + str(path))
    return paths


def read_bundle(path):
    if not path.exists():
        return {}
    bundle = read(path)
    if bundle.get('schema') != BUNDLE_SCHEMA:
        raise ValueError('Unsupported evidence bundle schema')
    return bundle['files']


def write_store(store, release, files):
    # One text bundle the site build unpacks to the public evidence URLs, and one
    # deterministic archive for download. Neither duplicates the other in git history.
    store.mkdir(parents=True, exist_ok=True)
    ordered = dict(sorted(files.items()))
    (store / 'bundle.json').write_text(json.dumps({'schema': BUNDLE_SCHEMA, 'release': release, 'files': ordered},
                                                  ensure_ascii=False, indent=1) + '\n')
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, 'w', zipfile.ZIP_DEFLATED) as archive:
        for name, text in ordered.items():
            info = zipfile.ZipInfo(name, ZIP_TIME); info.compress_type = zipfile.ZIP_DEFLATED; info.external_attr = 0o644 << 16
            archive.writestr(info, text)
    (store / 'evidence.zip').write_bytes(buffer.getvalue())


def collect(evidence_dir, episode):
    directory = evidence_dir / episode
    files = {}
    for source in public_files(directory):
        try:
            files[f'{episode}/{source.relative_to(directory).as_posix()}'] = source.read_text(encoding='utf-8')
        except UnicodeDecodeError as error:
            raise ValueError(f'Evidence must be UTF-8 text: {source}') from error
    return files


def claims_section(catalog, report):
    lines = ['## Claims', '', '| Claim | Status | Challenges that hold |', '| --- | --- | --- |']
    for claim in catalog['claims']:
        tier = claim_tier(claim['id'], catalog, report)
        lines.append(f'| {claim["id"]} · {claim["title"]} | {TIER_LABELS[tier["tier"]]} | {tier["held"]} of {tier["total"]} |')
    lines += ['', 'Out-of-scope challenges are not counted. Untested, planned and blocked challenges count as not holding,',
              'and any failed challenge means its claim does not hold.', '', '## Challenges', '',
              '| Challenge | Status | Kit |', '| --- | --- | --- |']
    for challenge in catalog['challenges']:
        status = challenge_status(challenge, report)
        label = 'Out of scope' if status == 'out-of-scope' else VERDICT_LABELS[status]
        if status == 'not-tested' and challenge['status'] in ('planned', 'blocked'):
            label += f' ({challenge["status"]})'
        kit = f'`{challenge["kit"]}`' if challenge.get('kit') else '—'
        lines.append(f'| {challenge["id"]} · {challenge["title"]} | {label} | {kit} |')
    return lines + ['']


def plain_card(report, catalog):
    kits = [episode for episode in catalog['kits'] if episode in report['episodes']]
    revisions = [report['kitsCommit']] + [s['kitsCommit'] for s in report.get('supplements', [])]
    lines = [f'# {report["release"]} system analysis', '',
             '**All claims are about Hubu\'s own ledger and records, not provider invoices.**', '',
             'Generated by `break-hubu/render-report.py` from clean runs of the public proof kits.',
             'The website analysis is the presentation of record; this file keeps the same',
             'results readable in the repository.', '',
             '| | |', '| --- | --- |',
             f'| Release | `{report["release"]}` |',
             f'| Released source | `{report["sourceCommit"]}` |',
             f'| Kits revisions | {", ".join(f"`{r}`" for r in revisions)} |',
             f'| Recorded | {report["recordedAt"]} |',
             f'| Host | `{report["host"]}` |', '']
    lines += claims_section(catalog, report)
    lines += ['## Experiments', '', '| Experiment | Verdict | Observed |', '| --- | --- | --- |']
    for episode in kits:
        row = report['episodes'][episode]
        lines.append(f'| {catalog["kits"][episode]["title"]} (`{episode}`) | {VERDICT_LABELS[row["verdict"]]} | {row["summary"]} |')
    lines += ['', '## Boundaries', '']
    lines += [f'- **{catalog["kits"][episode]["title"]}:** {report["episodes"][episode]["caveat"]}' for episode in kits]
    lines += ['', '## Known limits', '',
              '- Fixture receipts control cost and timing. They do not establish real invoices, truthful',
              '  executors, provider-side exactly-once execution or task quality. Gongbu, Temporal, MCP',
              '  transport, provider adapters and actual provider billing are outside these kits.', '',
              '## Reproduce', '', '```sh', 'git clone https://github.com/hacker-no-ice/hubu.git', 'cd hubu',
              f'git checkout {revisions[-1]}', f'./break-hubu/run-all.sh --release {report["release"]}', '```', '',
              f'Evidence: `site/evidence/{report["release"]}/` in the repository and `{BASE}/evidence.zip` on hubustack.dev.', '']
    return '\n'.join(lines)


def load_report(path):
    text = path.read_text()
    if PLACEHOLDER in text:
        return None
    start = text.index(REPORT_PREFIX) + len(REPORT_PREFIX)
    return json.loads(text[start:text.rindex(';')])


def main():
    global BASE
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('evidence', type=Path)
    parser.add_argument('--supplement', action='store_true',
                        help='add kits that the release\'s measured card does not include yet; never replaces a result')
    parser.add_argument('--root', type=Path, default=Path(__file__).resolve().parents[1], help=argparse.SUPPRESS)
    args = parser.parse_args()
    root = args.root.resolve()
    catalog = load_catalog(root)
    present = [episode for episode in catalog['kits'] if (args.evidence / episode / 'run.json').exists()]
    runs = [read(args.evidence / episode / 'run.json') for episode in present]
    if not runs or any(run.get('kits_worktree_dirty') is not False for run in runs):
        parser.error('Publication requires captured provenance from a clean committed kits checkout')
    identities = {(run.get('release'), run.get('source_commit')) for run in runs}
    if len(identities) != 1 or None in next(iter(identities)):
        parser.error('All included episodes must test one released source with recorded provenance')
    RELEASE, SOURCE = next(iter(identities))
    if not re.fullmatch(r'v\d+\.\d+\.\d+', RELEASE) or not re.fullmatch('[0-9a-f]{40}', SOURCE):
        parser.error('Captured release identity is malformed')
    if any(run.get('binary_versions', {}).get(binary, {}).get('source_commit') != SOURCE
           or run.get('binary_versions', {}).get(binary, {}).get('product_version') != RELEASE
           for run in runs for binary in ('hubu', 'hubu-server')):
        parser.error('Captured CLI and server versions must match the released source')
    if len({run['kits_commit'] for run in runs}) != 1 or not re.fullmatch('[0-9a-f]{40}', runs[0]['kits_commit']):
        parser.error('All included episodes must use one exact kits commit')
    BASE = '/break-hubu/evidence/' + RELEASE
    release_dir = root / 'site/app/releases' / RELEASE
    if not (release_dir / 'page.tsx').is_file():
        parser.error(f'Write the {RELEASE} release notes page first: {release_dir / "page.tsx"}')
    report_path = release_dir / 'report.ts'
    try:
        existing = load_report(report_path) if report_path.exists() else None
    except ValueError:
        parser.error(f'{report_path} already holds a measured report that this renderer cannot read; preserve prior publication')
    store = root / 'site/evidence' / RELEASE
    card_path = root / 'break-hubu/cards' / (RELEASE + '.md')
    recorded = min(run['started_at'] for run in runs).split('T')[0] + ' UTC'
    host = ', '.join(sorted({run['platform'] for run in runs}))
    if args.supplement:
        if existing is None:
            parser.error(f'{RELEASE} has no measured card to supplement; render without --supplement')
        if (existing['release'], existing['sourceCommit']) != (RELEASE, SOURCE):
            parser.error('Supplement evidence tests a different released source than the published card')
        overlap = sorted(set(present) & set(existing['episodes']))
        if overlap:
            parser.error('Supplements never replace published results; already measured: ' + ', '.join(overlap))
        report = existing
        report.setdefault('supplements', []).append(dict(kitsCommit=runs[0]['kits_commit'], recordedAt=recorded, host=host, episodes=present))
        files = read_bundle(store / 'bundle.json')
    else:
        if existing is not None:
            parser.error(f'{report_path} already holds a measured report; use --supplement to add kits it lacks')
        if card_path.exists() or store.exists():
            parser.error(f'{RELEASE} already has a published card or evidence; preserve prior publication')
        report = dict(release=RELEASE, sourceCommit=SOURCE, kitsCommit=runs[0]['kits_commit'], recordedAt=recorded, host=host,
                      evidenceBundle={'label': 'Download all evidence (ZIP)', 'href': BASE + '/evidence.zip'}, episodes={})
        files = {}
    for episode in present:
        report['episodes'][episode] = episode_report(args.evidence / episode, episode, catalog['kits'][episode]['boundary'])
        additions = collect(args.evidence, episode)
        if set(additions) & set(files):
            parser.error(f'Evidence for {episode} already exists in the {RELEASE} bundle')
        files.update(additions)
    summary = args.evidence / 'summary.json'
    if summary.exists():
        name = 'summary.json' if not args.supplement else f'summary-{runs[0]["kits_commit"][:12]}.json'
        files[name] = json.dumps(public_summary(summary, RELEASE), indent=2) + '\n'
    report['episodes'] = {episode: report['episodes'][episode] for episode in catalog['kits'] if episode in report['episodes']}
    write_store(store, RELEASE, files)
    source = 'import type { ReleaseReport } from "../report-types";\nexport type { EpisodeId, EpisodeResult, EvidenceVisual, Verdict } from "../report-types";\n\n'
    source += '// Generated from captured public evidence by break-hubu/render-report.py.\n'
    source += REPORT_PREFIX + json.dumps(report, ensure_ascii=False, indent=2) + ';\n'
    report_path.write_text(source)
    wrapper = release_dir / 'report/page.tsx'
    if not wrapper.exists():
        wrapper.parent.mkdir(exist_ok=True)
        wrapper.write_text(WRAPPER)
    card_path.write_text(plain_card(report, catalog))
    card = {'release': RELEASE,
            'claims': {claim['id']: claim_tier(claim['id'], catalog, report) for claim in catalog['claims']},
            'verdicts': {episode: row['verdict'] for episode, row in report['episodes'].items()}}
    (root / 'break-hubu/cards' / (RELEASE + '.json')).write_text(json.dumps(card, indent=2) + '\n')
    print(('Supplemented' if args.supplement else 'Prepared') + ' website report and evidence:', store)


if __name__ == '__main__':
    main()
