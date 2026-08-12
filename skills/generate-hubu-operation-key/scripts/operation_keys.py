#!/usr/bin/env python3
"""Allocate and persist model-managed Hubu operation keys."""

from __future__ import annotations

import argparse
import hashlib
import hmac
import json
import os
import sqlite3
import sys
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


SCHEMA = """
CREATE TABLE IF NOT EXISTS operations (
    record_id TEXT PRIMARY KEY,
    operation_key TEXT NOT NULL UNIQUE,
    label TEXT NOT NULL,
    scope_json TEXT NOT NULL,
    scope_sha256 TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'completed', 'abandoned')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS operations_status_created
    ON operations(status, created_at);
"""


class OperationKeyError(Exception):
    pass


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")


def database_path(value: str | None) -> Path:
    configured = value or os.environ.get("HUBU_OPERATION_KEY_DB")
    if configured:
        return Path(configured).expanduser().resolve()
    return (Path.cwd() / ".hubu" / "operation-keys.sqlite3").resolve()


def connect(path: Path) -> sqlite3.Connection:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    connection = sqlite3.connect(path, timeout=10)
    connection.row_factory = sqlite3.Row
    connection.execute("PRAGMA busy_timeout = 10000")
    connection.execute("PRAGMA journal_mode = WAL")
    connection.executescript(SCHEMA)
    try:
        path.chmod(0o600)
    except OSError:
        pass
    return connection


def load_scope(scope_json: str | None, scope_file: str | None) -> tuple[str, str]:
    if scope_file:
        try:
            raw = Path(scope_file).read_text(encoding="utf-8")
        except OSError as error:
            raise OperationKeyError(f"cannot read scope file: {error}") from error
    else:
        raw = scope_json or ""

    try:
        value = json.loads(raw)
    except json.JSONDecodeError as error:
        raise OperationKeyError(f"scope must be valid JSON: {error}") from error
    if not isinstance(value, dict) or not value:
        raise OperationKeyError("scope must be a non-empty JSON object")

    canonical = json.dumps(value, ensure_ascii=False, separators=(",", ":"), sort_keys=True)
    digest = hashlib.sha256(canonical.encode("utf-8")).hexdigest()
    return canonical, digest


def row_payload(row: sqlite3.Row, database: Path) -> dict[str, Any]:
    return {
        "record_id": row["record_id"],
        "operation_key": row["operation_key"],
        "label": row["label"],
        "scope": json.loads(row["scope_json"]),
        "scope_sha256": row["scope_sha256"],
        "status": row["status"],
        "created_at": row["created_at"],
        "updated_at": row["updated_at"],
        "database": str(database),
    }


def fetch_record(connection: sqlite3.Connection, record_id: str) -> sqlite3.Row:
    row = connection.execute(
        "SELECT * FROM operations WHERE record_id = ?", (record_id,)
    ).fetchone()
    if row is None:
        raise OperationKeyError(f"unknown operation record: {record_id}")
    return row


def begin(connection: sqlite3.Connection, database: Path, args: argparse.Namespace) -> Any:
    canonical, digest = load_scope(args.scope_json, args.scope_file)
    label = args.label.strip()
    if not label:
        raise OperationKeyError("label must not be empty")

    identifier = uuid.uuid4().hex
    now = utc_now()
    connection.execute("BEGIN IMMEDIATE")
    try:
        connection.execute(
            """
            INSERT INTO operations
                (record_id, operation_key, label, scope_json, scope_sha256,
                 status, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, 'active', ?, ?)
            """,
            (
                f"hop_{identifier}",
                f"codex:v1:{identifier}",
                label,
                canonical,
                digest,
                now,
                now,
            ),
        )
        connection.commit()
    except Exception:
        connection.rollback()
        raise
    return row_payload(fetch_record(connection, f"hop_{identifier}"), database)


def reuse(connection: sqlite3.Connection, database: Path, args: argparse.Namespace) -> Any:
    canonical, digest = load_scope(args.scope_json, args.scope_file)
    row = fetch_record(connection, args.record_id)
    if not hmac.compare_digest(row["scope_sha256"], digest) or row["scope_json"] != canonical:
        raise OperationKeyError("operation scope changed; allocate a new operation instead")
    if row["status"] == "abandoned":
        raise OperationKeyError("abandoned operation cannot be reused")
    return row_payload(row, database)


def list_records(connection: sqlite3.Connection, database: Path, args: argparse.Namespace) -> Any:
    if args.status == "all":
        rows = connection.execute(
            "SELECT * FROM operations ORDER BY created_at, record_id"
        ).fetchall()
    else:
        rows = connection.execute(
            "SELECT * FROM operations WHERE status = ? ORDER BY created_at, record_id",
            (args.status,),
        ).fetchall()
    return {
        "database": str(database),
        "operations": [row_payload(row, database) for row in rows],
    }


def transition(
    connection: sqlite3.Connection,
    database: Path,
    record_id: str,
    target: str,
) -> Any:
    connection.execute("BEGIN IMMEDIATE")
    try:
        row = fetch_record(connection, record_id)
        if row["status"] == target:
            connection.commit()
            return row_payload(row, database)
        if row["status"] != "active":
            raise OperationKeyError(
                f"cannot change {row['status']} operation to {target}"
            )

        cursor = connection.execute(
            """
            UPDATE operations
            SET status = ?, updated_at = ?
            WHERE record_id = ? AND status = 'active'
            """,
            (target, utc_now(), record_id),
        )
        if cursor.rowcount != 1:
            raise OperationKeyError("operation status changed concurrently")
        connection.commit()
    except Exception:
        connection.rollback()
        raise
    return row_payload(fetch_record(connection, record_id), database)


def scope_arguments(parser: argparse.ArgumentParser) -> None:
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--scope-json", help="Complete immutable spend scope as a JSON object")
    group.add_argument("--scope-file", help="Path to a file containing the scope JSON object")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db", help="SQLite path (default: .hubu/operation-keys.sqlite3)")
    commands = parser.add_subparsers(dest="command", required=True)

    begin_parser = commands.add_parser("begin", help="Allocate a key for new logical work")
    begin_parser.add_argument("--label", required=True, help="Short non-secret recovery label")
    scope_arguments(begin_parser)

    reuse_parser = commands.add_parser("reuse", help="Recover a key and verify immutable scope")
    reuse_parser.add_argument("--record-id", required=True)
    scope_arguments(reuse_parser)

    list_parser = commands.add_parser("list", help="List persisted operation records")
    list_parser.add_argument(
        "--status",
        choices=("active", "completed", "abandoned", "all"),
        default="active",
    )

    finish_parser = commands.add_parser("finish", help="Mark a terminal operation completed")
    finish_parser.add_argument("--record-id", required=True)

    abandon_parser = commands.add_parser("abandon", help="Mark definitely unexecuted work abandoned")
    abandon_parser.add_argument("--record-id", required=True)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    database = database_path(args.db)
    try:
        with connect(database) as connection:
            if args.command == "begin":
                payload = begin(connection, database, args)
            elif args.command == "reuse":
                payload = reuse(connection, database, args)
            elif args.command == "list":
                payload = list_records(connection, database, args)
            elif args.command == "finish":
                payload = transition(connection, database, args.record_id, "completed")
            else:
                payload = transition(connection, database, args.record_id, "abandoned")
    except (OperationKeyError, sqlite3.Error) as error:
        print(f"operation-key error: {error}", file=sys.stderr)
        return 2

    json.dump(payload, sys.stdout, ensure_ascii=False, indent=2, sort_keys=True)
    print()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
