#!/usr/bin/env python3
"""Render a release's website card from a clean, retained run-all evidence directory.

Offline presentation only: no server calls, verdict changes, database access or hashes.
The kits and the claims they measure come from site/app/releases/claims.json; the
release and source commit come from the captured run provenance. The release notes
page (site/app/releases/<release>/page.tsx) must already exist. This writes its report
data, its analysis page wrapper, and a plain-text card.

Usage:
  python3 break-hubu/render-report.py EVIDENCE_DIR
  python3 break-hubu/render-report.py --supplement EVIDENCE_DIR   # add kits the release's card lacks
"""
import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import re

from catalog import load_catalog
from verification import Evidence

VERDICT_LABELS = {'holds': 'Holds', 'holds-with-caveat': 'Holds with caveat', 'does-not-hold': 'Does not hold', 'not-tested': 'Not tested'}
TIER_LABELS = {'verified': 'Verified', 'partially-verified': 'Partially verified', 'unverified': 'Unverified', 'does-not-hold': 'Does not hold'}
UNCOUNTED_LABELS = {'out-of-scope': 'Out of scope', 'not-applicable': 'Not applicable'}
PLACEHOLDER = 'export const report = null as ReleaseReport | null;'
REPORT_PREFIX = 'export const report: ReleaseReport = '
WRAPPER = '''import { analysisMetadata, ReleaseAnalysis } from "../../components/ReleaseAnalysis";
import { report } from "../report";

export const metadata = analysisMetadata(report.release);

export default function ReleaseAnalysisPage() {
  return <ReleaseAnalysis report={report} />;
}
'''


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


def outcomes(records):
    # Retries belong to their original logical attempt. Unresolved outcomes are never denials.
    latest = {r['label']: r for r in records if r.get('route') == '/spend/authorize'}
    allowed = sum(r.get('status') == 200 and r.get('body', {}).get('decision') == 'allow' for r in latest.values())
    denied = sum(r.get('status') == 200 and r.get('body', {}).get('decision') == 'deny' for r in latest.values())
    return allowed, denied, len(latest) - allowed - denied



def observations(episode, evidence, records):
    final = evidence.snapshot('final')
    budget = final['budget']
    consumed, remaining = (micros(budget[key]) for key in ('consumed_amount', 'remaining_amount'))
    allowed, denied, errors = outcomes(records)
    postings = len(final['transactions'])
    metrics = [{'label': 'Ledger postings', 'value': str(postings)},
               {'label': 'Consumed', 'value': usd(consumed)},
               {'label': 'Remaining', 'value': usd(remaining)}]
    if episode == 'ep01-runaway':
        summary = f'{postings} ledger postings consumed {usd(consumed)}. {denied} authorization attempts were denied; {errors} remained unresolved.'
    elif episode == 'ep02-last-cent':
        if len(final['authorizations']) != 1:
            raise ValueError('Cost comparison requires exactly one public authorization')
        receipt = final['authorizations'][0]['receipt']
        authorized, recorded, overrun = (micros(receipt[key]) for key in ('authorized_max', 'budget_charge', 'overrun_amount'))
        summary = f'{usd(recorded)} recorded against {usd(authorized)} authorized; {usd(overrun)} overrun visible. {denied} new requests were denied.'
        if final['authorizations'][0]['claim']['reconciliation_outcome'] == 'vendor_billed':
            summary += ' The public claim records owner reconciliation.'
    elif episode == 'ep03-crash-recovery':
        frozen = micros(evidence.snapshot('after-restart')['budget']['frozen_amount'])
        summary = f'{usd(frozen)} remained frozen after restart. Final accounting contains {postings} posting totaling {usd(consumed)}, with owner and retry checks recorded separately.'
    elif episode == 'ep04-many-clients':
        frozen = micros(evidence.snapshot('burst')['budget']['frozen_amount'])
        summary = f'{allowed} allowed, {denied} denied, {errors} unresolved. {usd(frozen)} reserved during the burst; {usd(consumed)} consumed after settlement.'
    else:
        return None, metrics
    return summary, metrics


def documented_gaps(root, release):
    # The same per-release known gaps the site reads (site/app/releases/assessments.json).
    path = root / 'site/app/releases/assessments.json'
    return read(path).get(release, {}).get('documented', {}) if path.exists() else {}


