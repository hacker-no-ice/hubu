"""Verify exported public evidence, preserving contradictions on partial runs."""
from datetime import datetime
from fractions import Fraction
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from verification import Evidence


def verify(output):
    e = Evidence(output)
    record = lambda phase: e.snapshot(phase)["authorizations"][0]
    request = lambda label: e.requests(label)[-1]
    money = e.money

    def response(label):
        row = request(label)
        if row.get("status") is None or row["status"] >= 500:
            raise ValueError("Transport or server error is not a completed observation")
        return row

    def denied(label):
        row = response(label)
        body = row["body"]
        return (row["status"] == 200 and body["decision"] == "deny"
                and body["auth_token_id"] is None and body["budget_hold"] is None
                and any("budget" in reason.lower() for reason in body["reasons"]))

    def active_overrun():
        sent = datetime.fromisoformat(request("normal-overrun")["ended_at"].replace("Z", "+00:00"))
        deadline = datetime.fromisoformat(record("held-after-overrun")["claim"]["expires_at"].replace("Z", "+00:00"))
        if sent >= deadline:
            raise ValueError("The normal overrun probe missed the active lease")
        return True

    e.check("Fresh one-cent budget and empty public ledger", lambda: e.final(
        "initial", count=0, consumed="0", frozen="0", remaining="0.01", pending=0))
    e.check("Fresh instance has no earlier authorization records", lambda: not e.snapshot("initial")["authorizations"])
    e.check("Overrun probe ran before the returned lease deadline", active_overrun)
    e.check("Normal overrun rejected specifically as exceeding authorization", lambda:
        response("normal-overrun")["status"] == 400 and "actual vendor cost exceeds the authorized maximum"
        in response("normal-overrun")["body"]["error"])
    for phase in ("held-after-overrun", "expired"):
        e.check(f"{phase}: exactly one admitted operation", lambda phase=phase: len(e.snapshot(phase)["authorizations"]) == 1)
        e.check(f"{phase}: one cent remains held with no expense", lambda phase=phase: e.final(
            phase, count=0, consumed="0", frozen="0.01", remaining="0", pending=1))
        e.check(f"{phase}: claimed obligation has no receipt", lambda phase=phase:
            record(phase)["claim"]["status"] == "claimed" and record(phase)["receipt"] is None
            and record(phase)["budget_hold"]["status"] == "claimed"
            and money(record(phase)["budget_hold"]["amount"]) == Fraction(1, 100))
    e.check("Fresh request denied while the original cent remains held", lambda: denied("held-probe"))
    e.check("Expired authorization publicly requires reconciliation", lambda:
        record("expired")["status"] == "reconciliation_required"
        and record("expired")["claim"]["reconciliation_required"] is True)
    e.check("Original claim is visible in the public reconciliation queue", lambda:
        record("expired")["claim"]["id"] in [x["claim_id"] for x in
        response("reconciliation-visible")["body"]["claims"]])
    e.check("Owner first resolution and exact retry both succeeded", lambda:
        request("owner-first")["exit_code"] == request("owner-retry")["exit_code"] == 0)
    e.check("Owner reused the frozen rejected receipt", lambda:
        request("owner-first")["receipt"] == request("normal-overrun")["request"]["receipt"]
        == e.json("fixtures/synthetic-receipt.json")["receipt"])
    e.check("Owner retry preserved claim, receipt and evidence", lambda:
        all(request("owner-first")[key] == request("owner-retry")[key]
            for key in ("claim_id", "receipt", "evidence")))
    for index in range(1, 11):
        e.check(f"Post-reconciliation attempt {index} explicitly denied", lambda index=index:
            denied(f"after-reconcile-{index:02}"))
    for phase in ("reconciled", "final"):
        e.check(f"{phase}: no additional admitted operation", lambda phase=phase: len(e.snapshot(phase)["authorizations"]) == 1)
        e.check(f"{phase}: one three-cent expense, negative two-cent balance", lambda phase=phase: e.final(
            phase, count=1, consumed="0.03", frozen="0", remaining="-0.02", pending=0))
    e.check("Public receipt discloses original maximum and full overrun", lambda:
        money(record("final")["receipt"]["authorized_max"]) == Fraction(1, 100)
        and money(record("final")["receipt"]["actual_vendor_cost"]) == Fraction(3, 100)
        and money(record("final")["receipt"]["budget_charge"]) == Fraction(3, 100)
        and money(record("final")["receipt"]["overrun_amount"]) == Fraction(2, 100)
        and money(record("final")["receipt"]["released_amount"]) == 0)
    e.check("Public claim records an explicit billed reconciliation", lambda:
        record("final")["claim"]["reconciliation_outcome"] == "vendor_billed"
        and bool(record("final")["claim"]["reconciled_at"]))
    e.check("Resolution retained the original authorization, claim and hold", lambda:
        all(record(phase)["id"] == record("held-after-overrun")["id"]
            and record(phase)["claim"]["id"] == record("held-after-overrun")["claim"]["id"]
            and record(phase)["budget_hold"]["id"] == record("held-after-overrun")["budget_hold"]["id"]
            for phase in ("expired", "reconciled", "final")))
    e.check("Retry preserved settlement and canonical transaction identities", lambda:
        record("reconciled")["receipt"] == record("final")["receipt"]
        and record("reconciled")["ledger_transaction_ids"] == record("final")["ledger_transaction_ids"])
    e.check("Policy and budget configuration stayed fixed", lambda: e.fixed("initial", "final"))
    return e.result(caveat="The full overrun is recorded only after explicit owner reconciliation and lease expiry.")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("Usage: verify.py EVIDENCE_DIRECTORY")
    result = verify(Path(sys.argv[1]))
    print(json.dumps(result, indent=2))
    raise SystemExit(1 if result["verdict"] == "does not hold" else 2 if result["verdict"] == "not tested" else 0)
