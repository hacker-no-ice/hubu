"""Verify approval binding from saved public exports only."""
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from verification import Evidence


def verify(output):
    e = Evidence(output)

    def pending(label, index=0):
        row = e.requests(label)[index]
        if row.get('status') is None or row['status'] >= 500:
            raise ValueError('Unresolved transport/server outcome: ' + label)
        body = row['body']
        assert row['status'] == 200 and body['decision'] == 'needs_approval', f'{label} did not need a fresh approval'
        assert body['auth_token_id'] is None and body['budget_hold'] is None, f'{label} got a token or hold before approval'
        return True

    def refused(label):
        row = e.last(label)
        assert 400 <= row['status'] < 500, f'{label} was accepted with HTTP {row["status"]}'
        return True

    def approved_token():
        row = e.last('owner-approve-a')
        assert row['status'] == 200 and row['body']['decision'] == 'allow', 'Owner approval did not allow the request'
        return row['body']

    def replay_is_same_approval():
        first, replay = approved_token(), e.last('owner-approve-a-replay')
        assert replay['status'] == 200 and replay['body'].get('idempotent_replay') is True, 'Approval replay was not idempotent'
        assert replay['body']['decision_id'] == first['decision_id'], 'Approval replay created another decision'
        return True

    def denied_stays_denied():
        assert e.last('owner-deny-b')['body']['decision'] == 'deny', 'Owner denial was not recorded'
        refused('owner-approve-b-after-deny')
        replay = e.requests('reuse-new-operation')[-1]
        assert replay['body']['decision'] != 'allow' and replay['body']['auth_token_id'] is None, 'Denied operation later got a token'
        return True

    e.check('Fresh USD 1.00 budget and empty public ledger', lambda: e.final(
        'initial', count=0, consumed='0', frozen='0', remaining='1.00', pending=0))
    e.check('USD 0.10 request waits for approval without a token or hold', lambda: pending('approval-a'))
    e.check('The pending approval request cannot be used as an executor token', lambda: refused('claim-before-approval'))
    e.check('Owner approval issues a token for the reviewed operation', lambda: bool(approved_token()['auth_token_id']))
    e.check('The approval does not carry over to a new operation', lambda: pending('reuse-new-operation'))
    e.check('The approval does not carry over to a larger amount', lambda: pending('reuse-larger-amount'))
    e.check('The approved operation cannot be replayed with a changed amount', lambda: refused('replay-a-changed-amount'))
    e.check('The approved token cannot be claimed for a different amount', lambda: refused('claim-a-altered-amount'))
    e.check('Replaying the approval returns the same authorization', replay_is_same_approval)
    e.check('A denied approval cannot be approved later or replayed into a token', denied_stays_denied)
    e.check('Final ledger holds only the approved USD 0.10 operation', lambda: e.final(
        'final', count=1, consumed='0.10', frozen='0', remaining='0.90', pending=0))
    e.check('Operator configuration remained fixed', lambda: e.fixed())
    return e.result()


if __name__ == '__main__':
    import json
    result = verify(sys.argv[1])
    print(json.dumps(result, indent=2))
    raise SystemExit(1 if result['verdict'] == 'does not hold' else 2 if result['verdict'] == 'not tested' else 0)
