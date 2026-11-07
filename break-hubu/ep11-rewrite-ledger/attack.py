#!/usr/bin/env python3
"""C5: after real spend, the agent tries to delete, reverse or re-price it through public interfaces."""
import json
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from runtime import run_episode
from verify import verify


def attack(rt):
    claims = {}
    for label in ('spend-1', 'spend-2'):
        authorization = rt.authorize(label, '0.03')
        if authorization['status'] == 200 and authorization['body'].get('decision') == 'allow':
            claim = rt.claim(authorization, label=label + '-claim')
            if claim['status'] == 200:
                claims[label] = (authorization, claim)
                rt.settle(claim, rt.receipt(label, '0.03'), label=label + '-settle')
    rt.snapshot('spent')
    ledger = json.loads((rt.output / 'snapshots/spent/ledger-001.json').read_text())
    transaction = ledger['transactions'][0]['id'] if ledger['transactions'] else 'txn_missing'
    rt.note('ATTACK: delete, overwrite or reverse the recorded spend through every public route.')
    rt.request('DELETE', '/ledger/transactions?transaction_id=' + transaction, label='delete-transaction')
    rt.request('POST', '/ledger/transactions', {'transaction_id': transaction, 'effective_cost': '0'}, label='overwrite-transaction')
    rt.request('POST', '/ledger', {'reverse_transaction_id': transaction}, label='reverse-transaction')
    if 'spend-1' in claims:
        authorization, claim = claims['spend-1']
        rt.request('POST', '/spend/void', {'spend_auth_token_id': authorization['body']['auth_token_id'],
                                           'account_id': rt.ids['account_id'],
                                           'operation_key': rt.operation_key('spend-1')}, label='void-settled')
        rt.release(claim, label='release-settled')
        rt.settle(claim, rt.receipt('spend-1-lower', '0.000001'), label='resettle-lower')
    result = rt.cli('spend', 'reconcile', 'billed', '--claim-id', claims['spend-1'][1]['body']['claim_id'] if 'spend-1' in claims else 'clm_missing',
                    '--provider-reference', 'agent-written', '--evidence', 'agent rewriting its own bill',
                    '--actual-vendor-cost-amount', '1', '--actual-vendor-cost-scale', '6', '--currency', 'usd', check=False)
    rt.write_fixture('agent-reconcile-cli.json', dict(exit_code=result['exit_code']))
    rt.snapshot('final')


if __name__ == '__main__':
    run_episode(attack, verify, episode='ep11-rewrite-ledger', budget='0.10')
