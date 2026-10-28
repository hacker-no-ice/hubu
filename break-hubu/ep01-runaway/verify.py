"""Read public exports independently of attack counters."""
from fractions import Fraction
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from verification import Evidence, budget_denial


def verify(output):
    e = Evidence(output)
    def funding_complete():
        for index in range(1, 11):
            label = f'funded-{index:02}'
            for suffix in ('', '-claim-resolve', '-claim', '-settle'):
                row = e.requests(label + suffix)[-1]
                if row['status'] != 200:
                    raise ValueError('Funding operation did not complete successfully: ' + label + suffix)
                if not suffix and row['body'].get('decision') != 'allow':
                    raise ValueError('Funding operation was not admitted: ' + label)
        return True

    def exhausted():
        budget = e.json('snapshots/exhausted/budgets.json')['budgets'][0]
        if not (e.money(budget['amount_limit']) == e.money(budget['consumed_amount']) == Fraction('0.10')
                and e.money(budget['frozen_amount']) == e.money(budget['remaining_amount']) == 0):
            raise ValueError('Public state did not reach the declared consumed-budget checkpoint')
        return True

    def observed_cap():
        # A later missing response must not erase an already reported excess obligation.
        rows = [r for r in e.jsonl('requests.jsonl') if r.get('route') == '/spend/authorize'
                and r.get('status') == 200 and r['body'].get('budget_hold')]
        if not rows:
            raise ValueError('No public reservation observation')
        return all(e.money(r['body']['budget_hold']['consumed_amount'])
                   + e.money(r['body']['budget_hold']['frozen_amount']) <= Fraction('0.10') for r in rows)

    def complete_attempt_trace():
        expected = {f'funded-{i:02}' for i in range(1, 11)} | {f'after-budget-{i:03}' for i in range(1, 101)}
        actual = {r['label'] for r in e.jsonl('requests.jsonl') if r.get('route') == '/spend/authorize'}
        if actual != expected:
            raise ValueError('Missing or unexpected logical attempts in the declared 110-request workload')
        return True

    e.check('Fresh USD 0.10 budget', lambda: e.final('initial', count=0, consumed='0', frozen='0', remaining='0.10', pending=0))
    e.check('No observed reservation exceeded the fixed ten-cent budget', observed_cap)
    e.check('All ten funded authorize/resolve/claim/settle responses completed', funding_complete)
    for phase in ('exhausted', 'final'):
        e.check(phase + ': ten balanced settlements, USD 0.10 consumed, no hold',
                lambda p=phase: funding_complete() and e.final(p, count=10, consumed='0.10', frozen='0', remaining='0', pending=0))
    for index in range(1, 101):
        label = f'after-budget-{index:03}'
        e.check(label + ': explicit budget denial without token/hold', lambda k=label: exhausted() and budget_denial(e.requests(k)[-1]))
    e.check('Exactly 110 distinct logical attempts recorded', complete_attempt_trace)
    e.check('Exactly ten public authorizations, all settled at one cent', lambda:
            funding_complete() and len(e.snapshot('final')['authorizations']) == 10 and all(a['status'] == 'settled'
            and e.money(a['receipt']['budget_charge']) == Fraction('0.01')
            and e.money(a['receipt']['overrun_amount']) == 0 for a in e.snapshot('final')['authorizations']))
    e.check('All posting identities unchanged after the 100 denials', lambda:
            funding_complete() and {t['id'] for t in e.snapshot('exhausted')['transactions']} == {t['id'] for t in e.snapshot('final')['transactions']})
    e.check('Operator configuration remained fixed', lambda: e.fixed())
    return e.result()


if __name__ == '__main__':
    import json
    result = verify(sys.argv[1])
    print(json.dumps(result, indent=2))
    raise SystemExit(1 if result['verdict'] == 'does not hold' else 2 if result['verdict'] == 'not tested' else 0)
