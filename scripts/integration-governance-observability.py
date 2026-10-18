#!/usr/bin/env python3
"""Qualify governance answers through real Hubu HTTP and unified MCP reads.

All business records are synthetic and created through authenticated HTTP in a
fresh temporary database. No Gongbu or provider is configured. v4.5 is the
release target; explicit --expected-contract v4.4 runs provisional read proof.
"""
from __future__ import annotations

import argparse
import datetime as dt
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sqlite3
import sys
import tempfile
import time
import urllib.parse

SCAFFOLD = Path(__file__).with_name("integration-standalone-mcp-executor.py")
spec = importlib.util.spec_from_file_location("governance_qualification_scaffold", SCAFFOLD)
assert spec and spec.loader
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)
check = base.check
# Keep ordinary authorizations alive through restart/qualification. Only the
# explicitly selected short claim leases expire during the reconciliation case.
base.LEASE_CONFIG = (
    "authorization_ttl_seconds: 3600\ndefault_lease_profile: default\n"
    "lease_profiles:\n  default:\n    claim_ttl_seconds: 900\n"
    "  conformance_short:\n    claim_ttl_seconds: 1\n"
)
POLICY = """id: governance_qualification
version: v1
default_effect: needs_approval
rules:
  - id: deny_large
    effect: deny
    when:
      op: gt
      field: amount
      value:
        money: "0.02"
    reason: synthetic upper bound
  - id: allow_small
    effect: allow
    when:
      op: lte
      field: amount
      value:
        money: "0.005"
    reason: synthetic automatic limit
"""
PRIVATE = "PRIVATE_GOVERNANCE_QUALIFICATION"


def instant(value: dt.datetime | None = None) -> str:
    return (value or dt.datetime.now(dt.timezone.utc)).isoformat(timespec="microseconds").replace("+00:00", "Z")


def parse_instant(value: str) -> dt.datetime:
    return dt.datetime.fromisoformat(value.replace("Z", "+00:00"))


def stages(summary: dict) -> dict:
    return {row["outcome"]: row["fact_count"] for row in summary["stages"]}


class IsolatedHubu(base.HubuServer):
    def start(self) -> None:
        try:
            super().start()
        except Exception:
            self.stop()
            raise


