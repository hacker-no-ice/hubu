#!/usr/bin/env python3
"""Allocate and persist model-managed Hubu operation keys."""

from __future__ import annotations

import argparse
import hashlib
import hmac
import json
import os
import sqlite3
import stat
import sys
import tempfile
import uuid
from collections.abc import Callable
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
    binding_id TEXT,
    bound_at TEXT,
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


def new_identifier() -> str:
    return uuid.uuid4().hex


def database_path(value: str | None) -> Path:
    configured = value or os.environ.get("HUBU_OPERATION_KEY_DB")
    if configured:
        candidate = Path(configured).expanduser()
    else:
        candidate = Path.cwd() / ".hubu" / "operation-keys.sqlite3"
    return Path(os.path.abspath(candidate))


def require_private_directory(path: Path) -> None:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise OperationKeyError("operation-key directory is unavailable") from error
    if (
        stat.S_ISLNK(metadata.st_mode)
        or not stat.S_ISDIR(metadata.st_mode)
        or metadata.st_mode & 0o077
    ):
        raise OperationKeyError("operation-key directory permissions are unsafe")


def database_sidecars(path: Path) -> tuple[Path, Path]:
    return (Path(f"{path}-wal"), Path(f"{path}-shm"))


def secure_database_files(path: Path) -> None:
    for candidate in (path, *database_sidecars(path)):
        if not os.path.lexists(candidate):
            continue
        try:
            metadata = candidate.lstat()
            if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
                raise OperationKeyError("operation-key database files are unsafe")
            candidate.chmod(0o600)
            if candidate.stat().st_mode & 0o077:
                raise OperationKeyError("operation-key database permissions are unsafe")
        except OSError as error:
            raise OperationKeyError("cannot secure operation-key database") from error


def ensure_bridge_schema(connection: sqlite3.Connection) -> None:
    columns = {
        row[1] for row in connection.execute("PRAGMA table_info(operations)").fetchall()
    }
    for column in ("binding_id", "bound_at"):
        if column not in columns:
            connection.execute(f"ALTER TABLE operations ADD COLUMN {column} TEXT")
    connection.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS operations_binding "
        "ON operations(binding_id) WHERE binding_id IS NOT NULL"
    )


def connect(path: Path) -> sqlite3.Connection:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    require_private_directory(path.parent)
    if os.path.lexists(path):
        metadata = path.lstat()
        if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
            raise OperationKeyError("operation-key database target is unsafe")
    try:
        connection = sqlite3.connect(path, timeout=10)
        connection.row_factory = sqlite3.Row
        connection.execute("PRAGMA busy_timeout = 10000")
        connection.execute("PRAGMA journal_mode = WAL")
        connection.executescript(SCHEMA)
        ensure_bridge_schema(connection)
        connection.commit()
        path.chmod(0o600)
        secure_database_files(path)
    except Exception:
        if "connection" in locals():
            connection.close()
        raise
    return connection


def load_json_object(
    inline_json: str | None,
    json_file: str | None,
    description: str,
) -> dict[str, Any]:
    if json_file:
        try:
            raw = Path(json_file).read_text(encoding="utf-8")
        except OSError as error:
            raise OperationKeyError(f"cannot read {description} file") from error
    else:
        raw = inline_json or ""

    try:
        value = json.loads(raw)
    except json.JSONDecodeError as error:
        raise OperationKeyError(f"{description} must be valid JSON: {error}") from error
    if not isinstance(value, dict):
        raise OperationKeyError(f"{description} must be a JSON object")
    return value


def canonical_scope(value: dict[str, Any]) -> tuple[str, str]:
    if not value:
        raise OperationKeyError("scope must be a non-empty JSON object")

    canonical = json.dumps(value, ensure_ascii=False, separators=(",", ":"), sort_keys=True)
    digest = hashlib.sha256(canonical.encode("utf-8")).hexdigest()
    return canonical, digest


def load_scope(scope_json: str | None, scope_file: str | None) -> tuple[str, str]:
    return canonical_scope(load_json_object(scope_json, scope_file, "scope"))


def load_unified_scope(args: argparse.Namespace) -> tuple[str, str]:
    tool_name = args.tool_name.strip()
    if (
        not tool_name
        or len(tool_name.encode("utf-8")) > 128
        or any(character.isspace() or ord(character) < 32 for character in tool_name)
    ):
        raise OperationKeyError("tool name is invalid")
    arguments = load_json_object(
        args.arguments_json, args.arguments_file, "arguments"
    )
    return canonical_scope(
        {
            "schema_version": 1,
            "tool_name": tool_name,
            "arguments": arguments,
        }
    )


