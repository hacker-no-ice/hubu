"""Verify the approval review from saved public exports only."""
from fractions import Fraction
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from verification import Evidence

SCOPE_KEYS = ('provider', 'executor', 'capability', 'billing_merchant')


def verify(output):
    e = Evidence(output)
    run = e.json('run.json')

    def review(label):
        row = e.last(label)
        assert row['status'] == 200, f'{label} failed with HTTP {row["status"]}'
        return row['body']['review']

    def authoritative_fields():
        shown = review('review-before')
        requested = e.requests('misleading-request')[0]['request']
        assert e.money(shown['amount']) == Fraction('0.50'), f'Review shows amount {shown["amount"]}, not USD 0.50'
        assert shown['account_id'] == run['ids']['account_id'] and shown['agent_id'] == run['ids']['agent_id'], \
            'Review names a different account or agent'
        # The review shows each scope identity with its trusted catalog display name.
        ident = lambda value: value['id'] if isinstance(value, dict) else value
        assert all(ident(shown['execution_scope'][k]) == requested['execution_scope'][k] for k in SCOPE_KEYS), \
            'Review shows a different execution scope'
        return True

    def refused(label):
        row = e.last(label)
        assert 400 <= row['status'] < 500, f'{label} was accepted with HTTP {row["status"]}'
        return True

    def review_unchanged():
        assert review('review-before') == review('review-after'), 'The pending review changed after it was shown'
        return True

    def approved_as_reviewed():
        row = e.last('owner-approve')
        assert row['status'] == 200 and row['body']['decision'] == 'allow', 'Owner approval failed'
        hold = row['body']['budget_hold']
        assert e.money(hold['amount'] if 'amount' in hold else hold) == Fraction('0.50'), 'Approved hold differs from review'
        return True

    def reason_is_agent_text():
        try:
            return review('review-before')['reason'] == e.requests('misleading-request')[0]['request']['reason']
        except (OSError, ValueError, KeyError, IndexError, TypeError, AssertionError):
            return False

    e.check('Fresh USD 1.00 budget and empty public ledger', lambda: e.final(
        'initial', count=0, consumed='0', frozen='0', remaining='1.00', pending=0))
    e.check('The review shows the exact amount, account, agent and scope Hubu will authorize', authoritative_fields)
    e.check('The pending amount cannot be swapped under the same operation', lambda: refused('swap-amount-while-pending'))
    e.check('The pending description cannot be swapped under the same operation', lambda: refused('swap-reason-while-pending'))
    e.check('The review is unchanged after the swap attempts', review_unchanged)
    e.check('Approval reserves exactly the reviewed amount', approved_as_reviewed)
    e.check('The approved token cannot be claimed for more than was reviewed', lambda: refused('claim-larger-than-reviewed'))
    e.check('Final ledger holds exactly the reviewed USD 0.50', lambda: e.final(
        'final', count=1, consumed='0.50', frozen='0', remaining='0.50', pending=0))
    caveat = ('The review shows the agent\'s own description verbatim beside the exact amount and scope; '
              'a misleading description is visible to the human but not rejected.') if reason_is_agent_text() else ''
    return e.result(caveat)


if __name__ == '__main__':
    import json
    result = verify(sys.argv[1])
    print(json.dumps(result, indent=2))
    raise SystemExit(1 if result['verdict'] == 'does not hold' else 2 if result['verdict'] == 'not tested' else 0)
