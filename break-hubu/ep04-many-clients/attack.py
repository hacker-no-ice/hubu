#!/usr/bin/env python3
"""Ten ordinary OS processes compete through public HTTP for one fixed budget."""
import multiprocessing
import os
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from runtime import Runtime, append, now, run_episode, save
from verify import verify


def client(context, index, barrier, ready, results):
    rt = Runtime(context)
    ready.put(os.getpid())
    claims, error = [], None
    try:
        if not barrier.wait(timeout=60):
            raise RuntimeError('Client start barrier timed out')
        append(rt.output / 'clients.jsonl', dict(event='start', client=index, pid=os.getpid(), at=now()))
        for attempt in range(1, 11):
            label = f'client-{index:02}-{attempt:02}'
            authorization = rt.authorize(label, '0.03')
            if authorization['status'] == 200 and authorization['body']['decision'] == 'allow':
                claim = rt.claim(authorization, label=label + '-claim')
                claims.append((label, claim))
    except Exception as exc:
        error = str(exc)
    append(rt.output / 'clients.jsonl', dict(event='finish', client=index, pid=os.getpid(), at=now(), error=error))
    results.put(dict(client=index, claims=claims, error=error))


def attack(rt):
    ctx = multiprocessing.get_context('spawn')
    barrier, ready, results = ctx.Event(), ctx.Queue(), ctx.Queue()
    workers = [ctx.Process(target=client, args=(rt.context, i, barrier, ready, results)) for i in range(1, 11)]
    responses = []
    try:
        for worker in workers: worker.start()
        pids = [ready.get(timeout=60) for _ in workers]
        save(rt.output / 'workload.json', dict(client_processes=10, attempts_per_client=10,
             ready_pids=pids, barrier_released_at=now(), amount='0.03', budget='1.00'))
        rt.note('START BARRIER: ten ready client processes; no settlement during the burst.')
        barrier.set()
        # Drain queue before join: successful claim bodies can exceed pipe capacity.
        responses = [results.get(timeout=120) for _ in workers]
        for worker in workers: worker.join(timeout=10)
        if any(worker.is_alive() or worker.exitcode != 0 for worker in workers):
            raise RuntimeError('Client did not complete normally')
    finally:
        for worker in workers:
            if worker.is_alive(): worker.terminate()
            if worker.pid: worker.join(timeout=10)
    rt.snapshot('burst')
    rt.note('BURST FINISHED: now settle each admitted synthetic three-cent operation.')
    for response in responses:
        for label, claim in response['claims']:
            if claim['status'] == 200:
                rt.settle(claim, rt.receipt(label, '0.03'), label=label + '-settle')
    rt.snapshot('final')
    if any(response['error'] for response in responses):
        raise RuntimeError('One or more client processes reported errors; inspect clients.jsonl')


if __name__ == '__main__':
    run_episode(attack, verify, episode='ep04-many-clients', budget='1.00')
