#!/usr/bin/env python3
"""Upgrade drill for the v0.2.4 micro-unit migration on a COPY of a real Hubu DB.

The drill never touches the live database. Make the copy first with SQLite's
online backup, for example:

    sqlite3 -readonly ~/.../state/hubu/hubu.sqlite3 ".backup '/tmp/drill/hubu.sqlite3'"

Then run:

    python3 scripts/upgrade-drill-amount-precision.py \\
        --old-server-bin ~/.local/bin/hubu-server \\
        --new-server-bin target/debug/hubu-server \\
        --hubu-db /tmp/drill/hubu.sqlite3 [--report drill.json]

Phases:

1. The old server records the pre-upgrade budgets and ledger coverage, then
   creates in-flight state in a fresh drill agent: a claimed hold, a claim
   whose short lease expires into reconciliation, a budget update and a spend
   authorization whose requests are kept for replay.
2. The new server migrates the copy. The drill checks the backup, exact
   ×10^4 conversion, unchanged coverage, settles the in-flight hold with a
   sub-cent actual, reconciles the expired claim, and replays the pre-upgrade
   budget update and authorization.
3. The new server reopens the copy; the drill checks that nothing changed.
"""

from __future__ import annotations

import argparse
import json
import os
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

AUTH = "hubu-upgrade-drill-bearer"
RECONCILIATION = "hubu-upgrade-drill-reconciliation"
LEASE_CONFIG = """authorization_ttl_seconds: 300
default_lease_profile: default
lease_profiles:
  default:
    claim_ttl_seconds: 900
  drill_short:
    claim_ttl_seconds: 1
"""
SCOPE = {
    "schema_version": 1,
    "provider": "provider:local:fixture",
    "executor": "executor:gongbu:image",
    "capability": "capability:image:generate",
    "billing_merchant": "merchant:local",
}
DRILL_POLICY = """id: upgrade_drill
version: v1
default_effect: deny
rules:
  - id: allow_fixture_provider
    effect: allow
    reason: upgrade drill fixture provider
    when:
      op: eq
      field: provider
      value:
        string: provider:local:fixture
"""


class DrillFailure(AssertionError):
    pass


def check(condition: bool, message: str) -> None:
    if not condition:
        raise DrillFailure(message)


class Server:
    def __init__(self, binary: Path, database: Path, workdir: Path, label: str):
        lease = workdir / "lease.yaml"
        lease.write_text(LEASE_CONFIG)
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            address = f"127.0.0.1:{probe.getsockname()[1]}"
        self.base_url = f"http://{address}"
        env = dict(
            os.environ,
            HUBU_DB_PATH=str(database),
            HUBU_AUTH_TOKEN=AUTH,
            HUBU_RECONCILIATION_TOKEN=RECONCILIATION,
            HUBU_LEASE_CONFIG=str(lease),
        )
        for name in ("HUBU_AUTH_TOKEN_FILE", "HUBU_RECONCILIATION_TOKEN_FILE"):
            env.pop(name, None)
        self.log_path = workdir / f"{label}.log"
        self.log = open(self.log_path, "w")
        self.process = subprocess.Popen([str(binary), address], env=env, stdout=self.log, stderr=subprocess.STDOUT)
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            check(self.process.poll() is None, f"{label} server exited: {self.log_path.read_text()[-2000:]}")
            try:
                if self.request("GET", "/health")[0] == 200:
                    return
            except OSError:
                pass
            time.sleep(0.1)
        raise DrillFailure(f"{label} server did not become ready")

    def request(self, method: str, path: str, body: Any = None, headers: dict[str, str] | None = None) -> tuple[int, Any]:
        request_headers = {"Accept": "application/json", "Authorization": f"Bearer {AUTH}", **(headers or {})}
        data = None
        if body is not None:
            data = json.dumps(body).encode()
            request_headers["Content-Type"] = "application/json"
        request = urllib.request.Request(self.base_url + path, method=method, data=data, headers=request_headers)
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                status, raw = response.status, response.read()
        except urllib.error.HTTPError as error:
            status, raw = error.code, error.read()
        return status, (json.loads(raw) if raw else None)

    def ok(self, method: str, path: str, body: Any = None, **kwargs: Any) -> Any:
        status, response = self.request(method, path, body, **kwargs)
        check(status == 200, f"{method} {path} -> {status}: {response}")
        return response

    def stop(self) -> None:
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=10)
        self.log.close()


def coverage(server: Server, budgets: list[dict[str, Any]]) -> dict[str, Any]:
    result = {}
    for budget in budgets:
        page = server.ok(
            "GET",
            f"/ledger/transactions?agent_id={budget['agent_id']}&budget_id={budget['budget_id']}&limit=100",
        )
        result[budget["budget_id"]] = page["coverage"]
    return result


