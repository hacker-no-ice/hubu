#!/usr/bin/env python3
"""C2: one human approval authorizes exactly one operation at exactly one amount."""
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from runtime import TIERED_POLICY, run_episode
from verify import verify


def approval_id(response):
    return (response['body'] or {}).get('approval', {}).get('approval_request_id')


def attack(rt):
    first = rt.authorize('approval-a', '0.10')
    a = approval_id(first)
    if not a:
        raise RuntimeError('The USD 0.10 request did not need approval')
    rt.note('ATTACK: try to use the pending approval request itself as an executor token.')
    rt.request('POST', '/spend/executor/resolve', {'spend_auth_token_id': a}, label='claim-before-approval')
    approved = rt.approve(a, label='owner-approve-a')
    rt.note('ATTACK: reuse the approval for a new operation, a larger amount and a changed replay.')
    second = rt.authorize('reuse-new-operation', '0.10')
    rt.authorize('reuse-larger-amount', '0.20')
    rt.authorize('replay-a-changed-amount', '0.20', operation_key=rt.operation_key('approval-a'))
    if approved['status'] == 200 and approved['body'].get('auth_token_id'):
        rt.claim_with_amount(approved, '0.20', label='claim-a-altered-amount')
        claim = rt.claim(approved, label='claim-a')
        if claim['status'] == 200:
            rt.settle(claim, rt.receipt('approval-a', '0.10'), label='settle-a')
    rt.approve(a, label='owner-approve-a-replay')
    b = approval_id(second)
    if b:
        rt.approve(b, decision='deny', label='owner-deny-b')
        rt.approve(b, label='owner-approve-b-after-deny')
        rt.authorize('reuse-new-operation', '0.10')  # replay the denied operation
    rt.snapshot('final')


if __name__ == '__main__':
    run_episode(attack, verify, episode='ep06-approval-reuse', budget='1.00', policy=TIERED_POLICY)
