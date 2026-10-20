#!/usr/bin/env python3
"""Run the real Episode 01 regression and package its logs and visual replay."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time


ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / "examples/break-hubu"
TEST_FILE = "crates/hubu-api/src/tests/break_hubu.rs"
COMMAND = [
    "cargo", "test", "--lib", "-p", "hubu-api", "--locked",
    "break_hubu_episode_01", "--", "--nocapture", "--test-threads=1",
]


def git(*args: str) -> str:
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, help="New directory for evidence and replay")
    parser.add_argument("--serve", action="store_true", help="Serve the result on loopback until Ctrl-C")
    parser.add_argument("--port", type=int, default=0, help="Loopback port (default: choose an available port)")
    args = parser.parse_args()
    started = datetime.now(timezone.utc)
    output = (args.output or ROOT / "target/break-hubu" / started.strftime("%Y%m%dT%H%M%S.%fZ")).resolve()
    # Never mix an earlier passing JSON file with a new failed attempt.
    output.mkdir(parents=True, exist_ok=False)
    evidence_dir = output / "evidence"
    evidence_dir.mkdir()
    commit = git("rev-parse", "HEAD")
    dirty = bool(git("status", "--porcelain"))
    # The test constructor selects its own database and built-in test tokens.
    # An active shell/profile must not override those tokens or lease settings.
    env = {key: value for key, value in os.environ.items()
           if not key.startswith(("HUBU_", "GONGBU_"))}
    env.update({
        "HUBU_BREAK_EVIDENCE_DIR": str(evidence_dir),
        "GONGBU_MCP_INTEGRATION": "0",
        "GONGBU_SANDBOX_HUBU_MODE": "mock",
        "GONGBU_SANDBOX_PROVIDER_MODE": "mock",
        "CARGO_TERM_COLOR": "never",
        "HUBU_LOG_STDERR": "0",
    })
    print("Break Hubu / Episode 01: 100 concurrent callers, one agent account, $1.", flush=True)
    print(f"Evidence directory: {output}", flush=True)
    before = time.monotonic()
    with (evidence_dir / "test.log").open("w", encoding="utf-8") as log:
        process = subprocess.Popen(COMMAND, cwd=ROOT, env=env, stdout=subprocess.PIPE,
                                   stderr=subprocess.STDOUT, text=True)
        assert process.stdout is not None
        try:
            for line in process.stdout:
                log.write(line)
                log.flush()
                print(line, end="", flush=True)
            returncode = process.wait()
        except KeyboardInterrupt:
            process.terminate()
            process.wait()
            return 130
    if returncode:
        print(f"Test failed. Preserved logs: {evidence_dir / 'test.log'}", file=sys.stderr)
        return returncode
    captured = evidence_dir / "episode-01-evidence.json"
    if not captured.is_file():
        print("Test produced no evidence; refusing to package a passing replay.", file=sys.stderr)
        return 1
    run = json.loads(captured.read_text(encoding="utf-8"))
    if not run.get("assertions") or not all(item.get("passed") is True for item in run["assertions"]):
        print("Evidence assertions did not pass; inspect the test log.", file=sys.stderr)
        return 1
    # A working-tree run is useful for iteration, but cannot masquerade as a clean commit.
    run["provenance"] = {
        "commit": commit,
        "dirty": dirty or commit != git("rev-parse", "HEAD") or bool(git("status", "--porcelain")),
        "repository_url": "https://github.com/hacker-no-ice/hubu",
        "test_file": TEST_FILE,
        "test_command": " ".join(COMMAND),
        "started_at_utc": started.isoformat(),
        "elapsed_ms": round((time.monotonic() - before) * 1000),
        "duration_scope": "cargo command including build; replay is editorially paced",
        "rustc": subprocess.check_output(["rustc", "--version"], cwd=ROOT, text=True).strip(),
        "cargo_lock_sha256": hashlib.sha256((ROOT / "Cargo.lock").read_bytes()).hexdigest(),
        "scenario_sha256": hashlib.sha256((ASSETS / "scenario.json").read_bytes()).hexdigest(),
    }
    captured.write_text(json.dumps(run, indent=2) + "\n", encoding="utf-8")
    shutil.copy2(captured, evidence_dir / "run.json")
    for asset in ("index.html", "episode.css", "episode.js", "scenario.json"):
        shutil.copy2(ASSETS / asset, output / asset)
    (output / "README.txt").write_text(
        "Break Hubu Episode 01\n\n"
        "Open index.html and choose evidence/run.json, or serve this directory with\n"
        "python3 -m http.server --bind 127.0.0.1 8788 then open http://127.0.0.1:8788.\n"
        "This is a replay of a local concurrency regression with synthetic executor\n"
        "settlements. It does not measure a real provider bill or prove distinct\n"
        "authenticated agents share a budget. Inspect evidence/test.log and run.json.\n",
        encoding="utf-8",
    )
    print(f"\nVerified evidence and replay: {output / 'index.html'}", flush=True)
    if args.serve:
        handler = lambda *a, **kw: SimpleHTTPRequestHandler(*a, directory=str(output), **kw)
        with ThreadingHTTPServer(("127.0.0.1", args.port), handler) as server:
            print(f"Preview: http://127.0.0.1:{server.server_port}/ (Ctrl-C to stop)", flush=True)
            try:
                server.serve_forever()
            except KeyboardInterrupt:
                pass
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
