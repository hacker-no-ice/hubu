"""Verify the 100-operation burst and public accounting, without private state."""
from datetime import datetime
from fractions import Fraction
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from verification import Evidence, budget_denial


def verify(output):
    e = Evidence(output)
    def client_evidence():
        workload = e.json('workload.json')
        events = e.jsonl('clients.jsonl')
        starts = [r for r in events if r['event'] == 'start']
        finishes = [r for r in events if r['event'] == 'finish']
        valid = (workload['client_processes'] == workload['attempts_per_client'] == 10
                 and len(starts) == len(finishes) == len(set(workload['ready_pids'])) == 10
                 and {r['client'] for r in starts} == {r['client'] for r in finishes} == set(range(1, 11))
                 and {r['pid'] for r in starts} == {r['pid'] for r in finishes} == set(workload['ready_pids']))
        if not valid:
            raise ValueError('Incomplete ten-client start/finish/barrier evidence')
        for start in starts:
            finish = next(r for r in finishes if r['client'] == start['client'])
            if (start['pid'] != finish['pid'] or finish['error'] is not None
                    or not workload['barrier_released_at'] <= start['at'] <= finish['at']):
                raise ValueError('Client did not complete the declared barrier workload')
            for attempt in range(1, 11):
                for row in e.requests(f"client-{start['client']:02}-{attempt:02}"):
                    if not start['at'] <= row['started_at'] <= row['ended_at'] <= finish['at']:
                        raise ValueError('Request lies outside its client lifecycle')
        return True
    def observed_admissions():
        # Preserve a proved over-admission even when another response/export is missing.
        # Count replayed decisions once and require the observed allows to predate
        # every returned authorization expiry (no expired capacity being reused).
        import json
        rows, writes = [], []
        for line in (e.output / 'requests.jsonl').read_text().splitlines():
            try:
                row = json.loads(line)
                if row.get('route') in ('/spend/executor/settle', '/spend/executor/release'):
                    writes.append(row)
                if (row.get('route') == '/spend/authorize' and row.get('status') == 200
                        and row.get('body', {}).get('decision') == 'allow'):
                    rows.append(row)
            except (ValueError, TypeError):
                continue  # Complete trace checks below still mark this run incomplete.
        if not rows:
            raise ValueError('No observed admissions')
        last_admission = max(datetime.fromisoformat(r['ended_at']) for r in rows)
        if any(e.money(r['body']['budget_hold']['frozen_amount']) > 1 for r in rows):
            return False  # A returned above-budget obligation is direct evidence.
        if min(datetime.fromisoformat(r['body']['authorization_expires_at']) for r in rows) <= last_admission:
            raise ValueError('Observed admission interval includes authorization expiry')
        if any(datetime.fromisoformat(r['started_at']) <= last_admission for r in writes):
            raise ValueError('Settlement/release overlapped the admission interval')
        return len({r['body']['decision_id'] for r in rows}) <= 33

    def intact_workload(phase='burst'):
        auths = e.snapshot(phase)['authorizations']
        if any(a['status'] in ('expired', 'reconciliation_required') for a in auths):
            raise ValueError('Lease expiry disrupted the bounded burst')
        return True

    def outcomes():
        rows = [e.requests(f'client-{client:02}-{attempt:02}')[-1] for client in range(1, 11) for attempt in range(1, 11)]
        if any(r['status'] != 200 or r['body'].get('decision') not in ('allow', 'deny') for r in rows):
            raise ValueError('Unresolved request outcome; it is not a denial')
        return rows
    def claimed_workload():
        admitted = [r for r in outcomes() if r['body']['decision'] == 'allow']
        claims = []
        for admission in admitted:
            label = admission['label']
            for suffix in ('-claim-resolve', '-claim'):
                row = e.requests(label + suffix)[-1]
                if row['status'] != 200:
                    raise ValueError('Admitted claim did not complete successfully: ' + label + suffix)
            claims.append((admission, row))
        return claims

    def settled_workload():
        claims = claimed_workload()
        for admission, claim in claims:
            row = e.requests(admission['label'] + '-settle')[-1]
            if row['status'] != 200 or row['body'].get('status') != 'settled':
                raise ValueError('Normal settlement did not complete successfully: ' + admission['label'])
        return True

    def claim_mapping():
        public = {a['id']: a for a in e.snapshot('burst')['authorizations']}
        return all(claim['body']['spend']['decision_id'] == admission['body']['decision_id']
                   and claim['body']['claim_id'] == public[admission['body']['decision_id']]['claim']['id']
                   and claim['body']['spend']['budget_hold']['hold_id'] == public[admission['body']['decision_id']]['budget_hold']['id']
                   for admission, claim in claimed_workload())

    def settlement_mapping():
        settled_workload()
        public = {a['id']: a for a in e.snapshot('final')['authorizations']}
        return all(e.requests(admission['label'] + '-settle')[-1]['body']['claim_id'] == claim['body']['claim_id']
                   and e.requests(admission['label'] + '-settle')[-1]['body']['settlement_id']
                   == public[admission['body']['decision_id']]['receipt']['settlement_id']
                   for admission, claim in claimed_workload())

    def complete_burst():
        intact_workload()
        claimed_workload()
        rows = outcomes()
        return (sum(r['body'].get('decision') == 'allow' for r in rows) == 33
                and sum(budget_denial(r) for r in rows) == 67)
    e.check('Ten paired client lifecycles contain their requests after the barrier', client_evidence)
    e.check('No more than 33 observed admissions before expiry (USD 0.99)', observed_admissions)
    e.check('100 resolved attempts: 33 allows and 67 budget denials', complete_burst)
    e.check('Fresh USD 1.00 budget', lambda: e.final('initial', count=0, consumed='0', frozen='0', remaining='1', pending=0))
    e.check('Burst: USD 0.99 held, USD 0.01 available, zero postings', lambda:
            claimed_workload() and intact_workload() and e.final('burst', count=0, consumed='0', frozen='0.99', remaining='0.01', pending=33))
    e.check('Final: 33 balanced settlements, USD 0.99 consumed', lambda:
            settled_workload() and intact_workload() and intact_workload('final') and e.final('final', count=33, consumed='0.99', frozen='0', remaining='0.01', pending=0))
    e.check('33 unique authorizations and holds', lambda: claimed_workload() and intact_workload() and len(e.snapshot('burst')['authorizations']) == 33
            and len({a['budget_hold']['id'] for a in e.snapshot('burst')['authorizations']}) == 33)
    e.check('All costs remain exactly three cents', lambda: settled_workload() and all(e.money(a['receipt']['actual_vendor_cost']) == Fraction('0.03')
            and e.money(a['receipt']['overrun_amount']) == 0 for a in e.snapshot('final')['authorizations']))
    e.check('No settlement/release during burst', lambda: min(datetime.fromisoformat(r['started_at']) for r in e.jsonl('requests.jsonl')
            if r.get('route') in ('/spend/executor/settle', '/spend/executor/release')) > max(datetime.fromisoformat(r['at'])
            for r in e.jsonl('clients.jsonl') if r['event'] == 'finish'))
    e.check('Admitted authorization identities match public history', lambda:
            {r['body']['decision_id'] for r in outcomes() if r['body'].get('decision') == 'allow'} ==
            {a['id'] for a in e.snapshot('final')['authorizations']})
    e.check('Every admitted claim matches its public burst authorization and hold', claim_mapping)
    e.check('Every normal settlement response matches its public final settlement', settlement_mapping)
    e.check('Operator configuration remained fixed', lambda: e.fixed())
    return e.result('v0.2.4 serializes TCP request handling. This tests competing network demand, not simultaneous SQLite writers, throughput or fairness.')


if __name__ == '__main__':
    import json
    result = verify(sys.argv[1])
    print(json.dumps(result, indent=2))
    raise SystemExit(1 if result['verdict'] == 'does not hold' else 2 if result['verdict'] == 'not tested' else 0)
