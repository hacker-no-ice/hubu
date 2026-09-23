#!/usr/bin/env python3
"""Black-box Hubu spend-executor conformance runner.

The runner replays the versioned fixture corpus in
``fixtures/hubu-executor-conformance-v4.3.json`` against the public HTTP
contract of a real ``hubu-server`` process. It never imports Hubu code or
opens Hubu storage.

Two target modes are supported:

* ``--server-bin PATH`` starts an isolated ``hubu-server`` with the corpus
  lease profile and restarts it for process-restart fault injection.
* ``--base-url URL`` attaches to an already running, dedicated Hubu instance.
  Restart scenarios require ``--restart-command``; otherwise they are skipped
  and the run fails unless ``--allow-skips`` is given.

Executor-side operations (resolve, claim, settle, release, claim inspection
and executor reconciliation attempts) go through a pluggable executor side.
The built-in side sends the corpus request directly. ``--executor-command``
instead starts an external executor that speaks the JSON-lines plugin protocol
described in ``docs/executor-conformance.md``.

Only the Python standard library is used so any executor project can run it.
"""

from __future__ import annotations

import argparse
import copy
import datetime as dt
import json
import os
import re
import shlex
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_CORPUS = ROOT / "fixtures" / "hubu-executor-conformance-v4.3.json"
CORPUS_SCHEMA = "hubu-executor-conformance-v1"
PLUGIN_PROTOCOL = "hubu-executor-conformance-plugin-v1"
RECONCILIATION_HEADER = "X-Hubu-Reconciliation-Capability"
TEMPLATE = re.compile(r"\$\{([A-Za-z0-9_.\-]+)\}")
SEGMENT = re.compile(r"^([A-Za-z0-9_\-]*)(?:\[(.+)\])?$")


class ConformanceFailure(AssertionError):
    pass


class ScenarioSkipped(Exception):
    pass


class TransportLost(Exception):
    """The request outcome is unknown to the caller (no HTTP response)."""


# ---------------------------------------------------------------------------
# Value helpers


def resolve_path(value: Any, path: str) -> Any:
    """Resolve a dotted path with optional ``[index]`` or ``[key=value]``."""
    current = value
    if path in ("", "$"):
        return current
    for raw in path.split("."):
        match = SEGMENT.match(raw)
        if match is None:
            raise ConformanceFailure(f"invalid path segment {raw!r} in {path!r}")
        name, selector = match.groups()
        if name:
            if not isinstance(current, dict) or name not in current:
                raise ConformanceFailure(f"path {path!r} is missing {name!r}")
            current = current[name]
        if selector is None:
            continue
        if not isinstance(current, list):
            raise ConformanceFailure(f"path {path!r} selects from a non-list")
        if "=" in selector:
            key, expected = selector.split("=", 1)
            found = [item for item in current if isinstance(item, dict) and str(item.get(key)) == expected]
            if len(found) != 1:
                raise ConformanceFailure(
                    f"path {path!r} selector [{selector}] matched {len(found)} items, expected 1"
                )
            current = found[0]
        else:
            index = int(selector)
            if index >= len(current):
                raise ConformanceFailure(f"path {path!r} index {index} out of range")
            current = current[index]
    return current


def parse_timestamp(value: Any) -> dt.datetime:
    if not isinstance(value, str):
        raise ConformanceFailure(f"expected an RFC 3339 timestamp, got {value!r}")
    text = value.replace("Z", "+00:00")
    # Python before 3.11 accepts at most six fractional digits.
    text = re.sub(r"(\.\d{6})\d+", r"\1", text)
    try:
        parsed = dt.datetime.fromisoformat(text)
    except ValueError as error:
        raise ConformanceFailure(f"invalid timestamp {value!r}: {error}") from error
    if parsed.tzinfo is None:
        raise ConformanceFailure(f"timestamp {value!r} has no offset")
    return parsed


