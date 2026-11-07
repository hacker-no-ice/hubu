"""Verify ledger immutability through public interfaces from saved exports only."""
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from verification import Evidence


def verify(output):
    e = Evidence(output)

    def refused(label):
        row = e.last(label)
        assert not 200 <= row['status'] < 300, f'{label} was accepted with HTTP {row["status"]}'
        return True

    def resettle_did_not_reprice():
        row = e.last('resettle-lower')
        if 200 <= row['status'] < 300:
            original = e.last('spend-1-settle')['body']
            assert row['body'].get('settlement_id') == original.get('settlement_id'), 'A second settlement was recorded'
        return True

    def agent_reconcile_refused():
        for label in ('agent-reconcile-no-capability', 'agent-reconcile-forged-capability'):
            row = e.last(label)
            assert 400 <= row['status'] < 500, f'{label} was accepted with HTTP {row["status"]}'
        # The same claim was reconcilable: the owner path succeeded afterwards.
        owner = [r for r in e.jsonl('requests.jsonl') if r.get('type') == 'owner_cli' and r['label'] == 'owner-reconcile-spend-3']
        if not owner or owner[-1]['exit_code'] != 0:
            raise ValueError('Owner reconciliation of the expired claim did not complete')
        return True

    def ledger_unchanged():
        spent, final = e.snapshot('spent'), e.snapshot('final')
        before = {t['id']: (t['effective_cost'], t['budget_charge_delta']) for t in spent['transactions']}
        after = {t['id']: (t['effective_cost'], t['budget_charge_delta']) for t in final['transactions']}
        assert {k: after.get(k) for k in before} == before, 'Recorded postings changed after the agent\'s attempts'
        return True

    e.check('Fresh USD 0.10 budget and empty public ledger', lambda: e.final(
        'initial', count=0, consumed='0', frozen='0', remaining='0.10', pending=0))
    e.check('Two three-cent operations are recorded and a third is held', lambda: e.final(
        'spent', count=2, consumed='0.06', frozen='0.03', remaining='0.01', pending=1))
    for label, name in [('delete-transaction', 'Deleting a posting'), ('overwrite-transaction', 'Overwriting a posting'),
                        ('reverse-transaction', 'Reversing a posting'), ('void-settled', 'Voiding a settled authorization'),
                        ('release-settled', 'Releasing a settled claim')]:
        e.check(name + ' through the public API is refused', lambda k=label: refused(k))
    e.check('Settling again at a lower cost does not re-price the spend', resettle_did_not_reprice)
    e.check('The agent cannot reconcile an expired claim without the owner capability', agent_reconcile_refused)
    e.check('Every recorded posting is unchanged', ledger_unchanged)
    e.check('Final accounting shows only the two settlements and the owner\'s reconciliation', lambda: e.final(
        'final', count=3, consumed='0.09', frozen='0', remaining='0.01', pending=0))
    e.check('Operator configuration remained fixed', lambda: e.fixed())
    return e.result('This covers Hubu\'s public HTTP and CLI interfaces. Editing the database file directly is a separate challenge that needs Hubu\'s state under a different OS user.')


if __name__ == '__main__':
    import json
    result = verify(sys.argv[1])
    print(json.dumps(result, indent=2))
    raise SystemExit(1 if result['verdict'] == 'does not hold' else 2 if result['verdict'] == 'not tested' else 0)
