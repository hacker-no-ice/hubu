"""Verify the public claimed obligation and one resolution across a real kill."""
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
    events = lambda kind: [x for x in e.jsonl("process-events.jsonl") if x["event"] == kind]
    timestamp = lambda value: datetime.fromisoformat(value.replace("Z", "+00:00"))

    def response(label):
        row = request(label)
        if row.get("status") is None or row["status"] >= 500:
            raise ValueError("Transport or server error is not a completed observation")
        return row

    def crash_boundary():
        kills, restarts = events("kill"), events("restart")
        killed = kills[0]
        marker = [x for x in events("fixture_saved") if x["path"] == "fixtures/synthetic-receipt.json"][0]
        before = [x for x in e.jsonl("requests.jsonl") if x.get("type") == "http"
                  and timestamp(x["started_at"]) < timestamp(killed["at"])]
        valid = (len(kills) == 1 and len(restarts) == 3 and killed["signal"] == "SIGKILL"
                and killed["exit_code"] == -9 and killed["pid"] == restarts[0]["pid"]
                and restarts[1]["pid"] != killed["pid"]
                and timestamp(marker["at"]) < timestamp(killed["at"]) < timestamp(restarts[1]["at"])
                and not any(x["route"] == "/spend/executor/settle" for x in before))
        if not valid:
            raise ValueError("Recorded fault/restart sequence did not reach the declared crash boundary")
        return True

    e.check("Fresh one-cent budget and empty public ledger", lambda: e.final(
        "initial", count=0, consumed="0", frozen="0", remaining="0.01", pending=0))
    e.check("Fresh instance has no earlier authorization records", lambda: not e.snapshot("initial")["authorizations"])
    e.check("Durable fixture preceded an owned SIGKILL before settlement", crash_boundary)
    e.check("Public pre-crash checkpoint was claimed and unsettled", lambda:
        record("before-crash")["status"] == "claimed" and record("before-crash")["receipt"] is None)
    for phase in ("before-crash", "after-restart", "expired", "after-expired-probes"):
        e.check(f"{phase}: exactly one admitted operation", lambda phase=phase: len(e.snapshot(phase)["authorizations"]) == 1)
        e.check(f"{phase}: frozen cent survived with no expense or replacement capacity", lambda phase=phase: e.final(
            phase, count=0, consumed="0", frozen="0.01", remaining="0", pending=1))
        e.check(f"{phase}: original authorization, claim and hold survive", lambda phase=phase:
            record(phase)["id"] == record("before-crash")["id"]
            and record(phase)["claim"]["id"] == record("before-crash")["claim"]["id"]
            and record(phase)["budget_hold"]["id"] == record("before-crash")["budget_hold"]["id"]
            and record(phase)["budget_hold"]["status"] == "claimed"
            and e.money(record(phase)["budget_hold"]["amount"]) == Fraction(1, 100)
            and e.money(record(phase)["authorized_max"]) == Fraction(1, 100)
            and record(phase)["receipt"] is None)
    e.check("Fresh request after restart explicitly denied without a hold", lambda:
        response("restart-probe")["status"] == 200 and response("restart-probe")["body"]["decision"] == "deny"
        and response("restart-probe")["body"]["auth_token_id"] is None
        and response("restart-probe")["body"]["budget_hold"] is None
        and any("budget" in x.lower() for x in response("restart-probe")["body"]["reasons"]))
    e.check("Expired claim requires reconciliation and appears in public queue", lambda:
        record("expired")["status"] == "reconciliation_required"
        and record("expired")["claim"]["reconciliation_required"] is True
        and record("expired")["claim"]["id"] in [x["claim_id"] for x in
        response("reconciliation-visible")["body"]["claims"]])
    for label in ("expired-normal-settle", "expired-normal-release"):
        e.check(f"{label} rejected specifically for expired claim", lambda label=label:
            response(label)["status"] == 400 and "expired and requires reconciliation" in response(label)["body"]["error"])
    e.check("Owner first resolution and exact retry both succeeded", lambda:
        request("owner-first")["exit_code"] == request("owner-retry")["exit_code"] == 0)
    e.check("Owner reused the durable synthetic receipt", lambda:
        request("owner-first")["receipt"] == e.json("fixtures/synthetic-receipt.json")["receipt"])
    e.check("Owner retry preserved claim, receipt and evidence", lambda:
        all(request("owner-first")[key] == request("owner-retry")[key] for key in ("claim_id", "receipt", "evidence")))
    for phase in ("reconciled", "final"):
        e.check(f"{phase}: no additional admitted operation", lambda phase=phase: len(e.snapshot(phase)["authorizations"]) == 1)
        e.check(f"{phase}: one canonical cent settled, no pending obligation", lambda phase=phase: e.final(
            phase, count=1, consumed="0.01", frozen="0", remaining="0", pending=0))
    e.check("Public receipt and claim show billed owner resolution", lambda:
        e.money(record("final")["receipt"]["actual_vendor_cost"]) == Fraction(1, 100)
        and e.money(record("final")["receipt"]["overrun_amount"]) == 0
        and record("final")["claim"]["reconciliation_outcome"] == "vendor_billed"
        and bool(record("final")["claim"]["reconciled_at"]))
    e.check("Retry and final restart retain settlement, transaction, claim and hold identities", lambda:
        record("reconciled")["receipt"] == record("final")["receipt"]
        and record("reconciled")["ledger_transaction_ids"] == record("final")["ledger_transaction_ids"]
        and record("final")["id"] == record("before-crash")["id"]
        and record("final")["claim"]["id"] == record("before-crash")["claim"]["id"]
        and record("final")["budget_hold"]["id"] == record("before-crash")["budget_hold"]["id"])
    e.check("Policy and budget configuration stayed fixed", lambda: e.fixed("initial", "final"))
    return e.result()


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("Usage: verify.py EVIDENCE_DIRECTORY")
    result = verify(Path(sys.argv[1]))
    print(json.dumps(result, indent=2))
    raise SystemExit(1 if result["verdict"] == "does not hold" else 2 if result["verdict"] == "not tested" else 0)
