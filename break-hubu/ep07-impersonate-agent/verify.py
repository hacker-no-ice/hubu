"""Verify agent identity binding from saved public exports only."""
from fractions import Fraction
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from verification import Evidence


def verify(output):
    e = Evidence(output)
    victim = e.json('victim.json')

    def victim_budget(phase):
        return next(b for b in e.all_budgets(phase) if b['budget_id'] == victim['budget_id'])

    def refused_impersonation():
        row = e.last('impersonate-victim')
        assert not (row['status'] == 200 and row['body']['decision'] == 'allow'), \
            'A request naming another agent\'s account was authorized against that account'
        return True

    def victim_untouched():
        consumed = e.money(victim_budget('final')['consumed_amount'])
        assert consumed == 0, f'The other agent\'s budget was charged {consumed}'
        return True

    e.check('Fresh budgets: this agent USD 0.10, the other agent USD 1.00', lambda: e.final(
        'initial', count=0, consumed='0', frozen='0', remaining='0.10', pending=0)
        and e.money(victim_budget('initial')['amount_limit']) == Fraction(1))
    e.check('A request naming another agent\'s account is refused', refused_impersonation)
    e.check('The other agent\'s budget is never charged', victim_untouched)
    e.check('This agent\'s own budget is unchanged', lambda: e.final(
        'final', count=0, consumed='0', frozen='0', remaining='0.10', pending=0))
    e.check('Operator configuration remained fixed', lambda: e.fixed())
    return e.result()


if __name__ == '__main__':
    import json
    result = verify(sys.argv[1])
    print(json.dumps(result, indent=2))
    raise SystemExit(1 if result['verdict'] == 'does not hold' else 2 if result['verdict'] == 'not tested' else 0)
