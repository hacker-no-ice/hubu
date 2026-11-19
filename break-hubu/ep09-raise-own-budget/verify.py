"""Verify the owner path for budgets and policies from saved public exports only."""
from fractions import Fraction
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from verification import Evidence


def verify(output):
    e = Evidence(output)

    def budget_unchanged():
        before, after = e.snapshot('initial')['budget'], e.snapshot('final')['budget']
        assert e.money(after['amount_limit']) == e.money(before['amount_limit']) == Fraction('0.10'), \
            f'The agent changed its own budget limit to {after["amount_limit"]}'
        return True

    def policy_unchanged():
        before = (e.output / 'snapshots/initial/policy.txt').read_text()
        after = (e.output / 'snapshots/final/policy.txt').read_text()
        assert before == after, 'The agent replaced its own policy'
        return True

    def no_spend_beyond_owner_budget():
        row = e.last('spend-after-raise')
        assert not (row['status'] == 200 and row['body']['decision'] == 'allow'), \
            'The agent obtained USD 0.50 against an owner budget of USD 0.10'
        return True

    e.check('Fresh USD 0.10 budget and empty public ledger', lambda: e.final(
        'initial', count=0, consumed='0', frozen='0', remaining='0.10', pending=0))
    e.check('The agent credential cannot raise its own budget', budget_unchanged)
    e.check('The agent credential cannot replace its own policy', policy_unchanged)
    e.check('No authorization beyond the owner\'s original budget', no_spend_beyond_owner_budget)
    return e.result()


if __name__ == '__main__':
    import json
    result = verify(sys.argv[1])
    print(json.dumps(result, indent=2))
    raise SystemExit(1 if result['verdict'] == 'does not hold' else 2 if result['verdict'] == 'not tested' else 0)