class Context:
    """Variables visible to ``${...}`` templates within one scenario."""

    def __init__(self, definitions: dict[str, Any]):
        self.values: dict[str, Any] = {"defs": definitions}

    def lookup(self, name: str) -> Any:
        head, _, rest = name.partition(".")
        if head not in self.values:
            raise ConformanceFailure(f"template variable {name!r} is undefined")
        return resolve_path(self.values[head], rest) if rest else self.values[head]

    def set(self, name: str, value: Any) -> None:
        if name in self.values and self.values[name] != value:
            raise ConformanceFailure(
                f"capture {name!r} changed from {self.values[name]!r} to {value!r}"
            )
        self.values[name] = value

    def render(self, template: Any) -> Any:
        if isinstance(template, str):
            whole = TEMPLATE.fullmatch(template)
            if whole:
                return copy.deepcopy(self.lookup(whole.group(1)))
            return TEMPLATE.sub(lambda m: str(self.lookup(m.group(1))), template)
        if isinstance(template, list):
            return [self.render(item) for item in template]
        if isinstance(template, dict):
            rendered = {}
            for key, item in template.items():
                if key == "$merge":
                    continue
                rendered[key] = self.render(item)
            if "$merge" in template:
                base = self.render(template["$merge"])
                if not isinstance(base, dict):
                    raise ConformanceFailure("$merge must reference an object")
                base.update(rendered)
                return base
            return rendered
        return template


# ---------------------------------------------------------------------------
# HTTP


def http_request(
    base_url: str,
    method: str,
    path: str,
    *,
    bearer: str | None,
    body: Any = None,
    query: dict[str, Any] | None = None,
    headers: dict[str, str] | None = None,
    timeout: float = 10.0,
) -> tuple[int, Any]:
    url = base_url.rstrip("/") + path
    if query:
        url += "?" + urllib.parse.urlencode({k: str(v) for k, v in query.items()})
    request_headers = {"Accept": "application/json"}
    if bearer is not None:
        request_headers["Authorization"] = f"Bearer {bearer}"
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        request_headers["Content-Type"] = "application/json"
    request_headers.update(headers or {})
    request = urllib.request.Request(url, method=method, data=data, headers=request_headers)
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            status, raw = response.status, response.read()
    except urllib.error.HTTPError as error:
        status, raw = error.code, error.read()
    except (urllib.error.URLError, OSError) as error:
        raise TransportLost(str(error)) from error
    try:
        parsed = json.loads(raw) if raw else None
    except json.JSONDecodeError:
        parsed = {"raw_body": raw.decode(errors="replace")}
    return status, parsed


# ---------------------------------------------------------------------------
# Hubu targets


class Target:
    base_url: str

    def restart(self) -> None:
        raise ScenarioSkipped("target cannot restart Hubu (pass --restart-command)")

    def stop(self) -> None:
        pass

    def diagnostics(self) -> str:
        return ""