def amounts(database: Path) -> dict[str, Any]:
    """A deterministic fingerprint of every stored amount, for idempotency."""
    connection = sqlite3.connect(f"file:{database}?mode=ro", uri=True)
    try:
        snapshot = {}
        for table in ("budget_versions", "budget_balances", "budget_holds", "spend_executor_settlement_receipts", "spending_targets"):
            columns = [row[1] for row in connection.execute(f"PRAGMA table_info({table})")]
            wanted = [column for column in columns if "amount" in column or "cents" in column or column == "budget_charge"]
            if wanted:
                rows = connection.execute(f"SELECT {','.join(wanted)} FROM {table} ORDER BY rowid").fetchall()
                snapshot[table] = {"columns": wanted, "rows": rows}
        return snapshot
    finally:
        connection.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--old-server-bin", type=Path, required=True)
    parser.add_argument("--new-server-bin", type=Path, required=True)
    parser.add_argument("--hubu-db", type=Path, required=True, help="a COPY of the Hubu database")
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    database = args.hubu_db.resolve()
    check("/profiles/" not in str(database), "refusing to run on a profile database; use a backup copy")
    workdir = Path(tempfile.mkdtemp(prefix="hubu-upgrade-drill-"))
    report: dict[str, Any] = {"schema": "hubu-amount-precision-upgrade-drill-v1", "database_copy": str(database)}

    # -- Phase 1: pre-upgrade state with the old binary -------------------
    old = Server(args.old_server_bin, database, workdir, "old")
    try:
        report["old_version"] = old.ok("GET", "/version")
        before = old.ok("GET", "/budgets?all=true")["budgets"]
        report["before_budgets"] = before
        report["before_coverage"] = coverage(old, before)
        agent = old.ok("POST", "/agents/register", {"name": f"upgrade-drill-{int(time.time())}", "version": "v1"})
        old.ok("POST", "/policies", {"agent_id": agent["agent_id"], "policy_yaml": DRILL_POLICY})
        budget = old.ok(
            "POST",
            "/budgets",
            {"agent_id": agent["agent_id"], "amount_cents": 1000, "ending_before": "2999-01-01T00:00:00Z"},
        )["budget"]
        update = {"amount_limit_cents": 1200, "expected_revision": budget["current_revision"], "reason": "upgrade drill"}
        old.ok("POST", f"/budgets/{budget['budget_id']}/versions", update)

        def authorize(server: Server, key: str, cents: int, lease: str = "default") -> dict[str, Any]:
            return server.ok(
                "POST",
                "/spend/authorize",
                {
                    "operation_key": key,
                    "account_id": agent["account_id"],
                    "amount_cents": cents,
                    "reason": "upgrade drill",
                    "lease_profile": lease,
                    "execution_scope": SCOPE,
                },
            )

        def claim(server: Server, authorization: dict[str, Any], cents: int) -> str:
            resolved = server.ok("POST", "/spend/executor/resolve", {"spend_auth_token_id": authorization["auth_token_id"]})
            return server.ok(
                "POST",
                "/spend/executor/claim",
                {
                    "spend_auth_token_id": authorization["auth_token_id"],
                    "account_id": agent["account_id"],
                    "amount_cents": cents,
                    "execution_scope": resolved["execution_scope"],
                },
            )["claim_id"]

        in_flight = authorize(old, "drill:in-flight", 5)
        in_flight_claim = claim(old, in_flight, 5)
        expiring = authorize(old, "drill:expiring", 3, lease="drill_short")
        expiring_claim = claim(old, expiring, 3)
        replay_authorization = authorize(old, "drill:replay", 2)
        time.sleep(2)
        report["drill_agent"] = agent
        report["pre_upgrade_holds"] = {
            "in_flight_claim": in_flight_claim,
            "expiring_claim": expiring_claim,
            "replay_authorization": replay_authorization["auth_token_id"],
        }
    finally:
        old.stop()

    # -- Phase 2: upgrade with the new binary -----------------------------
    new = Server(args.new_server_bin, database, workdir, "new")
    try:
        report["new_version"] = new.ok("GET", "/version")
        backup = Path(f"{database}.pre-accounting-scale.bak")
        check(backup.exists(), f"missing migration backup {backup}")
        report["backup"] = {"path": str(backup), "bytes": backup.stat().st_size}
        after = new.ok("GET", "/budgets?all=true")["budgets"]
        by_id = {b["budget_id"]: b for b in after}
        for previous in before:
            current = by_id[previous["budget_id"]]
            for field in ("amount_limit", "consumed_amount", "frozen_amount", "remaining_amount"):
                check(
                    int(current[field]["units"]) == previous[f"{field}_cents"] * 10_000,
                    f"{previous['budget_id']} {field}: {previous[f'{field}_cents']}c -> {current[field]}",
                )
                check(current[f"{field}_cents"] == previous[f"{field}_cents"], f"{field}_cents changed")
        coverage_after = coverage(new, before)
        for budget_id, prior in report["before_coverage"].items():
            for field in ("consumed_amount_cents", "recorded_budget_charges_cents", "unaccounted_consumption_cents"):
                check(coverage_after[budget_id][field] == prior[field], f"coverage {budget_id} {field} changed")
            check(coverage_after[budget_id]["unaccounted_consumption"]["units"] == str(prior["unaccounted_consumption_cents"] * 10_000), "coverage units")
        report["after_coverage"] = coverage_after

        receipt = {
            "actual_vendor_cost": {"amount": 31, "scale": 4, "currency": "usd"},
            "provider_request_id": "upgrade-drill-in-flight",
            "price_model_snapshot": {"provider": "fixture", "model": "fixture", "currency": "usd"},
            "artifact_reference": "artifact://upgrade-drill",
        }
        settled = new.ok("POST", "/spend/executor/settle", {"claim_id": in_flight_claim, "receipt": receipt})
        check(settled["receipt"]["budget_charge"]["units"] == "3100", f"{settled['receipt']}")
        check(settled["receipt"]["released_amount"]["units"] == "46900", f"{settled['receipt']}")
        report["in_flight_settlement"] = settled["receipt"]

        expired = new.ok("GET", f"/spend/executor/claim?claim_id={expiring_claim}")
        check(expired["reconciliation_required"], f"claim did not expire into reconciliation: {expired}")
        reconciled = new.ok(
            "POST",
            "/spend/executor/settle",
            {
                "claim_id": expiring_claim,
                "provider_reference": "upgrade-drill-vendor",
                "evidence": "Drill fixture usage export.",
                "receipt": {**receipt, "provider_request_id": "upgrade-drill-expired", "actual_vendor_cost": {"amount": 25, "scale": 4, "currency": "usd"}},
            },
            headers={"X-Hubu-Reconciliation-Capability": RECONCILIATION},
        )
        check(reconciled["reconciliation_outcome"] == "vendor_billed", f"{reconciled}")
        report["expired_claim_reconciliation"] = reconciled["spend"]["budget_hold"]

        update_status, replayed_update = new.request("POST", f"/budgets/{budget['budget_id']}/versions", update)
        check(update_status == 200 and replayed_update["idempotent_replay"] is True, f"budget update replay: {update_status} {replayed_update}")
        replayed = authorize(new, "drill:replay", 2)
        check(replayed["idempotent_replay"] is True and replayed["auth_token_id"] == replay_authorization["auth_token_id"], f"{replayed}")
        status, recreated = new.request(
            "POST",
            "/budgets",
            {"agent_id": agent["agent_id"], "amount_cents": 1000, "ending_before": "2999-01-01T00:00:00Z"},
        )
        report["cross_version_replay"] = {
            "budget_update": {"status": update_status, "idempotent_replay": replayed_update["idempotent_replay"]},
            "spend_authorize": {"idempotent_replay": replayed["idempotent_replay"]},
            "budget_create_retry": {"status": status, "note": "budget create has no idempotency key; a retry is rejected as overlapping, never duplicated", "response": recreated},
        }
        drill_budget = new.ok("GET", "/budgets?all=true")["budgets"]
        drill_budget = next(b for b in drill_budget if b["budget_id"] == budget["budget_id"])
        report["drill_budget_after"] = drill_budget
    finally:
        new.stop()
    fingerprint = amounts(database)

    # -- Phase 3: reopen is idempotent ------------------------------------
    backup_bytes = Path(f"{database}.pre-accounting-scale.bak").stat().st_size
    again = Server(args.new_server_bin, database, workdir, "reopen")
    try:
        check(again.ok("GET", "/budgets?all=true")["budgets"] is not None, "budgets after reopen")
    finally:
        again.stop()
    check(amounts(database) == fingerprint, "reopening changed stored amounts")
    check(Path(f"{database}.pre-accounting-scale.bak").stat().st_size == backup_bytes, "reopen rewrote the backup")
    report["reopen_idempotent"] = True

    if args.report:
        args.report.write_text(json.dumps(report, indent=2, default=str) + "\n")
    print(json.dumps({k: report[k] for k in ("backup", "cross_version_replay", "reopen_idempotent")}, indent=2, default=str))
    print("upgrade drill passed")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except DrillFailure as error:
        print(f"upgrade drill FAILED: {error}", file=sys.stderr)
        sys.exit(1)
