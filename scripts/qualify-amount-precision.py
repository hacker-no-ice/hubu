#!/usr/bin/env python3
"""Qualify sub-cent accounting end to end through Hubu's public HTTP API.

Runs the amount-precision design's sub-cent cases (E1-E15) against an
isolated, freshly started ``hubu-server`` and prints a JSON evidence report.
Every request goes through the public HTTP API; the runner never opens Hubu
storage. Per-currency budgets supersede the design's conversion-rate case, so
E12 checks that USDC spend never touches a USD budget instead.

Usage:
    python3 scripts/qualify-amount-precision.py --server-bin target/debug/hubu-server \\
        [--report qualification.json]

Only the Python standard library is used.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from decimal import Decimal
from pathlib import Path
from typing import Any

AUTH = "hubu-qualification-bearer"
RECONCILIATION = "hubu-qualification-reconciliation"
LEASE_CONFIG = """authorization_ttl_seconds: 4
default_lease_profile: default
lease_profiles:
  default:
    claim_ttl_seconds: 900
  short:
    claim_ttl_seconds: 1
"""
SCOPE = {
    "schema_version": 1,
    "provider": "provider:local:fixture",
    "executor": "executor:gongbu:image",
    "capability": "capability:image:generate",
    "billing_merchant": "merchant:local",
}
ALLOW_FIXTURE_POLICY = """id: amount_precision_qualification
version: v1
default_effect: deny
rules:
  - id: allow_fixture_provider
    effect: allow
    reason: amount precision qualification fixture provider
    when:
      op: eq
      field: provider
      value:
        string: provider:local:fixture
"""
CAP_POLICY = """id: half_cent_cap
version: v1
default_effect: deny
rules:
  - id: allow_half_cent_calls
    effect: allow
    reason: calls up to half a cent
    when:
      op: all
      conditions:
        - op: eq
          field: provider
          value:
            string: provider:local:fixture
        - op: lte
          field: amount
          value:
            money: "0.005"
