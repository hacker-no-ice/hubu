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
import re
import shlex
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
SOURCE_PATHS = {
    "test": TEST_FILE,
    "runner": "scripts/break-hubu-episode-01.py",
    "scenario": "examples/break-hubu/scenario.json",
}
ASSERTION_IDS = {
    "callers_completed", "budget_decisions", "holds_before_settlement",
    "denied_no_authority", "balanced_settlements", "ledger_reconciled",
    "remaining_below_request", "durable_reopen",
}


def git(*args: str) -> str:
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def source_excerpts(source: str) -> list[dict]:
    """Extract exact contiguous test source between the walkthrough markers."""
    lines = source.splitlines(keepends=True)
    excerpts = []
    for phase in ("setup", "burst", "settlement", "persistence"):
        start_marker = f"// walkthrough:{phase}:start"
        end_marker = f"// walkthrough:{phase}:end"
        starts = [index for index, line in enumerate(lines) if line.strip() == start_marker]
        ends = [index for index, line in enumerate(lines) if line.strip() == end_marker]
        if len(starts) != 1 or len(ends) != 1 or starts[0] >= ends[0] - 1:
            raise ValueError(f"Invalid source excerpt markers for {phase}")
        start, end = starts[0], ends[0]
        excerpts.append({"id": phase, "path": TEST_FILE, "start_line": start + 2,
                         "end_line": end, "content": "".join(lines[start + 1:end])})
    return excerpts


