---
name: generate-hubu-operation-key
description: Generate, persist, recover, and safely reuse a Hubu spend operation key. Use when Codex must supply an operation_key for a new billable operation, retry an ambiguous Hubu request, recover an operation after context or process loss, or create distinct keys for separate provider attempts.
---

# Generate Hubu Operation Key

Use the bundled `scripts/operation_keys.py` helper. Do not invent a key in conversation memory or run a bare UUID command.

Treat this workflow as protection against accidental duplicate operations. Do not describe a model-managed key as trusted platform identity.

## Start one logical operation

1. Build a JSON object containing the immutable spend scope. Include every applicable identity and spend field: `agent_id`, `account_id`, `amount`, `currency`, `merchant`, `reason`, `workload_profile`, provider, model, and target.
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

Store records in `.hubu/operation-keys.sqlite3` under the current working directory by default. Override this only when needed with `--db PATH` or `HUBU_OPERATION_KEY_DB`.

Preserve the database across Codex and process restarts. Never commit it.
