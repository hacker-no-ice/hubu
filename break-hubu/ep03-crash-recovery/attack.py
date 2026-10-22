#!/usr/bin/env python3
"""Kill the owned Hubu process after a claimed, synthetically billed operation."""
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from runtime import run_episode
from verify import verify


def attack(rt):
    authorization = rt.authorize("billed-operation", "0.01")
    claim = rt.claim(authorization, label="claim-billed-operation")
    receipt = rt.receipt("crash-billed", "0.01")
    rt.write_fixture("synthetic-receipt.json", {
        "fixture": True,
        "description": "Durable synthetic billed marker outside Hubu's database; no provider call.",
        "receipt": receipt,
    })
    decision_id = claim["body"]["spend"]["decision_id"]
    observed = rt.inspect(decision_id)["body"]["authorization_record"]
    if not (observed["status"] == "claimed"
            and observed["claim"]["id"] == claim["body"]["claim_id"]
            and observed["budget_hold"]["status"] == "claimed"
            and observed["receipt"] is None):
        raise RuntimeError("Public claimed/held/unsettled checkpoint was not reached; no timed kill")
    rt.snapshot("before-crash")
    rt.note("FAULT: SIGKILL the exact owned Hubu PID before any settle request.")
    rt.kill()
    rt.restart()
    rt.snapshot("after-restart")
    rt.authorize("restart-probe", "0.01")
    rt.wait_expiry(claim)
    rt.snapshot("expired")
    rt.settle(claim, receipt, label="expired-normal-settle")
    rt.release(claim, label="expired-normal-release")
    rt.snapshot("after-expired-probes")
    rt.note("OPERATOR: resolve the saved synthetic bill with the distinct owner capability.")
    evidence = "Durable synthetic crash fixture confirms a USD 0.01 bill; no real provider billing."
    rt.owner_reconcile(claim, receipt, evidence=evidence, label="owner-first")
    rt.snapshot("reconciled")
    rt.owner_reconcile(claim, receipt, evidence=evidence, label="owner-retry")
    rt.restart()
    rt.snapshot("final")


if __name__ == "__main__":
    run_episode(attack, verify, episode="ep03-crash-recovery", budget="0.01", short_lease=True)
