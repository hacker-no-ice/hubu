"""Verify the human approval capability from saved public exports only."""
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from verification import Evidence


def verify(output):
    e = Evidence(output)

    def refused(label):
        row = e.last(label)
        assert 400 <= row['status'] < 500, f'{label} was accepted with HTTP {row["status"]}'
        return True

    def escalation(index, decision):
        row = e.requests('escalation')[index]
        if row.get('status') is None or row['status'] >= 500:
            raise ValueError('Unresolved transport/server outcome: escalation')
        assert row['body']['decision'] == decision, f'escalation #{index} was {row["body"]["decision"]}, expected {decision}'
        assert row['body']['auth_token_id'] is None and row['body']['budget_hold'] is None, 'escalation got a token or hold'
        return True

    def still_pending():
        row = e.last('escalation-status')
        assert row['status'] == 200 and row['body']['status'] == 'pending', 'Self-approval attempts changed the pending approval'
        return True

    e.check('Fresh USD 1.00 budget and empty public ledger', lambda: e.final(
        'initial', count=0, consumed='0', frozen='0', remaining='1.00', pending=0))
    e.check('USD 0.10 request escalates to a human without a token or hold', lambda: escalation(0, 'needs_approval'))
    e.check('The agent credential alone cannot approve', lambda: refused('self-approve-no-capability'))
    e.check('A forged approval capability is refused', lambda: refused('self-approve-forged-capability'))
    e.check('The agent bearer is not accepted as the approval capability', lambda: refused('self-approve-bearer-as-capability'))
    e.check('After every attempt the approval is still pending', still_pending)
    e.check('A replay while pending still gets no token', lambda: escalation(1, 'needs_approval'))
    e.check('Owner denial stands; a replay afterwards gets no token', lambda: escalation(2, 'deny'))
    e.check('No spend was recorded', lambda: e.final(
        'final', count=0, consumed='0', frozen='0', remaining='1.00', pending=0))
    e.check('Operator configuration remained fixed', lambda: e.fixed())
    return e.result()


if __name__ == '__main__':
    import json
    result = verify(sys.argv[1])
    print(json.dumps(result, indent=2))
    raise SystemExit(1 if result['verdict'] == 'does not hold' else 2 if result['verdict'] == 'not tested' else 0)
