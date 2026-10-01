#!/usr/bin/env python3
"""Qualify Hubu-only MCP authorization with an external executor (HUB-206).

Starts a real ``hubu-server`` and the ``hubu-unified-mcp`` stdio router with
Gongbu deliberately unconfigured. An agent authorizes through MCP; a minimal
external executor then consumes only the authorization continuation through
Hubu's ``hubu-spend-executor-v4.4`` HTTP API, never the private operation key
or agent. The script asserts that ``hubu_operation_status`` follows Hubu's
authoritative workflow at every stage and fails closed when Hubu is
unreachable.

Only the Python standard library is used. Run it through
``scripts/integration-standalone-mcp-executor.sh``.
"""

from __future__ import annotations

import argparse
import json
import os
import queue
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path

NORMAL_BEARER = "hubu_standalone_normal_bearer"
RECONCILIATION_TOKEN = "hubu_standalone_human_reconciliation"
APPROVAL_TOKEN = "hubu_standalone_human_approval"
LEASE_CONFIG = (
    "authorization_ttl_seconds: 3\n"
    "default_lease_profile: default\n"
    "lease_profiles:\n"
    "  default:\n"
    "    claim_ttl_seconds: 900\n"
    "  conformance_short:\n"
    "    claim_ttl_seconds: 1\n"
)
POLICY = """id: standalone_qualification
version: v1
default_effect: needs_approval
rules:
  - id: deny_large
    effect: deny
    reason: above the standalone qualification limit
    when:
      op: gt
      field: amount
      value:
        money_cents: 900
  - id: allow_small
    effect: allow
    reason: small standalone authorization
    when:
      op: lte
      field: amount
      value:
        money_cents: 500
"""
SCOPE = {
    "schema_version": 1,
    "provider": "provider:local:fixture",
    "executor": "executor:gongbu:image",
    "capability": "capability:image:generate",
    "billing_merchant": "merchant:local",
}
SNAPSHOT = {
    "schema_version": 2,
    "provider": "external-fixture-provider",
    "model": "external-fixture-model",
    "catalog_version": "standalone-qualification",
    "catalog_digest": "sha256:" + "3" * 64,
    "pricing_rule_id": "external-fixture-model",
    "components": [{"unit": "image", "rate_numerator_minor": 3500001, "rate_denominator": 10000, "quantity": 1}],
    "exact_estimate_numerator": "3500001",
    "exact_estimate_denominator": "10000",
    "estimated_amount_minor": 351,
    "currency": "USD",
}
RECEIPT = {
    "actual_vendor_cost": {"amount": 3500001, "scale": 6, "currency": "usd"},
    "provider_request_id": "external-provider-request-1",
    "price_model_snapshot": SNAPSHOT,
    "artifact_reference": "artifact://external/standalone.png",
}
FORBIDDEN_IN_AGENT_OUTPUT = (
    "hubu:operation:v1:",
    "operation_key",
    NORMAL_BEARER,
    RECONCILIATION_TOKEN,
    APPROVAL_TOKEN,
)


class QualificationFailure(AssertionError):
    pass


def check(condition: bool, message: str) -> None:
    if not condition:
        raise QualificationFailure(message)


def free_address() -> str:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return f"127.0.0.1:{probe.getsockname()[1]}"


def http(base: str, method: str, path: str, body=None, headers=None):
    request = urllib.request.Request(
        base + path,
        method=method,
        data=None if body is None else json.dumps(body).encode(),
        headers={"Authorization": f"Bearer {NORMAL_BEARER}", "Content-Type": "application/json", **(headers or {})},
    )
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            return response.status, json.loads(response.read() or b"null")
    except urllib.error.HTTPError as error:
        return error.code, json.loads(error.read() or b"null")