"""


class QualificationFailure(AssertionError):
    pass


def check(condition: bool, message: str) -> None:
    if not condition:
        raise QualificationFailure(message)


class Hubu:
    """A real hubu-server process owned by the runner."""

    def __init__(self, binary: Path):
        self.directory = Path(tempfile.mkdtemp(prefix="hubu-amount-qualification-"))
        (self.directory / "lease.yaml").write_text(LEASE_CONFIG)
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            self.address = f"127.0.0.1:{probe.getsockname()[1]}"
        self.base_url = f"http://{self.address}"
        env = dict(
            os.environ,
            HUBU_DB_PATH=str(self.directory / "hubu.sqlite3"),
            HUBU_AUTH_TOKEN=AUTH,
            HUBU_RECONCILIATION_TOKEN=RECONCILIATION,
            HUBU_LEASE_CONFIG=str(self.directory / "lease.yaml"),
        )
        for name in ("HUBU_AUTH_TOKEN_FILE", "HUBU_RECONCILIATION_TOKEN_FILE"):
            env.pop(name, None)
        self.log = open(self.directory / "hubu-server.log", "w")
        self.process = subprocess.Popen(
            [str(binary), self.address], env=env, stdout=self.log, stderr=subprocess.STDOUT
        )
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            check(self.process.poll() is None, "hubu-server exited before becoming ready")
            try:
                if self.request("GET", "/health", bearer=None)[0] == 200:
                    return
            except OSError:
                pass
            time.sleep(0.05)
        raise QualificationFailure("hubu-server did not become ready")

    def request(
        self,
        method: str,
        path: str,
        body: Any = None,
        *,
        bearer: str | None = AUTH,
        headers: dict[str, str] | None = None,
        query: dict[str, Any] | None = None,
    ) -> tuple[int, Any]:
        url = self.base_url + path
        if query:
            url += "?" + urllib.parse.urlencode(query)
        request_headers = {"Accept": "application/json", **(headers or {})}
        if bearer:
            request_headers["Authorization"] = f"Bearer {bearer}"
        data = None
        if body is not None:
            data = json.dumps(body).encode()
            request_headers["Content-Type"] = "application/json"
        request = urllib.request.Request(url, method=method, data=data, headers=request_headers)
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                status, raw = response.status, response.read()
        except urllib.error.HTTPError as error:
            status, raw = error.code, error.read()
        return status, (json.loads(raw) if raw else None)

    def ok(self, method: str, path: str, body: Any = None, **kwargs: Any) -> Any:
        status, response = self.request(method, path, body, **kwargs)
        check(status == 200, f"{method} {path} returned {status}: {response}")
        return response

    def stop(self, keep: bool) -> None:
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait(timeout=10)
        self.log.close()
        if keep:
            print(f"Hubu state preserved at {self.directory}", file=sys.stderr)
        else:
            shutil.rmtree(self.directory, ignore_errors=True)


class Qualification:
    def __init__(self, hubu: Hubu):
        self.hubu = hubu
        self.sequence = 0
        hubu.ok(
            "POST",
            "/init",
            {
                "username": "amount-qualification",
                "display_name": "Amount Qualification",
                "email": "amount-qualification@example.invalid",
            },
        )
        hubu.ok("POST", "/policies", {"policy_yaml": ALLOW_FIXTURE_POLICY})

    # -- fixtures --------------------------------------------------------

    def agent(self, name: str, budget: str | None, currency: str = "usd") -> dict[str, str]:
        agent = self.hubu.ok("POST", "/agents/register", {"name": name, "version": "v1"})
        result = {"agent_id": agent["agent_id"], "account_id": agent["account_id"]}
        if budget is not None:
            created = self.hubu.ok(
                "POST",
                "/budgets",
                {
                    "agent_id": agent["agent_id"],
                    "amount": budget,
                    "currency": currency,
                    "ending_before": "2999-01-01T00:00:00Z",
                },
            )
            result["budget_id"] = created["budget"]["budget_id"]
        return result

    def budget(self, agent: dict[str, str], currency: str = "usd") -> dict[str, Any]:
        budgets = self.hubu.ok("GET", "/budgets")["budgets"]
        return next(
            b for b in budgets if b["agent_id"] == agent["agent_id"] and b["currency"] == currency
        )

    def authorize(
        self, agent: dict[str, str], amount: str, currency: str = "usd", lease: str = "default"
    ) -> tuple[int, Any]:
        self.sequence += 1
        return self.hubu.request(
            "POST",
            "/spend/authorize",
            {
                "operation_key": f"qualification:{self.sequence}",
                "account_id": agent["account_id"],
                "amount": amount,
                "currency": currency,
                "reason": "amount precision qualification",
                "lease_profile": lease,
                "execution_scope": SCOPE,
            },
        )

    def allowed(self, agent: dict[str, str], amount: str, **kwargs: Any) -> dict[str, Any]:
        status, response = self.authorize(agent, amount, **kwargs)
        check(status == 200 and response["decision"] == "allow", f"{amount} not allowed: {response}")
        return response

    def claim(self, agent: dict[str, str], authorization: dict[str, Any]) -> str:
        resolved = self.hubu.ok(
            "POST", "/spend/executor/resolve", {"spend_auth_token_id": authorization["auth_token_id"]}
        )
        claim = self.hubu.ok(
            "POST",
            "/spend/executor/claim",
            {
                "spend_auth_token_id": authorization["auth_token_id"],
                "account_id": agent["account_id"],
                "contract_version": "hubu-spend-executor-v4.5",
                "amount": resolved["amount"],
                "execution_scope": resolved["execution_scope"],
            },
        )
        return claim["claim_id"]

    @staticmethod
    def receipt(amount: int, scale: int, currency: str = "usd") -> dict[str, Any]:
        return {
            "actual_vendor_cost": {"amount": amount, "scale": scale, "currency": currency},
            "provider_request_id": f"qualification-request-{amount}-{scale}-{time.monotonic_ns()}",
            "price_model_snapshot": {"provider": "fixture", "model": "fixture", "currency": currency},
            "artifact_reference": "artifact://qualification",
        }

    def settle(self, claim_id: str, amount: int, scale: int, currency: str = "usd") -> tuple[int, Any]:
        return self.hubu.request(
            "POST",
            "/spend/executor/settle",
            {"claim_id": claim_id, "receipt": self.receipt(amount, scale, currency)},
        )

    def spend(self, agent: dict[str, str], amount: str, actual: tuple[int, int]) -> dict[str, Any]:
        authorization = self.allowed(agent, amount)
        status, settled = self.settle(self.claim(agent, authorization), *actual)
        check(status == 200, f"settlement failed: {settled}")
        return settled

    def ledger_rounding(self, agent: dict[str, str]) -> tuple[Decimal, int, dict[str, Any]]:
        """Sum the ledger rounding deltas (budget charge - exact cost) for a budget."""
        total, count, cursor, coverage = Decimal(0), 0, None, {}
        while True:
            query = {"agent_id": agent["agent_id"], "budget_id": agent["budget_id"], "limit": 100}
            if cursor:
                query["cursor"] = cursor
            page = self.hubu.ok("GET", "/ledger/transactions", query=query)
            coverage = page["coverage"]
            for transaction in page["transactions"]:
                # The ledger reports the rounding delta as an integer string
                # at scale 18.
                rounding = transaction.get("rounding_delta_amount")
                if rounding is not None:
                    total += Decimal(rounding).scaleb(-18)
                count += 1
            cursor = page.get("next_cursor")
            if not cursor:
                return total, count, coverage

    # -- cases -----------------------------------------------------------

    def e1(self) -> dict[str, Any]:
        agent = self.agent("e1-thousandth", "1.00")
        for _ in range(1_000):
            self.spend(agent, "0.001", (1, 3))
        status, denied = self.authorize(agent, "0.001")
        check(status == 200 and denied["decision"] == "deny", f"1001st call not denied: {denied}")
        budget = self.budget(agent)
        check(budget["consumed_amount"]["units"] == "1000000", f"consumed {budget['consumed_amount']}")
        self.e1_agent = agent
        return {"settled_calls": 1_000, "call_1001": denied["reasons"], "consumed": budget["consumed_amount"]}

    def e2(self) -> dict[str, Any]:
        agent = self.agent("e2-half-cent", "1.00")
        for _ in range(200):
            self.allowed(agent, "0.005")
        status, denied = self.authorize(agent, "0.005")
        check(status == 200 and denied["decision"] == "deny", "201st half-cent call not denied")
        return {"allowed": 200, "call_201": denied["reasons"], "frozen": self.budget(agent)["frozen_amount"]}

    def e3(self) -> dict[str, Any]:
        agent = self.agent("e3-sub-cent-holds", "0.01")
        holds = [self.allowed(agent, "0.003") for _ in range(3)]
        status, denied = self.authorize(agent, "0.003")
        check(status == 200 and denied["decision"] == "deny", "4th 0.003 hold not denied")
        budget = self.budget(agent)
        check(budget["remaining_amount"]["units"] == "1000", f"remaining {budget['remaining_amount']}")
        return {"granted": len(holds), "fourth": denied["reasons"], "remaining": budget["remaining_amount"]}

    def e4(self) -> dict[str, Any]:
        agent = self.agent("e4-capped", "1.00")
        self.hubu.ok("POST", "/policies", {"agent_id": agent["agent_id"], "policy_yaml": CAP_POLICY})
        outcomes = {}
        for amount in ("0.005", "0.005001"):
            status, response = self.authorize(agent, amount)
            check(status == 200, f"{amount}: {response}")
            outcomes[amount] = response["decision"]
        status, response = self.authorize(agent, "0.0050001", currency="usdc")
        outcomes["0.0050001 usdc"] = f"rejected {status}: {response.get('error')}"
        check(outcomes["0.005"] == "allow" and outcomes["0.005001"] != "allow", f"{outcomes}")
        check(status == 400 and "6 decimal places" in response.get("error", ""), f"{response}")
        return outcomes

    def e5_e6_e7(self) -> dict[str, Any]:
        agent = self.agent("e5-settlement", "1.00")
        e5 = self.spend(agent, "0.005", (31, 4))["receipt"]
        check(e5["budget_charge"]["units"] == "3100" and e5["released_amount"]["units"] == "1900", f"{e5}")
        check(e5["actual_vendor_cost"]["amount"] in (31, "31") and e5["actual_vendor_cost"]["scale"] == 4, f"{e5}")
        e6 = self.spend(agent, "0.005", (0, 2))["receipt"]
        check(e6["budget_charge"]["units"] == "0" and e6["released_amount"]["units"] == "5000", f"{e6}")
        e7 = self.spend(agent, "0.005", (1, 7))["receipt"]
        check(e7["budget_charge"]["units"] == "1", f"{e7}")
        budget = self.budget(agent)
        check(budget["frozen_amount"]["units"] == "0", f"{budget}")
        check(budget["consumed_amount"]["units"] == "3101", f"{budget}")
        rounding, transactions, coverage = self.ledger_rounding(agent)
        # E7: charge 1 µ for 0.0000001 → rounding delta 0.0000009 (9e11 at scale 18).
        check(rounding == Decimal("0.0000009"), f"rounding {rounding}")
        return {
            "E5": {"budget_charge": e5["budget_charge"], "released": e5["released_amount"], "exact": e5["actual_vendor_cost"]},
            "E6": {"budget_charge": e6["budget_charge"], "released": e6["released_amount"]},
            "E7": {"budget_charge": e7["budget_charge"], "ledger_rounding_delta": str(rounding)},
            "ledger_transactions": transactions,
            "coverage_unaccounted": coverage.get("unaccounted_consumption"),
        }

    def e8(self) -> dict[str, Any]:
        agent = self.agent("e8-at-maximum", "1.00")
        exact = self.spend(agent, "0.005", (5000, 6))["receipt"]
        check(exact["released_amount"]["units"] == "0", f"{exact}")
        authorization = self.allowed(agent, "0.005")
        status, over = self.settle(self.claim(agent, authorization), 5001, 6)
        check(status != 200 and "exceeds" in json.dumps(over), f"{status} {over}")
        return {"at_hold_released": exact["released_amount"], "hold_plus_1_micro": f"{status}: {over.get('error')}"}

    def e9(self) -> dict[str, Any]:
        agent = self.agent("e9-reconciled-overrun", "0.006")
        authorization = self.allowed(agent, "0.005", lease="short")
        claim_id = self.claim(agent, authorization)
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            claim = self.hubu.ok("GET", "/spend/executor/claim", query={"claim_id": claim_id})
            if claim["reconciliation_required"]:
                break
            time.sleep(0.25)
        check(claim["reconciliation_required"], "claim never required reconciliation")
        reconciled = self.hubu.ok(
            "POST",
            "/spend/executor/settle",
            {
                "claim_id": claim_id,
                "provider_reference": "qualification-vendor-reference",
                "evidence": "Fixture usage export confirms the billed request.",
                "receipt": self.receipt(51, 4),
            },
            headers={"X-Hubu-Reconciliation-Capability": RECONCILIATION},
        )
        check(reconciled["reconciliation_outcome"] == "vendor_billed", f"{reconciled}")
        hold = reconciled["spend"]["budget_hold"]
        # The 0.0051 billed cost overruns the 0.005 hold by 100 µ.
        check(hold["consumed_amount"]["units"] == "5100", f"{hold}")
        budget = self.budget(agent)
        check(budget["remaining_amount"]["units"] == "900", f"{budget['remaining_amount']}")
        status, denied = self.authorize(agent, "0.001")
        check(denied["decision"] == "deny", f"{denied}")
        return {
            "hold": hold["amount"],
            "consumed": hold["consumed_amount"],
            "overrun_units": int(hold["consumed_amount"]["units"]) - int(hold["amount"]["units"]),
            "remaining": budget["remaining_amount"],
            "next_0.001": denied["decision"],
        }

    def e10(self) -> dict[str, Any]:
        agent = self.agent("e10-concurrent", "0.007")
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            results = list(pool.map(lambda amount: self.authorize(agent, amount), ["0.003", "0.004"]))
        check(all(s == 200 and r["decision"] == "allow" for s, r in results), f"{results}")
        status, third = self.authorize(agent, "0.000001")
        check(third["decision"] == "deny", f"{third}")
        return {"concurrent": [r["decision"] for _, r in results], "third": third["decision"], "remaining": self.budget(agent)["remaining_amount"]}

    def e11(self) -> dict[str, Any]:
        agent = self.agent("e11-limit-update", "1.00")
        self.spend(agent, "0.005", (31, 4))
        budget = self.budget(agent)
        path = f"/budgets/{budget['budget_id']}/versions"
        revision = budget["current_revision"]
        status, below = self.hubu.request("POST", path, {"amount_limit": "0.003", "expected_revision": revision})
        check(status in (400, 409, 422) and below.get("error_code") == "budget_limit_below_committed", f"{status} {below}")
        updated = self.hubu.ok("POST", path, {"amount_limit": "0.0031", "expected_revision": revision})
        check(updated["current_budget"]["remaining_amount"]["units"] == "0", f"{updated['current_budget']}")
        return {"0.003": f"{status} {below.get('error_code') or below.get('error')}", "0.0031": updated["current_budget"]["amount_limit"]}

    def e12(self) -> dict[str, Any]:
        agent = self.agent("e12-per-currency", "1.00", currency="usd")
        status, denied = self.authorize(agent, "0.005", currency="usdc")
        check(denied["decision"] == "deny" and "no active USDC budget found for agent" in denied["reasons"], f"{denied}")
        self.hubu.ok(
            "POST",
            "/budgets",
            {"agent_id": agent["agent_id"], "amount": "1", "currency": "usdc", "ending_before": "2999-01-01T00:00:00Z"},
        )
        settled = self.spend_usdc(agent)
        usd = self.budget(agent, "usd")
        usdc = self.budget(agent, "usdc")
        check(usd["consumed_amount"]["units"] == "0" and usdc["consumed_amount"]["units"] == "3100", f"{usd} {usdc}")
        return {"usdc_without_usdc_budget": denied["reasons"], "usdc_charge": settled["budget_charge"], "usd_consumed": usd["consumed_amount"], "usdc_consumed": usdc["consumed_amount"]}

    def spend_usdc(self, agent: dict[str, str]) -> dict[str, Any]:
        authorization = self.allowed(agent, "0.005", currency="usdc")
        status, settled = self.settle(self.claim(agent, authorization), 3100, 6, currency="usdc")
        check(status == 200, f"{settled}")
        return settled["receipt"]

    def e13(self) -> dict[str, Any]:
        agent = self.agent("e13-wallet", "1.00")
        self.sequence += 1
        status, wallet = self.hubu.request(
            "POST",
            "/spend",
            {
                "operation_key": f"qualification:{self.sequence}",
                "account_id": agent["account_id"],
                "amount": "0.005",
                "reason": "sub-cent wallet payment",
                "execution_scope": SCOPE,
            },
        )
        budget = self.budget(agent)
        check(budget["consumed_amount"]["units"] == "0", f"wallet consumed budget: {budget}")
        check(budget["frozen_amount"]["units"] == "0", f"wallet left a hold: {budget}")
        warned = self.hubu.ok("POST", "/policies", {"agent_id": agent["agent_id"], "policy_yaml": CAP_POLICY})
        check(any("finer than one cent" in warning for warning in warned.get("warnings", [])), f"{warned}")
        return {"wallet_half_cent": f"{status}: {wallet.get('error') or wallet.get('decision')}", "policy_warnings": warned["warnings"]}

    def e14(self) -> dict[str, Any]:
        agent = self.agent("e14-expiry", "0.01")
        self.allowed(agent, "0.0031")
        check(self.budget(agent)["frozen_amount"]["units"] == "3100", "hold not frozen")
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            budget = self.budget(agent)
            if budget["frozen_amount"]["units"] == "0":
                break
            time.sleep(0.5)
        check(budget["frozen_amount"]["units"] == "0" and budget["remaining_amount"]["units"] == "10000", f"{budget}")
        return {"after_expiry_remaining": budget["remaining_amount"], "frozen": budget["frozen_amount"]}

    def e15(self) -> dict[str, Any]:
        rounding, transactions, coverage = self.ledger_rounding(self.e1_agent)
        check(rounding == 0, f"E1 rounding {rounding}")
        check(coverage["unaccounted_consumption"]["units"] == "0", f"{coverage}")
        check(coverage["recorded_budget_charges"]["units"] == "1000000", f"{coverage}")
        return {"E1_transactions": transactions, "E1_rounding_total": str(rounding), "coverage": {k: coverage[k] for k in ("consumed_amount", "recorded_budget_charges", "unaccounted_consumption")}}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--server-bin", type=Path, required=True)
    parser.add_argument("--report", type=Path)
    parser.add_argument("--keep", action="store_true", help="keep Hubu state after the run")
    args = parser.parse_args()

    hubu = Hubu(args.server_bin)
    report: dict[str, Any] = {"schema": "hubu-amount-precision-qualification-v1", "cases": {}}
    failed = False
    try:
        qualification = Qualification(hubu)
        for case, run in [
            ("E1", qualification.e1),
            ("E2", qualification.e2),
            ("E3", qualification.e3),
            ("E4", qualification.e4),
            ("E5-E7", qualification.e5_e6_e7),
            ("E8", qualification.e8),
            ("E9", qualification.e9),
            ("E10", qualification.e10),
            ("E11", qualification.e11),
            ("E12", qualification.e12),
            ("E13", qualification.e13),
            ("E14", qualification.e14),
            ("E15", qualification.e15),
        ]:
            started = time.monotonic()
            try:
                evidence = run()
                report["cases"][case] = {"status": "pass", "evidence": evidence}
                print(f"PASS {case} ({time.monotonic() - started:.1f}s)")
            except (QualificationFailure, KeyError, TypeError, StopIteration) as error:
                failed = True
                report["cases"][case] = {"status": "fail", "error": repr(error)}
                print(f"FAIL {case}: {error!r}")
    finally:
        hubu.stop(keep=args.keep or failed)
    if args.report:
        args.report.write_text(json.dumps(report, indent=2) + "\n")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