def embedded_json(value: dict) -> str:
    """Keep recorded JSON inert inside an HTML script data element."""
    result = json.dumps(value, ensure_ascii=False)
    for character in ("<", ">", "&", "\u2028", "\u2029"):
        result = result.replace(character, f"\\u{ord(character):04x}")
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, help="New directory for evidence and replay")
    parser.add_argument("--serve", action="store_true", help="Serve the result on loopback until Ctrl-C")
    parser.add_argument("--port", type=int, default=0, help="Loopback port (default: choose an available port)")
    args = parser.parse_args()
    started = datetime.now(timezone.utc)
    output = (args.output or ROOT / "target/break-hubu" / started.strftime("%Y%m%dT%H%M%S.%fZ")).resolve()
    archive = output.parent / (output.name + ".zip")
    if archive.exists():
        parser.error(f"Refusing to overwrite existing archive: {archive}")
    # Never mix an earlier passing JSON file with a new failed attempt.
    output.mkdir(parents=True, exist_ok=False)
    evidence_dir = output / "evidence"
    evidence_dir.mkdir()
    commit = git("rev-parse", "HEAD")
    dirty = bool(git("status", "--porcelain"))
    source_bytes = {source_id: (ROOT / path).read_bytes()
                    for source_id, path in SOURCE_PATHS.items()}
    lock_bytes = (ROOT / "Cargo.lock").read_bytes()
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
    if (run.get("schema_version") != 2
            or {item.get("id") for item in run.get("assertions", [])} != ASSERTION_IDS
            or not all(item.get("passed") is True for item in run["assertions"])):
        print("Evidence assertions did not pass; inspect the test log.", file=sys.stderr)
        return 1
    if (any((ROOT / path).read_bytes() != source_bytes[source_id]
            for source_id, path in SOURCE_PATHS.items())
            or (ROOT / "Cargo.lock").read_bytes() != lock_bytes):
        print("Test source or lockfile changed during capture; refusing to package mismatched evidence.",
              file=sys.stderr)
        return 1
    log_bytes = (evidence_dir / "test.log").read_bytes()
    run["recording"] = {
        "command": shlex.join(COMMAND),
        "exit_code": returncode,
        "log": log_bytes.decode("utf-8"),
        "log_sha256": hashlib.sha256(log_bytes).hexdigest(),
        "sources": [{"id": source_id, "path": SOURCE_PATHS[source_id],
                     "content": content.decode("utf-8"),
                     "sha256": hashlib.sha256(content).hexdigest()}
                    for source_id, content in source_bytes.items()],
        "excerpts": source_excerpts(source_bytes["test"].decode("utf-8")),
    }
    # A working-tree run is useful for iteration, but cannot masquerade as a clean commit.
    run["provenance"] = {
        "commit": commit,
        "dirty": dirty or commit != git("rev-parse", "HEAD") or bool(git("status", "--porcelain")),
        "repository_url": "https://github.com/hacker-no-ice/hubu",
        "test_file": TEST_FILE,
        "test_command": shlex.join(COMMAND),
        "started_at_utc": started.isoformat(),
        "elapsed_ms": round((time.monotonic() - before) * 1000),
        "duration_scope": "cargo command including build; replay is editorially paced",
        "rustc": subprocess.check_output(["rustc", "--version"], cwd=ROOT, text=True).strip(),
        "cargo_lock_sha256": hashlib.sha256(lock_bytes).hexdigest(),
        "scenario_sha256": hashlib.sha256(source_bytes["scenario"]).hexdigest(),
    }
    captured.write_text(json.dumps(run, indent=2) + "\n", encoding="utf-8")
    shutil.copy2(captured, evidence_dir / "run.json")
    source_dir = evidence_dir / "source"
    source_dir.mkdir()
    for source_id, content in source_bytes.items():
        (source_dir / Path(SOURCE_PATHS[source_id]).name).write_bytes(content)
    for asset in ("index.html", "episode.css", "evidence.js", "episode.js", "scenario.json"):
        shutil.copy2(ASSETS / asset, output / asset)
    index = output / "index.html"
    html = index.read_text(encoding="utf-8")
    recorded = f'<script id="recorded-run" type="application/json">{embedded_json(run)}</script>\n    '
    html, injected = re.subn(r'(?=<script\s[^>]*src=[\'"](?:evidence|episode)\.js[\'"])',
                            lambda _: recorded, html, count=1)
    if injected != 1:
        print("Replay has no recognized script entry; refusing to package an unusable replay.", file=sys.stderr)
        return 1
    index.write_text(html, encoding="utf-8")
    (output / "README.txt").write_text(
        "Break Hubu Episode 01\n\n"
        "Unzip this whole directory, then double-click index.html. The captured run\n"
        "loads automatically; playback needs no server, account, keys, or network.\n"
        "Inspect evidence/run.json, evidence/test.log, and evidence/source/.\n\n"
        "To independently reproduce: install Git, Python 3, Rust (see rust-toolchain\n"
        "and Cargo.toml in the repository), and protoc. Clone the public repository:\n"
        "git clone https://github.com/hacker-no-ice/hubu.git\n"
        "cd hubu\n"
        f"git checkout {commit}\n"
        "python3 scripts/break-hubu-episode-01.py --output ../episode-01-reproduction\n"
        f"The runner executes: {shlex.join(COMMAND)}\n"
        f"Captured working tree had changes: {str(run['provenance']['dirty']).lower()}.\n"
        "If true, this commit alone is not the captured source; inspect the bundled\n"
        "source copies and use a clean published commit for exact reproduction.\n\n"
        "The test creates temporary SQLite state and fixture authentication. One\n"
        "registered agent sends 100 concurrent requests through real in-process\n"
        "Hubu authorization routes. Executor settlement receipts are synthetic;\n"
        "no provider is contacted and no real provider bill is measured.\n",
        encoding="utf-8",
    )
    # Exclusive creation also protects a same-name archive created during the run.
    import zipfile
    with zipfile.ZipFile(archive, mode="x", compression=zipfile.ZIP_DEFLATED) as bundle:
        for path in sorted(output.rglob("*")):
            if path.is_file():
                bundle.write(path, Path(output.name) / path.relative_to(output))
    print(f"\nVerified evidence and replay: {output / 'index.html'}", flush=True)
    print(f"Shareable offline bundle: {archive}", flush=True)
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
