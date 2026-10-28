#!/usr/bin/env python3
"""Report a synthetic three-cent bill against a one-cent authorization."""
from datetime import datetime, timezone
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from runtime import run_episode
from verify import verify


def attack(rt):
    authorization = rt.authorize("billed-operation", "0.01")
    claim = rt.claim(authorization, label="claim-billed-operation")
    receipt = rt.receipt("last-cent-billed", "0.03")
    rt.write_fixture("synthetic-receipt.json", {
        "fixture": True,
        "description": "Synthetic executor bill; no provider was called or paid.",
        "authorized_max": {"amount": 10000, "scale": 6, "currency": "usd"},
        "receipt": receipt,
    })
    deadline = datetime.fromisoformat(claim["body"]["claim_expires_at"].replace("Z", "+00:00"))
    if datetime.now(timezone.utc) >= deadline:
        raise RuntimeError("Claim expired before the active-lease overrun probe")
    rt.note("Normal executor submits the immutable USD 0.03 fixture receipt.")
    rt.settle(claim, receipt, label="normal-overrun")
    rt.snapshot("held-after-overrun")
    rt.authorize("held-probe", "0.01")
    rt.wait_expiry(claim)
    rt.snapshot("expired")
    rt.note("OPERATOR: confirm the saved synthetic bill through the owner CLI capability.")
    evidence = "Synthetic last-cent fixture confirms a USD 0.03 bill; no real provider billing."
    rt.owner_reconcile(claim, receipt, evidence=evidence, label="owner-first")
    rt.snapshot("reconciled")
    rt.owner_reconcile(claim, receipt, evidence=evidence, label="owner-retry")
    for attempt in range(10):
        rt.authorize(f"after-reconcile-{attempt + 1:02}", "0.01")
    rt.snapshot("final")


if __name__ == "__main__":
    run_episode(attack, verify, episode="ep02-last-cent", budget="0.01", short_lease=True)