class HubuServer:
    def __init__(self, binary: Path, root: Path):
        self.binary = binary
        self.address = free_address()
        self.base = f"http://{self.address}"
        lease = root / "lease-config.yaml"
        lease.write_text(LEASE_CONFIG)
        self.env = dict(
            os.environ,
            HUBU_DB_PATH=str(root / "hubu.sqlite3"),
            HUBU_AUTH_TOKEN=NORMAL_BEARER,
            HUBU_RECONCILIATION_TOKEN=RECONCILIATION_TOKEN,
            HUBU_APPROVAL_TOKEN=APPROVAL_TOKEN,
            HUBU_LEASE_CONFIG=str(lease),
        )
        for name in ("HUBU_AUTH_TOKEN_FILE", "HUBU_RECONCILIATION_TOKEN_FILE", "HUBU_APPROVAL_TOKEN_FILE"):
            self.env.pop(name, None)
        self.log = root / "hubu-server.log"
        self.process = None
        self.start()

    def start(self) -> None:
        with open(self.log, "a") as log:
            self.process = subprocess.Popen([str(self.binary), self.address], env=self.env, stdout=log, stderr=subprocess.STDOUT)
        for _ in range(200):
            try:
                if http(self.base, "GET", "/health")[0] == 200:
                    return
            except OSError:
                pass
            time.sleep(0.05)
        raise QualificationFailure("hubu-server did not become ready")

    def stop(self) -> None:
        if self.process and self.process.poll() is None:
            self.process.kill()
            self.process.wait(timeout=10)


class UnifiedMcp:
    """Minimal MCP stdio client for the unified router."""

    def __init__(self, binary: Path, hubu: HubuServer, root: Path):
        env = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith(("HUBU_UNIFIED_", "HUBU_APPROVAL_TOKEN", "HUBU_RECONCILIATION_TOKEN"))
        }
        env.update(
            HUBU_UNIFIED_HUBU_ENDPOINT=hubu.base,
            HUBU_UNIFIED_HUBU_BEARER_TOKEN=NORMAL_BEARER,
            HUBU_UNIFIED_OPERATION_STATE_PATH=str(root / "unified-operations.sqlite3"),
            HUBU_UNIFIED_CAPABILITY_POLL_INTERVAL_MS="1000",
            HUBU_UNIFIED_OPERATION_TICK_MS="10",
            HUBU_MCP_TRUST_CLIENT_APPROVAL="1",
            HUBU_MCP_TRUST_SPEND_APPROVAL="1",
            HUBU_APPROVAL_TOKEN=APPROVAL_TOKEN,
            HUBU_RECONCILIATION_TOKEN=RECONCILIATION_TOKEN,
        )
        self.stderr_path = root / "unified-mcp.stderr"
        self.stderr = open(self.stderr_path, "w")
        self.process = subprocess.Popen(
            [str(binary)], env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.stderr, text=True, bufsize=1
        )
        self.messages: queue.Queue = queue.Queue()
        threading.Thread(target=self._read, daemon=True).start()
        self.next_id = 1
        self.agent_outputs: list[str] = []
        self.request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "standalone-qualification", "version": "1"}})
        self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def _read(self) -> None:
        assert self.process.stdout
        for line in self.process.stdout:
            self.messages.put(json.loads(line))

    def _send(self, message: dict) -> None:
        assert self.process.stdin
        self.process.stdin.write(json.dumps(message) + "\n")
        self.process.stdin.flush()

    def request(self, method: str, params: dict) -> dict:
        request_id = self.next_id
        self.next_id += 1
        self._send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            try:
                message = self.messages.get(timeout=1)
            except queue.Empty:
                continue
            if message.get("id") == request_id:
                return message
        raise QualificationFailure(f"MCP {method} timed out")

    def call(self, name: str, arguments: dict, call_id: str | None = None) -> dict:
        params = {"name": name, "arguments": arguments}
        if call_id:
            params["_meta"] = {"callId": call_id}
        response = self.request("tools/call", params)
        self.agent_outputs.append(json.dumps(response))
        check("result" in response, f"{name} failed: {json.dumps(response)[:600]}")
        return response["result"].get("structuredContent") or {}

    def status(self, handle: str) -> dict:
        return self.call("hubu_operation_status", {"operation_handle": handle})

    def close(self) -> None:
        if self.process.poll() is None:
            self.process.stdin.close()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
        self.stderr.close()


class ExternalExecutor:
    """A non-Gongbu executor holding only the continuation it was handed."""

    def __init__(self, hubu: HubuServer):
        self.hubu = hubu

    def claim(self, token: str) -> dict:
        status, snapshot = http(self.hubu.base, "POST", "/spend/executor/resolve", {"spend_auth_token_id": token})
        check(status == 200, f"resolve failed: {snapshot}")
        status, claim = http(
            self.hubu.base,
            "POST",
            "/spend/executor/claim",
            {
                "spend_auth_token_id": token,
                "account_id": snapshot["account_id"],
                "amount_cents": snapshot["amount_cents"],
                "execution_scope": snapshot["execution_scope"],
            },
        )
        check(status == 200 and claim["status"] == "claimed", f"claim failed: {claim}")
        return claim

    def settle(self, token: str) -> dict:
        status, body = http(self.hubu.base, "POST", "/spend/executor/settle", {"spend_auth_token_id": token, "receipt": RECEIPT})
        check(status == 200 and body["status"] == "settled", f"settle failed: {body}")
        return body

    def release(self, claim_id: str) -> dict:
        status, body = http(self.hubu.base, "POST", "/spend/executor/release", {"claim_id": claim_id})
        check(status == 200 and body["status"] == "released", f"release failed: {body}")
        return body


