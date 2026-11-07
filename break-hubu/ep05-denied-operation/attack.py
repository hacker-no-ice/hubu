#!/usr/bin/env python3
"""C2: a policy-denied operation never gets a token, a hold, a claim or a posting."""
from pathlib import Path
import secrets
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from runtime import SCOPE, TIERED_POLICY, run_episode
from verify import verify


def attack(rt):
    rt.note('ATTACK: request USD 2.00, above the explicit USD 1.00 deny rule, then try every way around it.')
    rt.authorize('denied-request', '2.00')
    rt.authorize('denied-request', '2.00')  # exact replay of the same logical operation
    rt.authorize('denied-new-operation', '2.00')
    forged = 'sat_' + secrets.token_hex(16)
    rt.request('POST', '/spend/executor/resolve', {'spend_auth_token_id': forged}, label='forged-resolve')
    rt.request('POST', '/spend/executor/claim', {
        'spend_auth_token_id': forged, 'contract_version': 'hubu-spend-executor-v4.5', 'account_id': rt.ids['account_id'],
        'amount': {'units': '2000000', 'scale': 6, 'currency': 'usd'}, 'execution_scope': SCOPE}, label='forged-claim')
    rt.note('CONTROL: a small allowed operation, then an attempt to claim its token for the denied amount.')
    allowed = rt.authorize('allowed-small', '0.01')
    if allowed['status'] == 200 and allowed['body'].get('decision') == 'allow':
        rt.claim_with_amount(allowed, '2.00', label='allowed-token-denied-amount')
        claim = rt.claim(allowed, label='allowed-small-claim')
        if claim['status'] == 200:
            rt.settle(claim, rt.receipt('allowed-small', '0.01'), label='allowed-small-settle')
    rt.snapshot('final')


if __name__ == '__main__':
    run_episode(attack, verify, episode='ep05-denied-operation', budget='1.00', policy=TIERED_POLICY)
