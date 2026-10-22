#!/usr/bin/env python3
"""One deterministic agent keeps asking after ten synthetic one-cent settlements."""
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from runtime import run_episode
from verify import verify


def attack(rt):
    for index in range(1, 11):
        label = f'funded-{index:02}'
        authorization = rt.authorize(label, '0.01')
        if authorization['status'] == 200 and authorization['body']['decision'] == 'allow':
            claim = rt.claim(authorization, label=label + '-claim')
            if claim['status'] == 200:
                rt.settle(claim, rt.receipt(label, '0.01'), label=label + '-settle')
    rt.snapshot('exhausted')
    rt.note('ATTACK: 100 new logical attempts after the ten funded operations.')
    for index in range(1, 101):
        # Record every outcome, including an unexpected allow. Do not stop at the first denial.
        rt.authorize(f'after-budget-{index:03}', '0.01')
    rt.snapshot('final')


if __name__ == '__main__':
    run_episode(attack, verify, episode='ep01-runaway', budget='0.10')