class Qualification:
    def __init__(self, hubu_bin: Path, mcp_bin: Path, root: Path, contract: str):
        # Imported scaffold receives only isolated Hubu settings, never a user's
        # selected profile, log destination, backend endpoint or capability file.
        for key in list(os.environ):
            if key.startswith(("HUBU_", "GONGBU_")):
                os.environ.pop(key)
        self.hubu = IsolatedHubu(hubu_bin, root)
        self.mcp = None
        self.root, self.contract = root, contract
        self.private = [PRIVATE, base.NORMAL_BEARER, base.APPROVAL_TOKEN, base.RECONCILIATION_TOKEN]
        self.read_outputs: list[dict] = []
        self.claim_requests: dict[str, dict] = {}
        try:
            guidance = self.http("GET", "/spend/executor/guidance")
            check(guidance["protocol_version"] == f"hubu-spend-executor-{contract}",
                  f"expected {contract}, got {guidance['protocol_version']}; integrate the published precision baseline first")
            self.build = self.http("GET", "/version")
            self.http("POST", "/init", {"username": "governance", "display_name": "Synthetic governance owner"})
            self.mcp = base.UnifiedMcp(mcp_bin, self.hubu, root)
        except Exception:
            self.close()
            raise

    def close(self) -> None:
        if self.mcp:
            self.mcp.close()
        self.hubu.stop()

    def http(self, method: str, path: str, body=None, *, expected=200, headers=None):
        status, result = base.http(self.hubu.base, method, path, body, headers)
        check(status == expected, f"{method} {path}: expected {expected}, got {status}: {result}")
        return result

    def read(self, tool: str, arguments: dict, *, error=False) -> dict:
        assert self.mcp
        response = self.mcp.request("tools/call", {"name": tool, "arguments": arguments})
        self.read_outputs.append(response)
        check("result" in response, f"{tool} returned a JSON-RPC error")
        result = response["result"]
        check(bool(result.get("isError")) == error, f"unexpected {tool} error state: {result}")
        structured = result.get("structuredContent")
        content = result.get("content", [])
        check(isinstance(structured, dict) and len(content) == 1 and
              content[0].get("type") == "text" and json.loads(content[0]["text"]) == structured,
              "MCP text and structured governance projections diverged")
        return structured

    def summary(self, filters: dict, **extra) -> dict:
        return self.read("hubu_governance_summary", {**filters, **extra})["summary"]

    def records(self, filters: dict, summary=None) -> tuple[dict, list[dict]]:
        summary = summary or self.summary(filters)
        args = {**filters, "snapshot": summary["snapshot"]["token"], "limit": 2}
        records, seen = [], set()
        while True:
            page = self.read("hubu_list_governance_records", args)
            check(page["summary"] == summary, "pagination changed whole-filter summary or frozen boundary")
            records.extend(page["records"])
            cursor = page["next_cursor"]
            if not cursor:
                break
            check(cursor not in seen, "pagination repeated a cursor")
            seen.add(cursor)
            args = {**filters, "cursor": cursor, "limit": 3}
        check(len(records) == summary["fact_count"], "full-filter fact total disagrees with supporting pages")
        check(len({row["record_id"] for row in records}) == len(records), "a page duplicated an event")
        return summary, records

    def detail(self, filters: dict, summary: dict, record: dict) -> dict:
        result = self.read("hubu_get_governance_record", {
            **filters, "snapshot": summary["snapshot"]["token"], "record_id": record["record_id"],
        })
        check(result["summary"] == summary and result["record"] == record, "detail diverged from its list snapshot")
        return result["record"]

    def parity(self, filters: dict, summary: dict) -> None:
        args = {**filters, "snapshot": summary["snapshot"]["token"]}
        encoded = {key: ",".join(value) if isinstance(value, list) else value for key, value in args.items()}
        response = self.http("GET", "/governance/summary?" + urllib.parse.urlencode(encoded))
        self.read_outputs.append(response)
        check(response["summary"] == summary, "HTTP and MCP summary projections diverged")

    def agent(self, name: str, *, policy=POLICY, usd=None, usdc=None) -> dict:
        agent = self.http("POST", "/agents/register", {"name": name, "version": "qualification-v1"})
        self.policy(agent, policy)
        for currency, limit in [("usd", usd), ("usdc", usdc)]:
            if limit is not None:
                self.add_budget(agent, currency, limit)
        return agent

    def policy(self, agent: dict, policy: str) -> dict:
        return self.http("POST", "/policies", {"agent_id": agent["agent_id"], "policy_yaml": policy})

    def add_budget(self, agent: dict, currency: str, limit: str) -> dict:
        return self.http("POST", "/budgets", {
            "agent_id": agent["agent_id"], "amount": limit, "currency": currency,
            "ending_before": "2999-01-01T00:00:00Z",
        })["budget"]

    def authorize(self, agent: dict, label: str, amount: str, currency="usd", lease="default") -> dict:
        key = f"{PRIVATE}:{agent['agent_id']}:{label}"
        self.private.append(key)
        result = self.http("POST", "/spend/authorize", {
            "operation_key": key, "account_id": agent["account_id"], "amount": amount,
            "currency": currency, "reason": PRIVATE, "execution_scope": base.SCOPE,
            "lease_profile": lease,
        })
        if result.get("auth_token_id"):
            self.private.append(result["auth_token_id"])
        return result

    def claim(self, authorization: dict) -> dict:
        token = authorization["auth_token_id"]
        snapshot = self.http("POST", "/spend/executor/resolve", {"spend_auth_token_id": token})
        request = {"spend_auth_token_id": token, "account_id": snapshot["account_id"],
                   "currency": snapshot["currency"], "execution_scope": snapshot["execution_scope"]}
        if self.contract == "v4.5":
            request.update(contract_version="hubu-spend-executor-v4.5", amount={
                key: snapshot["amount"][key] for key in ("units", "scale", "currency")})
        else:
            request["amount_cents"] = snapshot["amount_cents"]
        self.claim_requests[token] = request
        return self.http("POST", "/spend/executor/claim", request)

    def receipt(self, currency="usd", units=3101) -> dict:
        return {"actual_vendor_cost": {"amount": units, "scale": 6, "currency": currency},
                "provider_request_id": PRIVATE + "_PROVIDER_REQUEST", "artifact_reference": "artifact://" + PRIVATE,
                "price_model_snapshot": {"provider": "deterministic-fixture", "model": "synthetic", "currency": currency}}

    def resolve_human(self, authorization: dict, action: str, *, expected=200) -> dict:
        return self.http("POST", "/spend/approval/resolve", {
            "approval_request_id": authorization["approval"]["approval_request_id"], "decision": action,
        }, expected=expected, headers={"X-Hubu-Approval-Capability": base.APPROVAL_TOKEN})

    def late_reconciliation(self, agent: dict) -> dict:
        filters = {"agent_id": agent["agent_id"]}
        # Test-only fault injection holds back observation even on a slow CI
        # host. It changes no business records and does not rely on the 30s tick.
        with sqlite3.connect(self.root / "hubu.sqlite3") as connection:
            connection.execute("""CREATE TRIGGER qualification_delay_reconciliation
                BEFORE INSERT ON spend_lifecycle_records
                WHEN json_extract(NEW.payload_json, '$.input.fact.kind') = 'reconciliation_required'
                BEGIN SELECT RAISE(ABORT, 'qualification observation barrier'); END""")
        try:
            claims = [self.claim(self.authorize(agent, f"short-{i}", "0.004", lease="conformance_short")) for i in range(2)]
            expiry = max(parse_instant(claim["claim_expires_at"]) for claim in claims)
            time.sleep(max(0, (expiry - dt.datetime.now(dt.timezone.utc)).total_seconds()) + 0.05)
            # A later authoritative fact anchors the snapshot AFTER expiry.
            check(self.authorize(agent, "late-snapshot-anchor", "0.03")["decision"] == "deny", "anchor was not denied")
            frozen = self.summary(filters)
            check(frozen["backlog"]["unresolved_reconciliations"] == 0, "read bypassed the observation barrier")
        finally:
            with sqlite3.connect(self.root / "hubu.sqlite3") as connection:
                connection.execute("DROP TRIGGER qualification_delay_reconciliation")
        self.hubu.stop()
        self.hubu.start()  # independent startup observation writes both entries
        deadline = time.monotonic() + 5
        while True:
            opened = self.summary(filters)
            if opened["backlog"]["unresolved_reconciliations"] == 2 or time.monotonic() >= deadline:
                break
            time.sleep(0.02)
        check(opened["backlog"]["unresolved_reconciliations"] == 2, "startup collector missed expired claims")
        check(self.summary(filters, snapshot=frozen["snapshot"]["token"]) == frozen, "late observation rewrote frozen snapshot")
        _, rows = self.records(filters, opened)
        entries = [row for row in rows if row["outcome"] == "reconciliation_required"]
        old_boundary = parse_instant(frozen["snapshot"]["captured_evidence_as_of"])
        check(len(entries) == 2 and
              sorted(parse_instant(row["occurred_at"]) for row in entries) ==
              sorted(parse_instant(claim["claim_expires_at"]) for claim in claims) and
              all(parse_instant(row["occurred_at"]) < old_boundary <
                  parse_instant(row["recorded_at"]) for row in entries),
              "late entries did not retain canonical expiry and actual observation times")
        # Backlog is scoped capture at the snapshot, including openings before
        # the requested event period. An empty event window must still show it.
        period_start = dt.datetime.now(dt.timezone.utc)
        empty_period = {**filters, "occurred_from": instant(period_start),
                        "occurred_before": instant(period_start + dt.timedelta(microseconds=1))}
        preperiod, empty_rows = self.records(empty_period)
        check(not empty_rows and preperiod["fact_count"] == 0 and
              preperiod["backlog"]["unresolved_reconciliations"] == 2,
              "event period incorrectly removed pre-period reconciliation backlog")
        for claim, billed in zip(claims, [True, False]):
            payload = {"claim_id": claim["claim_id"], "provider_reference": PRIVATE,
                       "evidence": PRIVATE + "_CONFIRMED_BILLING"}
            if billed:
                payload["receipt"] = self.receipt()
            self.http("POST", "/spend/executor/" + ("settle" if billed else "release"), payload,
                      headers={"X-Hubu-Reconciliation-Capability": base.RECONCILIATION_TOKEN})
        closed = self.summary(filters)
        check(closed["backlog"]["unresolved_reconciliations"] == 0, "confirmed reconciliation did not close backlog")
        check(stages(closed).get("reconciliation_required") == 2 and stages(closed).get("reconciliation_resolved") == 2, "closure lost historical reconciliation counts")
        check(self.summary(filters, snapshot=opened["snapshot"]["token"]) == opened, "later resolution rewrote frozen open backlog")
        check(self.summary(empty_period, snapshot=preperiod["snapshot"]["token"]) == preperiod,
              "closure changed frozen pre-period backlog")
        check(self.summary(empty_period)["backlog"]["unresolved_reconciliations"] == 0,
              "fresh backlog incorrectly ignored a resolution outside the event period")
        _, rows = self.records(filters, closed)
        resolutions = {row["reconciliation_resolution"] for row in rows if row["outcome"] == "reconciliation_resolved"}
        check(resolutions == {"vendor_billed", "vendor_did_not_bill"}, "human reconciliation branches collapsed")
        return {"frozen_before_observation": frozen, "frozen_open": opened, "preperiod_open": preperiod, "closed": closed}

    def run(self) -> dict:
        agent = self.agent("governance-main", usd="10", usdc="1")
        recovery = self.late_reconciliation(agent)
        filters = {"agent_id": agent["agent_id"]}
        # Positive USDC exact wire, settlement, and independent native budgets.
        usd_before = next(b for b in self.http("GET", "/budgets")["budgets"] if b["agent_id"] == agent["agent_id"] and b["currency"] == "usd")
        usdc = self.authorize(agent, "usdc-exact", "0.005", "usdc")
        claim = self.claim(usdc)
        replay = self.http("POST", "/spend/executor/claim", self.claim_requests[usdc["auth_token_id"]])
        check(replay["claim_id"] == claim["claim_id"], "claim replay duplicated a claim")
        receipt = self.receipt("usdc")
        settled = self.http("POST", "/spend/executor/settle", {"claim_id": claim["claim_id"], "receipt": receipt})
        for field, units in [("authorized_max", "5000"), ("budget_charge", "3101"),
                             ("released_amount", "1899"), ("overrun_amount", "0")]:
            money = settled["receipt"][field]
            check((money["units"], money["scale"], money["currency"]) == (units, 6, "usdc"),
                  "USDC settlement used a cent projection or another currency")
        settlement_replay = self.http("POST", "/spend/executor/settle", {"claim_id": claim["claim_id"], "receipt": receipt})
        check(settlement_replay["receipt"] == settled["receipt"], "settlement replay changed its immutable receipt")
        budgets_after = self.http("GET", "/budgets")["budgets"]
        usd_after = next(b for b in budgets_after if b["budget_id"] == usd_before["budget_id"])
        check(usd_after == usd_before, "USDC settlement changed the USD budget")
        usdc_after = next(b for b in budgets_after if b["agent_id"] == agent["agent_id"] and b["currency"] == "usdc")
        for field, units in [("consumed_amount", "3101"), ("frozen_amount", "0"),
                             ("remaining_amount", "996899")]:
            money = usdc_after[field]
            check((money["units"], money["scale"], money["currency"]) == (units, 6, "usdc"),
                  f"USDC budget disagreed with exact settlement: {field}")
        # Safe releases have different facts and cannot imply provider failure.
        voided = self.authorize(agent, "safe-void", "0.004")
        self.http("POST", "/spend/void", {"spend_auth_token_id": voided["auth_token_id"], "account_id": agent["account_id"], "operation_key": voided["operation_key"]})
        released = self.claim(self.authorize(agent, "safe-release", "0.004"))
        self.http("POST", "/spend/executor/release", {"claim_id": released["claim_id"]})
        approved = self.authorize(agent, "human-approved", "0.006")
        declined = self.authorize(agent, "human-declined", "0.007")
        check(approved["decision"] == declined["decision"] == "needs_approval", "human requests became automatic decisions")
        self.resolve_human(approved, "approve")
        self.resolve_human(declined, "deny")
        # A corrected revision remains one operation with two immutable attempts.
        before_correction = self.summary(filters)
        denied = self.authorize(agent, "corrected", "0.03")
        corrected = self.authorize(agent, "corrected", "0.004")
        check(denied["revision"] == 1 and corrected["revision"] == 2, "corrected request did not create a new revision")
        prior, rows = self.records(filters)
        check(prior["distinct_attempts"] == before_correction["distinct_attempts"] + 2 and
              prior["distinct_operations"] == before_correction["distinct_operations"] + 1 and
              prior["fact_count"] == before_correction["fact_count"] + 5,
              "corrected revisions confused facts, attempts, or logical operations")
        replay = self.authorize(agent, "corrected", "0.004")
        check(replay["revision"] == 2, "exact replay created another revision")
        check(self.records(filters)[1] == rows, "authorization replay duplicated or changed evidence")
        # Changing today's configuration must not rewrite captured explanation.
        denial = next(row for row in rows if row["automatic_denial_reason"] == "policy")
        saved = self.detail(filters, prior, denial)
        budget_record = next(row for row in rows if row["budget"].get("value", {}).get("budget_id") == usd_before["budget_id"])
        saved_budget = self.detail(filters, prior, budget_record)
        self.policy(agent, POLICY.replace('money: "0.02"', 'money: "0.01"'))
        self.http("POST", f"/budgets/{usd_before['budget_id']}/versions", {"amount_limit": "11", "expected_revision": 1})
        current = self.summary(filters)
        check(self.detail(filters, current, denial) == saved and
              self.detail(filters, current, budget_record) == saved_budget,
              "later policy/budget edit changed historical explanation")
        # A genuine human request can fail approval without confirming closure.
        poor = self.agent("governance-poor", usd="0.004")
        insufficient = self.authorize(poor, "budget-deny", "0.005")
        check(insufficient["decision"] == "deny", "insufficient budget was not denied")
        pending = self.authorize(poor, "failed-human", "0.006")
        self.resolve_human(pending, "approve", expected=400)
        poor_summary = self.summary({"agent_id": poor["agent_id"]})
        check(poor_summary["backlog"]["pending_human_approvals"] == 1 and stages(poor_summary).get("human_approved", 0) == 0 and stages(poor_summary).get("authorized", 0) == 0, "failed approval published a human confirmation or authorization")
        check(self.authorize(poor, "missing-currency", "0.005", "usdc")["decision"] == "deny", "request used a different currency's budget")
        # Checked wide aggregates are exercised through real public writes.
        large = self.agent("governance-wide", policy="id: wide\nversion: v1\ndefault_effect: deny\nrules: []\n")
        for i in range(2):
            self.authorize(large, f"large-{i}", "9223372036854.775807")
        self.authorize(large, "separate-usdc", "0.005", "usdc")
        wide = self.summary({"agent_id": large["agent_id"]})
        totals = {m["currency"]: m["units"] for m in wide["requested"]["per_currency"]}
        check(totals == {"usd": "18446744073709551614", "usdc": "5000"}, "wide totals rounded, overflowed, or combined currencies")
        # Explicit period answers and their complete supporting record set.
        now = dt.datetime.now(dt.timezone.utc)
        week = {"occurred_from": instant(now - dt.timedelta(days=7)), "occurred_before": instant(now), "outcomes": ["automatic_deny"]}
        weekly, denied_rows = self.records(week)
        breakdown = {row["reason"]: row["distinct_attempts"] for row in weekly["automatic_denials"]}
        check(breakdown == {"policy": 5, "insufficient_budget": 1, "no_available_budget": 1}, f"weekly denial source counts wrong: {breakdown}")
        for row in denied_rows:
            self.detail(week, weekly, row)
        self.parity(week, weekly)
        intervention_filters = {"outcomes": ["human_approved", "human_declined"]}
        interventions, intervention_rows = self.records(intervention_filters)
        for row in intervention_rows:
            self.detail(intervention_filters, interventions, row)
        check(stages(interventions) == {"human_approved": 1, "human_declined": 1}, "human answers collapsed approve and decline")
        all_summary, all_rows = self.records({})
        check(all(stages(all_summary).get(outcome) == count for outcome, count in
                  [("policy_needs_approval", 3), ("escalated", 3),
                   ("human_approved", 1), ("human_declined", 1)]),
              "needs_approval, escalation, and confirmed human branches collapsed")
        check(all_summary["backlog"]["pending_human_approvals"] == 1 and all_summary["backlog"]["unresolved_reconciliations"] == 0, "current captured backlog answer is wrong")
        check(set(all_summary["unsupported_signals"]) == {"provider_execution_failure", "pre_expiry_billing_uncertainty"}, "unsupported provider history appeared complete")
        check(all(not item["complete_history"] for item in all_summary["record_coverage"]), "new capture claimed complete historical coverage")
        check(not any(row["outcome"] == "execution_failed" for row in all_rows), "release invented provider failure")
        frozen = all_summary["snapshot"]["token"]
        self.hubu.stop()
        self.hubu.start()
        check(self.summary({}, snapshot=frozen) == all_summary, "restart changed frozen history")
        check(self.records({})[1] == all_rows, "restart duplicated or rewrote facts")
        # A different owner cannot reuse old scope, tokens, or supporting IDs.
        self.http("POST", "/init", {"username": "other-governance", "display_name": "Other synthetic owner"})
        self.read("hubu_governance_summary", filters, error=True)
        self.read("hubu_governance_summary", {"snapshot": frozen}, error=True)
        self.read("hubu_get_governance_record", {"record_id": denial["record_id"]}, error=True)
        check(self.summary({})["fact_count"] == 0, "another owner's records appeared in the new owner")
        serialized = json.dumps(self.read_outputs)
        for value in [*self.private, "operation_key", "correlation", "source_evidence", "auth_token_id", "spend_auth_token_id", "provider_request_id", "reconciliation_evidence"]:
            check(value not in serialized, f"governance reads leaked private field/value {value[:80]!r}")
        return {"contract": self.contract, "provisional": self.contract != "v4.5", "build": self.build,
                "weekly_denials": {"filters": week, "summary": weekly, "supporting_records": denied_rows},
                "human_interventions": {"summary": interventions, "supporting_records": intervention_rows}, "current_backlog": all_summary["backlog"],
                "reconciliation_snapshots": recovery, "wide_totals": totals,
                "usdc_settlement": {"receipt": settled["receipt"], "budget": usdc_after,
                                    "usd_budget_unchanged": usd_after == usd_before},
                "gongbu_precision_qualification": "covered separately by existing Gongbu/Amount Precision suites",
                "component_proofs_reused": ["governance_query_tests", "coverage_inventory tests", "accounting_scale_tests", "financial_lifecycle_tests"],
                "read_calls": len(self.read_outputs)}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--hubu-server-bin", type=Path, required=True)
    parser.add_argument("--unified-mcp-bin", type=Path, required=True)
    parser.add_argument("--expected-contract", choices=["v4.4", "v4.5"], default="v4.5")
    parser.add_argument("--keep-state", action="store_true")
    args = parser.parse_args()
    root = Path(tempfile.mkdtemp(prefix="hubu-governance-qualification-"))
    qualification = None
    try:
        qualification = Qualification(args.hubu_server_bin, args.unified_mcp_bin, root, args.expected_contract)
        report = qualification.run()
        (root / "qualification.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report, sort_keys=True), flush=True)
    except Exception as error:
        print(f"governance qualification failed: {error}\ndiagnostic state preserved at {root}", file=sys.stderr)
        return 1
    finally:
        if qualification:
            qualification.close()
    if args.keep_state:
        print(f"diagnostic state: {root}", file=sys.stderr)
    else:
        shutil.rmtree(root)
    print(f"governance observability qualification passed ({args.expected_contract}{' provisional' if args.expected_contract == 'v4.4' else ''})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