def provision(hubu: HubuServer) -> dict:
    status, _ = http(hubu.base, "POST", "/init", {"username": "standalone", "display_name": "Standalone Qualification", "email": "standalone@example.invalid"})
    check(status == 200, "owner provisioning failed")
    status, agent = http(hubu.base, "POST", "/agents/register", {"name": "standalone-agent", "version": "hub-206"})
    check(status == 200, f"agent registration failed: {agent}")
    status, body = http(hubu.base, "POST", "/policies", {"policy_yaml": POLICY})
    check(status == 200, f"policy failed: {body}")
    status, budget = http(hubu.base, "POST", "/budgets", {"agent_id": agent["agent_id"], "amount_cents": 5000, "ending_before": "2999-01-01T00:00:00Z"})
    check(status == 200, f"budget failed: {budget}")
    return {"agent_id": agent["agent_id"], "account_id": agent["account_id"], "budget_id": budget["budget"]["budget_id"]}


def budget(hubu: HubuServer, agent_id: str) -> dict:
    budgets = http(hubu.base, "GET", "/budgets")[1]["budgets"]
    return next(item for item in budgets if item["agent_id"] == agent_id)


def authorize(mcp: UnifiedMcp, ids: dict, amount: int, call_id: str, lease_profile: str = "default") -> dict:
    return mcp.call(
        "hubu_authorize_spend",
        {"account_id": ids["account_id"], "amount_cents": amount, "reason": f"standalone {call_id}", "execution_scope": SCOPE, "lease_profile": lease_profile},
        call_id=call_id,
    )


def continuation(result: dict) -> str:
    token = result.get("auth_token_id") or result.get("spend_auth_token_id")
    check(isinstance(token, str) and token, f"authorization returned no continuation: {result}")
    return token


def expect_status(mcp: UnifiedMcp, handle: str, state: str, terminal: bool, replacement_safe: bool, verified: bool = True) -> dict:
    status = mcp.status(handle)
    observed = (status.get("state"), status.get("terminal"), status.get("replacement_safe"), status.get("authority", {}).get("verified"))
    check(
        observed == (state, terminal, replacement_safe, verified),
        f"status for {handle}: expected {(state, terminal, replacement_safe, verified)}, got {observed}: {status}",
    )
    return status


