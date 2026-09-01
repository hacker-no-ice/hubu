---
name: generate-hubu-operation-key
description: Generate, persist, recover, and safely reuse a Hubu spend operation key. Use when Codex must supply an operation_key for a new billable operation, retry an ambiguous Hubu request, recover an operation after context or process loss, or create distinct keys for separate provider attempts.
---

# Generate Hubu Operation Key

Use the bundled `scripts/operation_keys.py` helper. Do not invent a key in conversation memory or run a bare UUID command.

Treat this workflow as protection against accidental duplicate operations. Do not describe a model-managed key as trusted platform identity.

## Use the key-redacted unified MCP bridge

When `hubu-unified-mcp` is configured with an absolute
`HUBU_UNIFIED_OPERATION_KEY_DB` path, keep the helper database at that path and
let the router atomically claim the matching record before reading its key. Use
an owner-only directory. The helper uses a restrictive process umask and secures
the database plus SQLite WAL/SHM sidecars to owner-only mode; the router rejects
symlinks, non-files, and group- or world-accessible directories, stores, or
sidecars. Trusted `_meta.callId` remains the operation identity; the helper
record supplies scoped key material only. Never copy the raw key into tool
arguments, `_meta`, chat, shell arguments, environment variables, config, logs,
or evidence.

Only after the human has authorized the live operation, write the exact public MCP arguments to a local file and allocate one record with the key-redacted mode:

```bash
python3 skills/generate-hubu-operation-key/scripts/operation_keys.py \
  --db /absolute/private/path/operation-keys.sqlite3 \
  begin-unified \
  --label "HUB-172 one approved generation" \
  --tool-name hubu_submit_governed_execution \
  --arguments-file /absolute/private/path/governed-arguments.json
```

The command builds and binds the canonical scope
`{"schema_version":1,"tool_name":TOOL,"arguments":ARGUMENTS}`. It generates the
record reference and operation key from independent random identifiers, so the
redacted record ID cannot be used to derive the key. Its minimal output contains
the record ID, scope hash, status, label, and timestamps; it omits the operation
key, full scope/arguments, and database path. A new trusted `callId` succeeds
only when the configured store has exactly one active, valid record for that
scope. The router validates both independent identifier formats, rejects a
record whose suffix matches the key suffix, then atomically claims the helper
record for its stable registry installation, trusted call identity, and exact
request hash before durably binding the key in the router registry. A crash
between those two local writes can recover only with that same binding. A
different call or router registry cannot consume the record or key.

For an exact retry or recovery, reuse the original trusted `callId`; do not allocate another record. The router replays its durable binding without consulting the helper database. To verify a recovered record and arguments locally without printing the key, run `reuse-unified` with the original record ID and the same `--tool-name` and `--arguments-file`. Any scope mismatch is a stop condition.

Recover active record references without exposing keys, scopes, or paths:

```bash
python3 skills/generate-hubu-operation-key/scripts/operation_keys.py \
  --db /absolute/private/path/operation-keys.sqlite3 \
  list --status active --reference-only
```

After verified terminal settlement or release, close the exact record without emitting private material:

```bash
python3 skills/generate-hubu-operation-key/scripts/operation_keys.py \
  --db /absolute/private/path/operation-keys.sqlite3 \
  finish --record-id hop_RECORD_ID --reference-only
```

Use `abandon --reference-only` only when the operation is definitively canceled before authorization or billable execution. Never abandon an ambiguous outcome.

## Start one logical operation

1. Build a JSON object containing the immutable spend scope. Include every applicable identity and spend field: `agent_id`, `account_id`, `amount`, `currency`, `merchant`, `reason`, `lease_profile`, provider, model, and target.
2. Exclude credentials, authorization tokens, raw provider payloads, and other secrets.
3. Run `begin` exactly once before the first authorization or billable call:

```bash
python3 skills/generate-hubu-operation-key/scripts/operation_keys.py begin \
  --label "gemini logo candidate" \
  --scope-json '{"account_id":"ACCOUNT","amount":5,"merchant":"gongbu.image","provider":"gemini","reason":"Generate logo candidate"}'
```

4. Immediately use the returned `operation_key` and retain the returned `record_id` for retries and recovery.

`begin` always creates a new operation. Run it again only for intentionally distinct work, including a second provider candidate or a new attempt that requires separate authorization.

## Retry or resume

Run `reuse` with the original record and the complete current scope:

```bash
python3 skills/generate-hubu-operation-key/scripts/operation_keys.py reuse \
  --record-id hop_RECORD_ID \
  --scope-json '{"account_id":"ACCOUNT","amount":5,"merchant":"gongbu.image","provider":"gemini","reason":"Generate logo candidate"}'
```

Use the returned key for authorization, claim, settlement, release, and every exact retry of that logical operation. The helper rejects changed scope locally. If Hubu returns `retry_guidance.action = reuse_operation_key` after a terminal denial, keep the same operation key for the corrected authorization request; Hubu's SQLite admission check remains the authority. Never correct scope when guidance says `replay_exactly` or `create_new_operation`.

After context or process loss, run:

```bash
python3 skills/generate-hubu-operation-key/scripts/operation_keys.py list --status active
```

Inspect active records and reuse the matching record. Do not create a replacement merely because a previous response was lost or ambiguous.

## Close the local record

After a terminal settlement or release, run:

```bash
python3 skills/generate-hubu-operation-key/scripts/operation_keys.py finish --record-id hop_RECORD_ID
```

If work is definitively canceled before authorization or billable execution, run `abandon` instead. Never abandon an operation with an ambiguous provider or financial outcome.

## Persistence

Store records in `.hubu/operation-keys.sqlite3` under the current working
directory by default. Its containing directory must be owner-only. Override
this only when needed with `--db PATH` or `HUBU_OPERATION_KEY_DB`.

Preserve the database across Codex and process restarts. Never commit it.
