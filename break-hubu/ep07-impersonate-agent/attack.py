#!/usr/bin/env python3
"""C3: an agent names another registered agent's account in its own spend request."""
import json
from pathlib import Path
import re
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from runtime import POLICY, run_episode
from verify import verify


def setup(rt):
    # Owner registers a second agent with its own policy and USD 1.00 budget.
    registration = rt.cli('register', 'agent', '--name', 'ep07-victim', '--version', rt.context['release'])['stdout']
    victim = {key: re.search(r'\b' + key + r':\s*(\S+)', registration).group(1) for key in ('agent_id', 'account_id')}
    policy = rt.private / 'victim-policy.yaml'; policy.write_text(POLICY)
    rt.cli('policy', 'apply', '--path', policy, '--name', 'Synthetic fixture allow policy', '--agent-id', victim['agent_id'])
    rt.cli('budget', 'create', '--agent-id', victim['agent_id'], '--amount', '1.00',
           '--currency', 'usd', '--ending-before', '2999-01-01T00:00:00Z')
    budgets = rt.request('GET', '/budgets', label='setup-victim-budget')['body']['budgets']
    victim['budget_id'] = next(b['budget_id'] for b in budgets if b['agent_id'] == victim['agent_id'])
    rt.ids['victim'] = victim
    (rt.output / 'victim.json').write_text(json.dumps(victim, indent=2) + '\n')


def attack(rt):
    victim = rt.ids['victim']
    rt.note('ATTACK: spend USD 0.05 while naming the other agent\'s account_id.')
    impersonated = rt.authorize('impersonate-victim', '0.05', account_id=victim['account_id'])
    if impersonated['status'] == 200 and impersonated['body'].get('decision') == 'allow':
        claim = rt.claim(impersonated, label='impersonate-victim-claim')
        if claim['status'] == 200:
            rt.settle(claim, rt.receipt('impersonate-victim', '0.05'), label='impersonate-victim-settle')
    rt.snapshot('final')


if __name__ == '__main__':
    run_episode(attack, verify, episode='ep07-impersonate-agent', budget='0.10', setup=setup)