def challenge_status(challenge, report, documented=None):
    if challenge['status'] in UNCOUNTED_LABELS:
        return challenge['status']
    episode = report['episodes'].get(challenge.get('kit') or '')
    if episode:
        return episode['verdict']
    return 'does-not-hold' if (documented or {}).get(challenge['id']) == 'known-gap' else 'not-tested'


def claim_tier(claim_id, catalog, report, documented=None):
    """Verified when every in-scope challenge holds; any failure means the claim does not hold."""
    statuses = [challenge_status(c, report, documented) for c in catalog['challenges'] if c['claim'] == claim_id]
    in_scope = [s for s in statuses if s not in UNCOUNTED_LABELS]
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
               summary='', metrics=[])
    checks = result.get('checks', [])
    failures = sum(check['status'] != 'supported' for check in checks)
    try:
        summary, row['metrics'] = observations(episode, evidence, evidence.jsonl('requests.jsonl'))
        if summary is None:
            held = len(checks) - failures
            summary = f'{held} of {len(checks)} checks held.'
            row['metrics'] = [{'label': 'Checks held', 'value': f'{held} / {len(checks)}'}]
        row['summary'] = summary
    except (OSError, ValueError, KeyError, IndexError, TypeError):
        row['summary'] = 'The captured observations were incomplete.'
        row['execution'] = 'incomplete'
        if verdict != 'does-not-hold':
            row['verdict'] = 'not-tested'
    if failures:
        label = 'Failed' if verdict == 'does-not-hold' else 'Unconfirmed'
        row['summary'] = f'{label} checks: {failures}. ' + row['summary']
    return row


def claims_section(catalog, report, documented):
    lines = ['## Claims', '', '| Claim | Status | Challenges that hold |', '| --- | --- | --- |']
    for claim in catalog['claims']:
        tier = claim_tier(claim['id'], catalog, report, documented)
        lines.append(f'| {claim["id"]} · {claim["title"]} | {TIER_LABELS[tier["tier"]]} | {tier["held"]} of {tier["total"]} |')
    lines += ['', 'Out-of-scope and not-applicable challenges are omitted from the totals.',
              'Untested, planned and blocked challenges count as not holding,',
              'and any failed challenge means its claim does not hold.', '', '## Challenges', '',
              '| Challenge | Status | Kit |', '| --- | --- | --- |']
    for challenge in catalog['challenges']:
        status = challenge_status(challenge, report, documented)
        label = (VERDICT_LABELS | UNCOUNTED_LABELS)[status]
        if status == 'does-not-hold' and challenge.get('kit') not in report['episodes']:
            label = 'Known gap (documented)'
        if status == 'not-tested' and challenge['status'] in ('planned', 'blocked'):
            label += f' ({challenge["status"]})'
        kit = f'`{challenge["kit"]}`' if challenge.get('kit') else '—'
        lines.append(f'| {challenge["id"]} · {challenge["title"]} | {label} | {kit} |')
    return lines + ['']


def plain_card(report, catalog, documented=None):
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
             f'| Recorded | {report["recordedAt"]} |', '']
    lines += claims_section(catalog, report, documented or {})
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
              '## Reproduce', '', 'Follow the [proof-kit prerequisites](../README.md).', '',
              '```sh', 'git clone https://github.com/hacker-no-ice/hubu.git', 'cd hubu', '```', '']
    lines += ['```sh', f'git checkout {report["kitsCommit"]}',
              f'./break-hubu/run-all.sh --release {report["release"]}', '```', '']
    for supplement in report.get('supplements', []):
        selection = ','.join(supplement['episodes'])
        lines += ['```sh', f'git checkout {supplement["kitsCommit"]}',
                  f'./break-hubu/run-all.sh --release {report["release"]} --kits {selection}', '```', '']
    lines += ['Each kit runs its verifier. The kit README explains how to verify saved output again.', '']
    return '\n'.join(lines)


def load_report(path):
    text = path.read_text()
    if PLACEHOLDER in text:
        return None
    start = text.index(REPORT_PREFIX) + len(REPORT_PREFIX)
    return json.loads(text[start:text.rindex(';')])


