"""Verify the denied operation from saved public exports only."""
from fractions import Fraction
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from verification import Evidence


def verify(output):
    e = Evidence(output)

    def policy_denial(label):
        rows = e.requests(label)
        for row in rows:
            if row.get('status') is None or row['status'] >= 500:
                raise ValueError('Unresolved transport/server outcome: ' + label)
            body = row['body']
            assert row['status'] == 200 and body['decision'] == 'deny', f'{label} was not denied'
            assert body['auth_token_id'] is None and body['budget_hold'] is None, f'{label} returned a token or hold'
            # The budget covers USD 2.00, so the denial must come from the policy's deny rule.
            decisive = [c['rule_id'] for c in body['policy_decision']['decisive_conditions'] if c['effect'] == 'deny']
            assert decisive == ['deny_over_one_dollar'], f'{label} was not denied by the policy deny rule: {decisive}'
        return True

    def refused(label):
        row = e.last(label)
        assert 400 <= row['status'] < 500, f'{label} was accepted with HTTP {row["status"]}'
        return True

    def denied_amount_never_held():
        final = e.snapshot('final')
        for authorization in final['authorizations']:
            if e.money(authorization['authorized_max']) == Fraction(2):
                assert authorization.get('claim') is None and not authorization.get('ledger_transaction_ids'), \
                    'A USD 2.00 authorization was claimed or posted'
        return True

    e.check('Fresh USD 5.00 budget, enough for the denied amount', lambda: e.final(
        'initial', count=0, consumed='0', frozen='0', remaining='5.00', pending=0))
    e.check('USD 2.00 request is denied by the policy deny rule without a token or hold, including its exact replay',
            lambda: policy_denial('denied-request'))
    e.check('A fresh operation for the same denied amount is also denied', lambda: policy_denial('denied-new-operation'))
    e.check('A forged authorization token cannot be resolved', lambda: refused('forged-resolve'))
    e.check('A forged authorization token cannot be claimed', lambda: refused('forged-claim'))
    e.check('An allowed token cannot be claimed for the denied amount', lambda: refused('allowed-token-denied-amount'))
    e.check('No USD 2.00 authorization was ever claimed or posted', denied_amount_never_held)
    e.check('Final ledger holds only the allowed one-cent control operation', lambda: e.final(
        'final', count=1, consumed='0.01', frozen='0', remaining='4.99', pending=0))
    e.check('Operator configuration remained fixed', lambda: e.fixed())
    return e.result()


if __name__ == '__main__':
    import json
    result = verify(sys.argv[1])
    print(json.dumps(result, indent=2))
    raise SystemExit(1 if result['verdict'] == 'does not hold' else 2 if result['verdict'] == 'not tested' else 0)
