#!/usr/bin/env python3
"""C5: with only its own credential, the agent raises its budget and loosens its policy."""
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from runtime import run_episode
from verify import verify

LOOSE_POLICY = '''id: agent_written_allow_all
version: v1
default_effect: allow
rules: []
'''


def attack(rt):
    rt.note('ATTACK: raise the agent\'s own USD 0.10 budget to USD 100 with its own credential.')
    result = rt.cli('budget', 'update', '--budget-id', rt.ids['budget_id'], '--amount', '100',
                    '--reason', 'agent raising its own limit', '--yes', check=False)
    rt.write_fixture('raise-budget-cli.json', dict(exit_code=result['exit_code']))
    rt.note('ATTACK: replace the agent\'s own policy with an allow-all policy.')
    policy = rt.private / 'agent-policy.yaml'; policy.write_text(LOOSE_POLICY)
    result = rt.cli('policy', 'apply', '--path', policy, '--name', 'Agent-written allow all',
                    '--agent-id', rt.ids['agent_id'], check=False)
    rt.write_fixture('loosen-policy-cli.json', dict(exit_code=result['exit_code']))
    rt.note('PROBE: request USD 0.50, five times the owner\'s original budget.')
    rt.authorize('spend-after-raise', '0.50')
    rt.snapshot('final')


if __name__ == '__main__':
    run_episode(attack, verify, episode='ep09-raise-own-budget', budget='0.10')