def row_payload(
    row: sqlite3.Row, database: Path, *, include_operation_key: bool = True
) -> dict[str, Any]:
    if not include_operation_key:
        return {
            "record_id": row["record_id"],
            "label": row["label"],
            "scope_sha256": row["scope_sha256"],
            "status": row["status"],
            "created_at": row["created_at"],
            "updated_at": row["updated_at"],
        }
    payload = {
        "record_id": row["record_id"],
        "label": row["label"],
        "scope": json.loads(row["scope_json"]),
        "scope_sha256": row["scope_sha256"],
        "status": row["status"],
        "created_at": row["created_at"],
        "updated_at": row["updated_at"],
        "database": str(database),
    }
    payload["operation_key"] = row["operation_key"]
    return payload


def fetch_record(connection: sqlite3.Connection, record_id: str) -> sqlite3.Row:
    row = connection.execute(
        "SELECT * FROM operations WHERE record_id = ?", (record_id,)
    ).fetchone()
    if row is None:
        raise OperationKeyError(f"unknown operation record: {record_id}")
    return row


def begin(connection: sqlite3.Connection, database: Path, args: argparse.Namespace) -> Any:
    canonical, digest = load_scope(args.scope_json, args.scope_file)
    return begin_with_scope(connection, database, args.label, canonical, digest, True)


def begin_with_scope(
    connection: sqlite3.Connection,
    database: Path,
    raw_label: str,
    canonical: str,
    digest: str,
    include_operation_key: bool,
    identifier_factory: Callable[[], str] = new_identifier,
) -> Any:
    label = raw_label.strip()
    if not label:
        raise OperationKeyError("label must not be empty")

    record_identifier = identifier_factory()
    key_identifier = identifier_factory()
    while hmac.compare_digest(record_identifier, key_identifier):
        key_identifier = identifier_factory()
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
                f"hop_{record_identifier}",
                f"codex:v1:{key_identifier}",
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
    return row_payload(
        fetch_record(connection, f"hop_{record_identifier}"),
        database,
        include_operation_key=include_operation_key,
    )


def begin_unified(
    connection: sqlite3.Connection, database: Path, args: argparse.Namespace
) -> Any:
    canonical, digest = load_unified_scope(args)
    return begin_with_scope(connection, database, args.label, canonical, digest, False)


def reuse(connection: sqlite3.Connection, database: Path, args: argparse.Namespace) -> Any:
    canonical, digest = load_scope(args.scope_json, args.scope_file)
    return reuse_with_scope(connection, database, args.record_id, canonical, digest, True)


def reuse_with_scope(
    connection: sqlite3.Connection,
    database: Path,
    record_id: str,
    canonical: str,
    digest: str,
    include_operation_key: bool,
) -> Any:
    row = fetch_record(connection, record_id)
    if not hmac.compare_digest(row["scope_sha256"], digest) or row["scope_json"] != canonical:
        raise OperationKeyError("operation scope changed; allocate a new operation instead")
    if row["status"] == "abandoned":
        raise OperationKeyError("abandoned operation cannot be reused")
    return row_payload(row, database, include_operation_key=include_operation_key)


def reuse_unified(
    connection: sqlite3.Connection, database: Path, args: argparse.Namespace
) -> Any:
    canonical, digest = load_unified_scope(args)
    return reuse_with_scope(
        connection, database, args.record_id, canonical, digest, False
    )


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
        "operations": [
            row_payload(
                row, database, include_operation_key=not args.reference_only
            )
            for row in rows
        ],
        **({"database": str(database)} if not args.reference_only else {}),
    }


def transition(
    connection: sqlite3.Connection,
    database: Path,
    record_id: str,
    target: str,
    include_operation_key: bool,
) -> Any:
    connection.execute("BEGIN IMMEDIATE")
    try:
        row = fetch_record(connection, record_id)
        if row["status"] == target:
            connection.commit()
            return row_payload(
                row, database, include_operation_key=include_operation_key
            )
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
    return row_payload(
        fetch_record(connection, record_id),
        database,
        include_operation_key=include_operation_key,
    )


def self_test() -> None:
    """Exercise the redacted bridge contract with fixed, non-billable fixtures."""
    canonical, digest = canonical_scope(
        {
            "schema_version": 1,
            "tool_name": "hubu_submit_governed_execution",
            "arguments": {"fixture": True},
        }
    )
    with tempfile.TemporaryDirectory(prefix="hubu-operation-key-self-test-") as temporary:
        parent = Path(temporary) / "private"
        parent.mkdir(mode=0o700)
        parent.chmod(0o755)
        database = parent / "operation-keys.sqlite3"
        try:
            connect(database)
        except OperationKeyError:
            pass
        else:
            raise OperationKeyError("unsafe parent permissions were accepted")
        parent.chmod(0o700)

        missing_arguments = parent / "private-arguments-path.json"
        try:
            load_json_object(None, str(missing_arguments), "arguments")
        except OperationKeyError as error:
            if str(missing_arguments) in str(error):
                raise OperationKeyError("file-read diagnostics exposed a local path") from error
        else:
            raise OperationKeyError("missing arguments fixture was accepted")

        with connect(database) as connection:
            identifiers = iter(("a" * 32, "b" * 32))
            payload = begin_with_scope(
                connection,
                database,
                "fixed non-billable self-test",
                canonical,
                digest,
                False,
                lambda: next(identifiers),
            )
            secure_database_files(database)
            row = fetch_record(connection, "hop_" + "a" * 32)
            if row["operation_key"] != "codex:v1:" + "b" * 32:
                raise OperationKeyError("self-test key fixture mismatch")
            if row["record_id"].removeprefix("hop_") == row[
                "operation_key"
            ].removeprefix("codex:v1:"):
                raise OperationKeyError("record references must not reveal operation keys")
            serialized = json.dumps(payload, sort_keys=True)
            for forbidden in (
                "operation_key",
                "codex:v1:",
                "scope_json",
                "arguments",
                "database",
            ):
                if forbidden in serialized:
                    raise OperationKeyError(
                        "redacted self-test output exposed private material"
                    )
            if (
                payload["record_id"] != row["record_id"]
                or payload["scope_sha256"] != digest
            ):
                raise OperationKeyError("redacted self-test reference mismatch")
            for candidate in (database, *database_sidecars(database)):
                if candidate.exists() and candidate.stat().st_mode & 0o077:
                    raise OperationKeyError("self-test database permissions are unsafe")