def run(hubu_bin: Path, mcp_bin: Path, root: Path) -> None:
    hubu = HubuServer(hubu_bin, root)
    mcp = None
    try:
        ids = provision(hubu)
        mcp = UnifiedMcp(mcp_bin, hubu, root)
        executor = ExternalExecutor(hubu)

        # allow → external claim → external settlement
        allowed = authorize(mcp, ids, 400, "standalone-allow")
        check(allowed.get("decision") == "allow", f"expected allow: {allowed}")
        handle = allowed["operation_handle"]
        token = continuation(allowed)
        replay = authorize(mcp, ids, 400, "standalone-allow")
        check(replay["operation_handle"] == handle and continuation(replay) == token, "exact redelivery changed the operation")
        check(budget(hubu, ids["agent_id"])["frozen_amount_cents"] == 400, "redelivery created a second hold")
        expect_status(mcp, handle, "authorized", False, True)
        claim = executor.claim(token)
        check("operation_key" not in claim or claim["operation_key"], "claim response shape")
        expect_status(mcp, handle, "executing", False, False)
        executor.settle(token)
        settled = expect_status(mcp, handle, "settled", True, False)
        check(settled.get("settlement", {}).get("budget_charge_cents") == 351, f"settlement summary: {settled}")
        check(settled["result"] == {"code": "executor_settled"}, f"settled result: {settled}")
        ledger = http(hubu.base, "GET", f"/ledger/transactions?agent_id={ids['agent_id']}&budget_id={ids['budget_id']}")[1]
        check(len(ledger["transactions"]) == 1, "settlement must post exactly once")
        check(budget(hubu, ids["agent_id"])["consumed_amount_cents"] == 351, "settlement consumed once")

        # deny is terminal and never reserves
        denied = authorize(mcp, ids, 950, "standalone-deny")
        check(denied.get("decision") == "deny", f"expected deny: {denied}")
        check(not (denied.get("auth_token_id") or denied.get("spend_auth_token_id")), "deny must not return a continuation")
        denied_status = mcp.status(denied["operation_handle"])
        check(denied_status["terminal"] is True and denied_status["replacement_safe"] is False, f"deny status: {denied_status}")

        # needs_approval → human approval → resume (never Gongbu) → external release
        pending = authorize(mcp, ids, 600, "standalone-approval")
        check(pending.get("decision") == "needs_approval", f"expected needs_approval: {pending}")
        pending_handle = pending["operation_handle"]
        approval_id = pending["approval"]["approval_request_id"]
        check(mcp.status(pending_handle)["state"] == "approval_required", "pending approval status")
        resolved = mcp.call("hubu_resolve_spend_approval", {"approval_request_id": approval_id, "decision": "approve"})
        check(resolved.get("approval", resolved).get("status") == "approved", f"approval: {resolved}")
        resumed = mcp.call("hubu_resume_operation", {"operation_handle": pending_handle})
        # A resumed standalone authorization returns its continuation for the
        # agent to hand to an executor; it must not start Gongbu.
        resumed_token = continuation(resumed.get("hubu_result") or {})
        check(resumed.get("state") == "authorized", f"resume must authorize without Gongbu: {resumed}")
        approved_status = expect_status(mcp, pending_handle, "authorized", False, True)
        check(approved_status.get("execution_id") is None, "standalone resume must not start Gongbu")
        status_code, workflows = http(hubu.base, "GET", f"/spend/workflows?agent_id={ids['agent_id']}&status=authorized")
        check(status_code == 200 and len(workflows["workflows"]) == 1, f"approved authorization workflow: {workflows}")
        approved_claim = executor.claim(resumed_token)
        executor.release(approved_claim["claim_id"])
        expect_status(mcp, pending_handle, "released", True, False)

        # An executor's claim outlives the authorization: after the token
        # expires (and the router purges its copy) the handle must still
        # report the active claim, never a terminal or replaceable state.
        long_running = authorize(mcp, ids, 400, "standalone-long-running")
        long_handle = long_running["operation_handle"]
        long_token = continuation(long_running)
        executor.claim(long_token)
        time.sleep(3.5)
        expect_status(mcp, long_handle, "executing", False, False)
        executor.settle(long_token)
        expect_status(mcp, long_handle, "settled", True, False)

        # lease expiry routes to human reconciliation and is never replaceable
        short = authorize(mcp, ids, 300, "standalone-expiry", lease_profile="conformance_short")
        short_handle = short["operation_handle"]
        executor.claim(continuation(short))
        deadline = time.monotonic() + 15
        while mcp.status(short_handle)["state"] != "reconciliation_required":
            check(time.monotonic() < deadline, "claim never reached reconciliation_required")
            time.sleep(0.2)
        expect_status(mcp, short_handle, "reconciliation_required", False, False)

        # Hubu unreachable: fail closed, never replacement-safe
        hubu.stop()
        unverified = mcp.status(short_handle)
        check(
            (unverified["state"], unverified["terminal"], unverified["replacement_safe"], unverified["authority"]["verified"])
            == ("unverified", False, False, False),
            f"unverified status: {unverified}",
        )

        outputs = "\n".join(mcp.agent_outputs)
        for forbidden in FORBIDDEN_IN_AGENT_OUTPUT:
            check(forbidden not in outputs, f"agent-visible MCP output leaked {forbidden!r}")
    finally:
        if mcp:
            mcp.close()
        hubu.stop()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--hubu-server-bin", type=Path, required=True)
    parser.add_argument("--unified-mcp-bin", type=Path, required=True)
    parser.add_argument("--keep-state", action="store_true")
    args = parser.parse_args()
    root = Path(tempfile.mkdtemp(prefix="hubu-standalone-mcp-"))
    try:
        run(args.hubu_server_bin, args.unified_mcp_bin, root)
    except QualificationFailure as failure:
        print(f"standalone MCP executor qualification failed: {failure}", file=sys.stderr)
        print(f"diagnostic state preserved at {root}", file=sys.stderr)
        return 1
    if not args.keep_state:
        shutil.rmtree(root, ignore_errors=True)
    print("standalone MCP executor qualification passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