def wait_until_ready(base_url: str, alive=lambda: True, timeout: float = 20.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if not alive():
            raise ConformanceFailure("hubu-server exited before becoming ready")
        try:
            status, _ = http_request(base_url, "GET", "/health", bearer=None, timeout=1)
            if status == 200:
                return
        except TransportLost:
            pass
        time.sleep(0.05)
    raise ConformanceFailure(f"Hubu at {base_url} did not become ready")


class ManagedServer(Target):
    """A real hubu-server process owned by the runner."""

    def __init__(self, binary: Path, corpus: dict[str, Any], auth: str, reconciliation: str, keep: bool):
        self.binary = binary
        self.directory = Path(tempfile.mkdtemp(prefix="hubu-executor-conformance-"))
        self.keep = keep
        lease = self.directory / "lease-config.yaml"
        lease.write_text(corpus["server_profile"]["lease_config_yaml"])
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            self.address = f"127.0.0.1:{probe.getsockname()[1]}"
        self.base_url = f"http://{self.address}"
        self.log_path = self.directory / "hubu-server.log"
        self.env = dict(
            os.environ,
            HUBU_DB_PATH=str(self.directory / "hubu.sqlite3"),
            HUBU_AUTH_TOKEN=auth,
            HUBU_RECONCILIATION_TOKEN=reconciliation,
            HUBU_LEASE_CONFIG=str(lease),
        )
        for name in ("HUBU_AUTH_TOKEN_FILE", "HUBU_RECONCILIATION_TOKEN_FILE"):
            self.env.pop(name, None)
        self.process: subprocess.Popen | None = None
        self.starts = 0
        self.start()

    def start(self) -> None:
        log = open(self.log_path, "a")
        self.process = subprocess.Popen(
            [str(self.binary), self.address],
            env=self.env,
            stdout=log,
            stderr=subprocess.STDOUT,
        )
        log.close()
        self.starts += 1
        wait_until_ready(self.base_url, lambda: self.process.poll() is None)

    def restart(self) -> None:
        self.kill()
        self.start()

    def kill(self) -> None:
        if self.process and self.process.poll() is None:
            # SIGKILL: no graceful shutdown, so recovery relies only on
            # durable state.
            self.process.kill()
            self.process.wait(timeout=10)

    def stop(self) -> None:
        self.kill()

    def cleanup(self, success: bool) -> None:
        if success and not self.keep:
            shutil.rmtree(self.directory, ignore_errors=True)
        else:
            print(f"diagnostic Hubu state preserved at {self.directory}", file=sys.stderr)

    def diagnostics(self) -> str:
        try:
            lines = self.log_path.read_text(errors="replace").splitlines()
        except OSError:
            return ""
        return "\n".join(lines[-40:])


class AttachedServer(Target):
    def __init__(self, base_url: str, restart_command: str | None):
        self.base_url = base_url.rstrip("/")
        self.restart_command = restart_command
        wait_until_ready(self.base_url)

    def restart(self) -> None:
        if not self.restart_command:
            super().restart()
        subprocess.run(self.restart_command, shell=True, check=True)
        wait_until_ready(self.base_url, timeout=60)


# ---------------------------------------------------------------------------
# Executor sides


class ExecutorSide:
    name = "abstract"

    def invoke(self, message: dict[str, Any]) -> tuple[int, Any]:
        raise NotImplementedError

    def close(self) -> None:
        pass


class BuiltinHttpExecutor(ExecutorSide):
    """Reference executor side: sends the canonical corpus request itself."""

    name = "builtin-http"

    def __init__(self, base_url: str, bearer: str):
        self.base_url = base_url
        self.bearer = bearer

    def invoke(self, message: dict[str, Any]) -> tuple[int, Any]:
        headers = {}
        if message.get("reconciliation_capability") == "executor_bearer":
            headers[RECONCILIATION_HEADER] = self.bearer
        return http_request(
            self.base_url,
            message["method"],
            message["path"],
            bearer=self.bearer,
            body=message.get("body"),
            query=message.get("query"),
            headers=headers,
        )


class PluginExecutor(ExecutorSide):
    """External executor speaking the JSON-lines plugin protocol."""

    def __init__(self, command: str, base_url: str, contract: str):
        self.process = subprocess.Popen(
            shlex.split(command),
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        ready = self._exchange(
            {"type": "hello", "protocol": PLUGIN_PROTOCOL, "base_url": base_url, "executor_contract": contract}
        )
        if ready.get("type") != "ready":
            raise ConformanceFailure(f"executor plugin did not report ready: {ready}")
        self.name = f"plugin:{ready.get('executor', 'unnamed')}"

    def _exchange(self, message: dict[str, Any]) -> dict[str, Any]:
        assert self.process.stdin and self.process.stdout
        self.process.stdin.write(json.dumps(message) + "\n")
        self.process.stdin.flush()
        line = self.process.stdout.readline()
        if not line:
            raise ConformanceFailure("executor plugin exited unexpectedly")
        return json.loads(line)

    def invoke(self, message: dict[str, Any]) -> tuple[int, Any]:
        reply = self._exchange(dict(message, type="invoke"))
        if "transport_error" in reply:
            raise TransportLost(reply["transport_error"])
        return reply["status"], reply.get("body")

    def close(self) -> None:
        if self.process.poll() is None:
            try:
                self._exchange({"type": "shutdown"})
            except (ConformanceFailure, OSError, json.JSONDecodeError):
                pass
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()


# ---------------------------------------------------------------------------
# Runner


class Runner:
    def __init__(self, corpus: dict[str, Any], target: Target, executor: ExecutorSide, args: argparse.Namespace):
        self.corpus = corpus
        self.target = target
        self.executor = executor
        self.auth = args.auth_token
        self.reconciliation = args.reconciliation_token
        self.run_id = args.run_id
        self.verbose = args.verbose
        self.transcript: list[dict[str, Any]] = []

    # -- setup ------------------------------------------------------------

    def check_contract(self) -> None:
        version = self.corpus["protocol_version"]
        for path in ("/spend/executor/guidance", "/.well-known/hubu-spend-executor.json"):
            status, guidance = http_request(self.target.base_url, "GET", path, bearer=None)
            if status != 200 or guidance.get("protocol_version") != version:
                raise ConformanceFailure(f"{path} does not publish {version}: {status} {guidance}")
        required = self.corpus["server_profile"]["required_lease_profiles"]
        published = guidance["lease"]["lease_profiles"]
        for profile, claim_ttl in required.items():
            actual = published.get(profile, {}).get("claim_ttl_seconds")
            if actual != claim_ttl:
                raise ConformanceFailure(
                    f"target lease profile {profile!r} has claim_ttl_seconds={actual!r}, "
                    f"the corpus requires {claim_ttl}; configure HUBU_LEASE_CONFIG from the corpus"
                )

    def provision_owner(self) -> None:
        status, user = http_request(self.target.base_url, "GET", "/user", bearer=self.auth)
        if status != 200:
            status, user = http_request(
                self.target.base_url, "POST", "/init", bearer=self.auth, body=self.corpus["setup"]["owner"]
            )
            if status != 200:
                raise ConformanceFailure(f"owner provisioning failed: {status} {user}")
        status, body = http_request(
            self.target.base_url,
            "POST",
            "/policies",
            bearer=self.auth,
            body={"policy_yaml": self.corpus["setup"]["policy_yaml"]},
        )
        if status != 200:
            raise ConformanceFailure(f"policy provisioning failed: {status} {body}")

    def provision_agent(self, scenario: dict[str, Any], context: Context) -> None:
        name = f"conformance-{scenario['id'].lower()}-{self.run_id}"
        status, agent = http_request(
            self.target.base_url,
            "POST",
            "/agents/register",
            bearer=self.auth,
            body={"name": name, "version": self.corpus["protocol_version"]},
        )
        if status != 200:
            raise ConformanceFailure(f"agent registration failed: {status} {agent}")
        status, budget = http_request(
            self.target.base_url,
            "POST",
            "/budgets",
            bearer=self.auth,
            body={
                "agent_id": agent["agent_id"],
                "amount_cents": scenario["setup"]["budget_cents"],
                "ending_before": self.corpus["setup"]["budget_ending_before"],
            },
        )
        if status != 200:
            raise ConformanceFailure(f"budget provisioning failed: {status} {budget}")
        context.set("agent", {"agent_id": agent["agent_id"], "account_id": agent["account_id"]})
        context.set("budget", {"budget_id": budget["budget"]["budget_id"]})

    # -- execution ----------------------------------------------------------

    def scenario_steps(self, scenario: dict[str, Any]) -> list[dict[str, Any]]:
        steps: list[dict[str, Any]] = []
        for included in scenario.get("includes", []):
            steps.extend(self.scenario_steps(self.scenarios[included]))
        steps.extend(scenario["steps"])
        return steps

    def run(self, selected: list[str] | None) -> list[tuple[str, str, str]]:
        self.scenarios = {scenario["id"]: scenario for scenario in self.corpus["scenarios"]}
        self.check_contract()
        self.provision_owner()
        results = []
        for scenario in self.corpus["scenarios"]:
            if selected and scenario["id"] not in selected:
                continue
            label = f"{scenario['id']} {scenario['title']}"
            context = Context(self.corpus["definitions"])
            try:
                self.provision_agent(scenario, context)
                for step in self.scenario_steps(scenario):
                    self.run_step(scenario, step, context)
            except ScenarioSkipped as skipped:
                results.append((scenario["id"], "SKIP", str(skipped)))
                print(f"SKIP {label}: {skipped}")
                continue
            except ConformanceFailure as failure:
                results.append((scenario["id"], "FAIL", str(failure)))
                print(f"FAIL {label}: {failure}")
                continue
            results.append((scenario["id"], "PASS", ""))
            print(f"PASS {label}")
        return results

    def run_step(self, scenario: dict[str, Any], step: dict[str, Any], context: Context) -> None:
        where = f"step {step['id']!r}"
        try:
            if "control" in step:
                self.run_control(step, context)
                return
            operation = self.corpus["operations"][step["operation"]]
            actor = step.get("actor", operation["actor"])
            message = {
                "scenario": scenario["id"],
                "step": step["id"],
                "operation": step["operation"],
                "method": operation["method"],
                "path": operation["path"],
                "body": context.render(step["body"]) if "body" in step else None,
                "query": context.render(step["query"]) if "query" in step else None,
                "fault": step.get("fault"),
                "reconciliation_capability": step.get("reconciliation_capability"),
            }
            status, body = self.send(actor, message)
            self.transcript.append({"step": step["id"], "request": message, "status": status, "body": body})
            if self.verbose:
                print(f"  {scenario['id']}/{step['id']} -> {status}")
            if step.get("fault") == "drop_response":
                # The request reached Hubu, but the caller observes only an
                # ambiguous outcome. The corpus follows with a retry step.
                return
            self.check_expectations(step.get("expect", {}), status, body, context)
        except ConformanceFailure as failure:
            raise ConformanceFailure(f"{where}: {failure}") from None

    def send(self, actor: str, message: dict[str, Any]) -> tuple[int, Any]:
        try:
            if actor == "executor":
                return self.executor.invoke(message)
            headers = {}
            if actor == "human":
                headers[RECONCILIATION_HEADER] = self.reconciliation
            elif actor not in ("agent", "observer"):
                raise ConformanceFailure(f"unknown actor {actor!r}")
            return http_request(
                self.target.base_url,
                message["method"],
                message["path"],
                bearer=self.auth,
                body=message["body"],
                query=message["query"],
                headers=headers,
            )
        except TransportLost as lost:
            if message.get("fault") == "drop_response":
                return 0, None
            raise ConformanceFailure(f"transport failure: {lost}") from None

    def run_control(self, step: dict[str, Any], context: Context) -> None:
        control = step["control"]
        if control == "restart_hubu":
            self.target.restart()
        elif control == "await_reconciliation_required":
            claim_id = context.render(step["claim_id"])
            deadline = time.monotonic() + step.get("timeout_seconds", 15)
            while time.monotonic() < deadline:
                status, claim = http_request(
                    self.target.base_url,
                    "GET",
                    self.corpus["operations"]["inspect_claim"]["path"],
                    bearer=self.auth,
                    query={"claim_id": claim_id},
                )
                if status == 200 and claim.get("reconciliation_required") is True:
                    return
                time.sleep(0.1)
            raise ConformanceFailure(f"claim {claim_id} never required reconciliation")
        else:
            raise ConformanceFailure(f"unknown control {control!r}")

    # -- assertions -------------------------------------------------------

    def check_expectations(self, expect: dict[str, Any], status: int, body: Any, context: Context) -> None:
        decision_id = expect.get("retry_decision")
        expected_status = expect.get("status")
        match = dict(expect.get("match", {}))
        error_contains = expect.get("error_contains")
        if decision_id:
            decision = self.corpus["retry_decisions"][decision_id]
            observed = decision.get("observed")
            if observed is None:
                raise ConformanceFailure(f"retry decision {decision_id!r} has no observable response")
            expected_status = observed["status"]
            error_contains = observed.get("error_contains", error_contains)
            match.update(observed.get("match", {}))
        if expected_status is not None and status != expected_status:
            raise ConformanceFailure(f"expected HTTP {expected_status}, got {status}: {json.dumps(body)[:800]}")
        if error_contains is not None:
            error = (body or {}).get("error", "")
            if error_contains not in error:
                raise ConformanceFailure(f"expected error containing {error_contains!r}, got {error!r}")
        for path, expected in match.items():
            actual = resolve_path(body, context.render(path))
            wanted = context.render(expected)
            if actual != wanted:
                raise ConformanceFailure(f"{path}: expected {wanted!r}, got {actual!r}")
        for rule in expect.get("counts", []):
            items = resolve_path(body, context.render(rule["path"]))
            if not isinstance(items, list):
                raise ConformanceFailure(f"{rule['path']} is not a list")
            where = context.render(rule.get("where", {}))
            matched = [
                item for item in items if all(resolve_path(item, key) == value for key, value in where.items())
            ]
            if len(matched) != rule["equals"]:
                raise ConformanceFailure(
                    f"expected {rule['equals']} items at {rule['path']} where {where}, found {len(matched)}"
                )
        for name, path in expect.get("capture", {}).items():
            context.set(name, resolve_path(body, context.render(path)))
        for relation in expect.get("relations", []):
            self.check_relation(relation, body, context)

    def check_relation(self, relation: list[str], body: Any, context: Context) -> None:
        op, left_ref, right_ref = relation

        def operand(ref: str) -> Any:
            if ref.startswith("@"):
                return resolve_path(body, context.render(ref[1:]))
            return context.render(ref)

        left, right = operand(left_ref), operand(right_ref)
        if op == "same":
            ok = left == right
        elif op == "different":
            ok = left != right
        elif op == "before":
            ok = parse_timestamp(left) < parse_timestamp(right)
        elif op == "not_after":
            ok = parse_timestamp(left) <= parse_timestamp(right)
        elif op == "non_null":
            ok = left is not None and left != ""
        else:
            raise ConformanceFailure(f"unknown relation {op!r}")
        if not ok:
            raise ConformanceFailure(f"relation {op}({left_ref}, {right_ref}) failed: {left!r} vs {right!r}")


# ---------------------------------------------------------------------------
# Corpus validation


def validate_corpus(corpus: dict[str, Any]) -> None:
    if corpus.get("schema") != CORPUS_SCHEMA:
        raise ConformanceFailure(f"unsupported corpus schema {corpus.get('schema')!r}")
    ids = [scenario["id"] for scenario in corpus["scenarios"]]
    if len(ids) != len(set(ids)):
        raise ConformanceFailure("duplicate scenario ids")
    known = set(ids)
    for scenario in corpus["scenarios"]:
        for included in scenario.get("includes", []):
            if included not in known or ids.index(included) >= ids.index(scenario["id"]):
                raise ConformanceFailure(f"{scenario['id']} includes unknown or later scenario {included}")
        step_ids = set()
        for step in scenario["steps"]:
            if step["id"] in step_ids:
                raise ConformanceFailure(f"{scenario['id']} repeats step id {step['id']}")
            step_ids.add(step["id"])
            if "control" in step:
                continue
            if step["operation"] not in corpus["operations"]:
                raise ConformanceFailure(f"{scenario['id']}/{step['id']} uses unknown operation")
            decision = step.get("expect", {}).get("retry_decision")
            if decision and decision not in corpus["retry_decisions"]:
                raise ConformanceFailure(f"{scenario['id']}/{step['id']} uses unknown retry decision")
            if step.get("fault") not in (None, "drop_response"):
                raise ConformanceFailure(f"{scenario['id']}/{step['id']} uses unknown fault")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    target = parser.add_mutually_exclusive_group()
    target.add_argument("--server-bin", type=Path, help="start and manage an isolated hubu-server")
    target.add_argument("--base-url", help="attach to a running, dedicated Hubu instance")
    parser.add_argument("--corpus", type=Path, default=DEFAULT_CORPUS)
    parser.add_argument("--auth-token", default=os.environ.get("HUBU_CONFORMANCE_AUTH_TOKEN", "hubu_conformance_normal_bearer"))
    parser.add_argument(
        "--reconciliation-token",
        default=os.environ.get("HUBU_CONFORMANCE_RECONCILIATION_TOKEN", "hubu_conformance_human_capability"),
    )
    parser.add_argument("--restart-command", help="shell command that restarts an attached Hubu target")
    parser.add_argument("--executor-command", help="external executor plugin command (JSON-lines protocol)")
    parser.add_argument("--scenario", action="append", help="run only this scenario id (repeatable)")
    parser.add_argument("--run-id", default="run", help="suffix that keeps agent names unique on reused targets")
    parser.add_argument("--transcript", type=Path, help="write the observed request/response transcript as JSON")
    parser.add_argument("--allow-skips", action="store_true")
    parser.add_argument("--keep-state", action="store_true", help="keep managed Hubu state after success")
    parser.add_argument("--list", action="store_true", help="print the scenario catalog and exit")
    parser.add_argument("--verbose", action="store_true")
    args = parser.parse_args()

    corpus = json.loads(args.corpus.read_text())
    validate_corpus(corpus)
    if args.list:
        for scenario in corpus["scenarios"]:
            print(f"{scenario['id']}  {scenario['title']}")
        return 0
    if not (args.server_bin or args.base_url):
        parser.error("one of --server-bin or --base-url is required")
    if args.auth_token == args.reconciliation_token:
        parser.error("the human reconciliation capability must differ from the normal bearer")

    managed: ManagedServer | None = None
    if args.server_bin:
        managed = ManagedServer(args.server_bin, corpus, args.auth_token, args.reconciliation_token, args.keep_state)
        hubu: Target = managed
    else:
        hubu = AttachedServer(args.base_url, args.restart_command)

    executor: ExecutorSide
    if args.executor_command:
        executor = PluginExecutor(args.executor_command, hubu.base_url, corpus["protocol_version"])
    else:
        executor = BuiltinHttpExecutor(hubu.base_url, args.auth_token)

    runner = Runner(corpus, hubu, executor, args)
    success = False
    try:
        print(
            f"Hubu executor conformance {corpus['corpus_version']} ({corpus['protocol_version']}) "
            f"against {hubu.base_url} with executor side {executor.name}"
        )
        results = runner.run(args.scenario)
        failed = [r for r in results if r[1] == "FAIL"]
        skipped = [r for r in results if r[1] == "SKIP"]
        passed = [r for r in results if r[1] == "PASS"]
        print(f"{len(passed)} passed, {len(failed)} failed, {len(skipped)} skipped")
        success = not failed and (args.allow_skips or not skipped) and bool(results)
        if not success and hubu.diagnostics():
            print("\nHubu server log tail:\n" + hubu.diagnostics(), file=sys.stderr)
    finally:
        executor.close()
        hubu.stop()
        if args.transcript:
            args.transcript.write_text(json.dumps(runner.transcript, indent=2))
        if managed:
            managed.cleanup(success)
    return 0 if success else 1


if __name__ == "__main__":
    sys.exit(main())