def scope_arguments(parser: argparse.ArgumentParser) -> None:
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--scope-json", help="Complete immutable spend scope as a JSON object")
    group.add_argument("--scope-file", help="Path to a file containing the scope JSON object")


def unified_scope_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--tool-name", required=True, help="Exact unified MCP tool name")
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--arguments-json", help="Exact unified MCP arguments JSON object")
    group.add_argument("--arguments-file", help="Path to exact unified MCP arguments JSON")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db", help="SQLite path (default: .hubu/operation-keys.sqlite3)")
    commands = parser.add_subparsers(dest="command", required=True)

    begin_parser = commands.add_parser("begin", help="Allocate a key for new logical work")
    begin_parser.add_argument("--label", required=True, help="Short non-secret recovery label")
    scope_arguments(begin_parser)

    begin_unified_parser = commands.add_parser(
        "begin-unified",
        help="Allocate a key for unified MCP without printing the key",
    )
    begin_unified_parser.add_argument(
        "--label", required=True, help="Short non-secret recovery label"
    )
    unified_scope_arguments(begin_unified_parser)

    reuse_parser = commands.add_parser("reuse", help="Recover a key and verify immutable scope")
    reuse_parser.add_argument("--record-id", required=True)
    scope_arguments(reuse_parser)

    reuse_unified_parser = commands.add_parser(
        "reuse-unified",
        help="Verify unified MCP scope without printing the key",
    )
    reuse_unified_parser.add_argument("--record-id", required=True)
    unified_scope_arguments(reuse_unified_parser)

    list_parser = commands.add_parser("list", help="List persisted operation records")
    list_parser.add_argument(
        "--status",
        choices=("active", "completed", "abandoned", "all"),
        default="active",
    )
    list_parser.add_argument(
        "--reference-only",
        action="store_true",
        help="Omit operation keys from output",
    )

    finish_parser = commands.add_parser("finish", help="Mark a terminal operation completed")
    finish_parser.add_argument("--record-id", required=True)
    finish_parser.add_argument(
        "--reference-only",
        action="store_true",
        help="Omit the operation key from output",
    )

    abandon_parser = commands.add_parser("abandon", help="Mark definitely unexecuted work abandoned")
    abandon_parser.add_argument("--record-id", required=True)
    abandon_parser.add_argument(
        "--reference-only",
        action="store_true",
        help="Omit the operation key from output",
    )
    commands.add_parser(
        "self-test",
        help="Validate the redacted bridge with fixed ephemeral fixtures",
    )
    return parser


def main() -> int:
    args = build_parser().parse_args()
    os.umask(0o077)
    if args.command == "self-test":
        try:
            self_test()
        except (OperationKeyError, sqlite3.Error) as error:
            print(f"operation-key self-test error: {error}", file=sys.stderr)
            return 2
        print("operation-key self-test passed")
        return 0

    database = database_path(args.db)
    try:
        with connect(database) as connection:
            if args.command == "begin":
                payload = begin(connection, database, args)
            elif args.command == "begin-unified":
                payload = begin_unified(connection, database, args)
            elif args.command == "reuse":
                payload = reuse(connection, database, args)
            elif args.command == "reuse-unified":
                payload = reuse_unified(connection, database, args)
            elif args.command == "list":
                payload = list_records(connection, database, args)
            elif args.command == "finish":
                payload = transition(
                    connection,
                    database,
                    args.record_id,
                    "completed",
                    not args.reference_only,
                )
            else:
                payload = transition(
                    connection,
                    database,
                    args.record_id,
                    "abandoned",
                    not args.reference_only,
                )
            secure_database_files(database)
    except (OperationKeyError, sqlite3.Error) as error:
        print(f"operation-key error: {error}", file=sys.stderr)
        return 2

    json.dump(payload, sys.stdout, ensure_ascii=False, indent=2, sort_keys=True)
    print()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