def main():
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
    if any(run.get('build_mode', 'release') != 'release' for run in runs):
        parser.error('Publication requires a published stable release run; commit rehearsals are private qualification evidence')
    if not runs or any(run.get('kits_worktree_dirty') is not False for run in runs):
        parser.error('Publication requires captured provenance from a clean committed kits checkout')
    identities = {(run.get('release'), run.get('source_commit')) for run in runs}
    if len(identities) != 1 or None in next(iter(identities)):
        parser.error('All included episodes must test one released source with recorded provenance')
    RELEASE, SOURCE = next(iter(identities))
    catalog = load_catalog(root, RELEASE)
    if set(present) - catalog['kits'].keys():
        parser.error(f'The run includes episodes outside the {RELEASE} catalog')
    if not re.fullmatch(r'v\d+\.\d+\.\d+', RELEASE) or not re.fullmatch('[0-9a-f]{40}', SOURCE):
        parser.error('Captured release identity is malformed')
    if any(run.get('binary_versions', {}).get(binary, {}).get('source_commit') != SOURCE
           or run.get('binary_versions', {}).get(binary, {}).get('product_version') != RELEASE
           for run in runs for binary in ('hubu', 'hubu-server')):
        parser.error('Captured CLI and server versions must match the released source')
    if len({run['kits_commit'] for run in runs}) != 1 or not re.fullmatch('[0-9a-f]{40}', runs[0]['kits_commit']):
        parser.error('All included episodes must use one exact kits commit')
    release_dir = root / 'site/app/releases' / RELEASE
    if not (release_dir / 'page.tsx').is_file():
        parser.error(f'Write the {RELEASE} release notes page first: {release_dir / "page.tsx"}')
    report_path = release_dir / 'report.ts'
    try:
        existing = load_report(report_path) if report_path.exists() else None
    except ValueError:
        parser.error(f'{report_path} already holds a measured report that this renderer cannot read; preserve prior publication')
    card_path = root / 'break-hubu/cards' / (RELEASE + '.md')
    recorded = min(datetime.fromisoformat(run['started_at'].replace('Z', '+00:00')) for run in runs)
    recorded = recorded.astimezone(timezone.utc).date().isoformat() + ' UTC'
    if args.supplement:
        if existing is None:
            parser.error(f'{RELEASE} has no measured card to supplement; render without --supplement')
        if (existing['release'], existing['sourceCommit']) != (RELEASE, SOURCE):
            parser.error('Supplement evidence tests a different released source than the published card')
        overlap = sorted(set(present) & set(existing['episodes']))
        if overlap:
            parser.error('Supplements never replace published results; already measured: ' + ', '.join(overlap))
        report = existing
        report.setdefault('supplements', []).append(dict(kitsCommit=runs[0]['kits_commit'], recordedAt=recorded, episodes=present))
    else:
        if existing is not None:
            parser.error(f'{report_path} already holds a measured report; use --supplement to add kits it lacks')
        if card_path.exists():
            parser.error(f'{RELEASE} already has a published card; preserve prior publication')
        report = dict(release=RELEASE, sourceCommit=SOURCE, kitsCommit=runs[0]['kits_commit'],
                      recordedAt=recorded, episodes={})
    for episode in present:
        report['episodes'][episode] = episode_report(args.evidence / episode, episode, catalog['kits'][episode]['boundary'])
    report['episodes'] = {episode: report['episodes'][episode] for episode in catalog['kits'] if episode in report['episodes']}
    source = 'import type { ReleaseReport } from "../report-types";\nexport type { EpisodeId, EpisodeResult, Verdict } from "../report-types";\n\n'
    source += '// Generated from proof-kit results by break-hubu/render-report.py.\n'
    source += REPORT_PREFIX + json.dumps(report, ensure_ascii=False, indent=2) + ';\n'
    report_path.write_text(source)
    wrapper = release_dir / 'report/page.tsx'
    if not wrapper.exists():
        wrapper.parent.mkdir(exist_ok=True)
        wrapper.write_text(WRAPPER)
    documented = documented_gaps(root, RELEASE)
    card_path.write_text(plain_card(report, catalog, documented))
    card = {'release': RELEASE,
            'claims': {claim['id']: claim_tier(claim['id'], catalog, report, documented) for claim in catalog['claims']},
            'verdicts': {episode: row['verdict'] for episode, row in report['episodes'].items()}}
    (root / 'break-hubu/cards' / (RELEASE + '.json')).write_text(json.dumps(card, indent=2) + '\n')
    print(('Supplemented' if args.supplement else 'Prepared') + ' website report:', report_path)


if __name__ == '__main__':
    main()
