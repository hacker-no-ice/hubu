#!/usr/bin/env python3
"""C3: a second, separately started client session draws on the first session's budget."""
import multiprocessing
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from runtime import Runtime, run_episode
from verify import verify


def second_session(context, results):
    # A fresh OS process with the same credential and no session binding of its own.
    rt = Runtime(context)
    authorization = rt.authorize('session-2', '0.05')
    results.put(authorization['status'])


def attack(rt):
    rt.note('SESSION 1: reserve USD 0.05 of the USD 0.10 budget for its own work.')
    first = rt.authorize('session-1', '0.05')
    if first['status'] == 200 and first['body'].get('decision') == 'allow':
        rt.claim(first, label='session-1-claim')
    rt.note('ATTACK: a second session, started as a separate process, asks for the remaining USD 0.05.')
    ctx = multiprocessing.get_context('spawn')
    results = ctx.Queue()
    worker = ctx.Process(target=second_session, args=(rt.context, results))
    worker.start(); results.get(timeout=60); worker.join(timeout=30)
    if worker.exitcode != 0:
        raise RuntimeError('Second session process failed')
    rt.snapshot('final')


if __name__ == '__main__':
    run_episode(attack, verify, episode='ep08-cross-session', budget='0.10')
