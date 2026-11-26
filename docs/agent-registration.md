# Agent registration

Hubu registration binds an agent identity and version to an owner, spending
account, and active session. The protocol is designed around one rule:

```text
agent fills, human reviews, server verifies
```

Humans normally provide only an agent name and version label. The client fills
runtime metadata from the active Hubu session, canonicalizes the identity and
version payloads, computes their fingerprints, presents a compact review, and
submits the complete envelope. The server independently canonicalizes and
hashes both payloads before it creates or reuses any record.

## Records and identifiers

One successful registration resolves four records:

- `AgentIdentity` is the logical agent lineage, keyed by its identity
  fingerprint.
- `AgentVersion` is an exact code, model, and runtime configuration within that
  lineage.
- `AgentAccount` is the spending account used by policy, budgets, and spend
  requests.
- `AgentSession` represents the current client connection or invocation
  context.

Every record has an internal UUID-backed ID and a public ID. APIs and CLI output
use public IDs such as `agt_...`, `agv_...`, `aga_...`, and `ags_...`; public
IDs do not encode names, fingerprints, ownership, or model metadata.

Registration is idempotent for identity, version, and account. Each successful
registration creates a new session. The response's `identity_resolution` is
`created` for a new identity, `reused` when the identity fingerprint already
belongs to the owner's agent, or `alias` when it resolved through an owner
rename (see [Renaming an agent](#renaming-an-agent)).

Registration is governance state, not execution-plane startup configuration.
After the stack is running, registering a new agent requires no stack render,
activation, stop, restart, or Gongbu configuration change. Hubu later supplies
that agent's authoritative attribution in each approved spend authorization;
the installation-scoped Gongbu caller capability does not select the agent.

## Guidance-first client flow

Clients must read the compact registration guidance before collecting fields:

```text
hubu registration guidance
GET /.well-known/hubu-agent-registration.json
GET /registration/guidance
```

The guidance object identifies:

- fields a human supplies;
- fields the client derives from the Hubu session and runtime;
- required and optional envelope fields;
- canonicalization and hashing rules;
- fields shown during human review; and
- the submission endpoint and conflict behavior.

Clients should not infer registration requirements from prose or hard-code a
larger questionnaire. The CLI provides the normal low-friction path:

```sh
hubu register agent --name codex-agent --version dev
```

When the runtime can supply stable defaults, `hubu register agent` may infer
both labels and still show the review before submission.

## Registration envelope

The version-1 envelope contains these logical parts:

```json
{
  "protocol_version": "hubu-agent-registration-v1",
  "owner_user_id": "usr_...",
  "identity": {},
  "identity_fingerprint": "sha256:...",
  "version": {},
  "version_fingerprint": "sha256:...",
  "session": {},
  "signature": null
}
```

The identity payload describes the logical agent and its owner-facing labels.
The version payload describes the exact implementation, model, tool/runtime,
and configuration inputs that distinguish one version from another. Session
data is intentionally excluded from both fingerprints so reconnecting does not
create a new identity or version.

The optional `signature` field is reserved for a future protocol revision. A
version-1 server does not treat its presence as proof of authenticity.

## Canonicalization and fingerprints

Clients and the server use the same deterministic procedure:

1. Construct only the documented identity or version payload.
2. Normalize strings and optional values according to registration guidance.
3. Serialize the object as canonical JSON with stable object-key ordering and
   no insignificant whitespace.
4. Hash the resulting UTF-8 bytes with SHA-256.
5. Encode the digest using the guidance-defined fingerprint representation.

Fingerprints cover the payload, not the surrounding envelope. Unknown fields
must not silently influence identity. A client that cannot implement the
advertised canonicalization version must stop rather than submit a guessed
fingerprint.

`identity_fingerprint` is globally unique for a logical agent.
`version_fingerprint` is scoped to one agent lineage, giving the invariant:

```text
(agent_id, version_fingerprint) identifies one AgentVersion
```

## Human review

Before submission, show the human a compact review containing:

- owner identity;
- agent name and type;
- version label;
- relevant implementation, model, and runtime labels;
- the shortened identity and version fingerprints; and
- whether Hubu expects to create or reuse records, when known.

Do not expose credentials, bearer tokens, secret paths, complete environment
snapshots, or unrelated machine metadata in the review or fingerprint payloads.

## Server validation and conflicts

The server validates the protocol version, required payload fields, owner
context, and canonicalization version. It then recomputes both fingerprints
from the submitted payloads and rejects a mismatch before creating or reusing
records.

Registration fails when:

- either fingerprint is empty or malformed;
- a recomputed fingerprint differs from the submitted value;
- an existing identity fingerprint resolves to a different owner or agent
  type; or
- an existing version fingerprint for the same agent resolves to different
  version content; or
- a new identity would reuse the normalized name of another agent of the same
  owner (`display_name_conflict`) or a name reserved by a rename
  (`agent_name_reserved`).

### Unique agent names per owner

Agent names are unique per owner after normalization: names are trimmed and
compared case-insensitively (`hubu_core::registration::normalize_agent_name`,
the same helper used for rename reservations). When a registration would
create a **new** identity whose `display_name` normalizes to an existing
agent's current name for the same owner, the server rejects it with HTTP 409:

```json
{
  "error": "an agent named `alpha` already exists for this owner (agt_...)",
  "error_code": "display_name_conflict",
  "details": { "existing_agent_id": "agt_...", "existing_display_name": "alpha" }
}
```

A name that is a previous name of another agent (see
[Renaming an agent](#renaming-an-agent)) is rejected with
`agent_name_reserved`. Different owners may use the same name, and
re-registering an existing identity is unaffected.

Storage enforces the same rule: `agent_identities.normalized_display_name`
holds the Rust-normalized name and a unique index on
`(owner_user_id, normalized_display_name)` is the backstop for races. There is
no data-preserving migration. If an existing database already contains
duplicate names, the server refuses to start and names the conflicting
`usr_...` owner and `agt_...` agents; rename or remove them, or start from a
fresh `HUBU_DB_PATH`.

Matching identity and version content is reused: registering the same agent
again returns the existing `agt_...`, `agv_...`, and `aga_...` records and a new
`ags_...` session. Before HUB-250 the HTTP API rejected such a repeat with
`agent is already registered for this owner`; it now reuses the records.
Conflicting content is never silently merged or overwritten.

## Renaming an agent

An agent's public `agt_...` ID is permanent. The agent name is part of the
fingerprinted identity payload, so before HUB-250 a corrected name could only be
registered as a separate identity. A human owner can now relabel an agent in
place:

```sh
hubu agent rename --agent-id agt_EXACT_AGENT_ID --name research-agent --reason "fix typo"
hubu agent history --agent-id agt_EXACT_AGENT_ID
```

The CLI calls `POST /agents/rename` with `agent_id`, `name`, and `reason` only.
The route is an owner action: it needs the owner credential and an owner
window, which `hubu-server` opens only after the owner confirms on the Mac
(see [owner presence](owner-presence.md)), so agent sessions cannot rename
themselves. Rename is not exposed through
`hubu-unified-mcp`: there is no rename tool and the unified MCP route allowlist
excludes `/agents/rename` and `/agents/history`.

A rename:

- keeps the internal agent ID and `agt_...`, and therefore the account,
  budgets, holds, policy assignments, versions, sessions, and ledger history;
- copies the stored identity payload, replaces only `agent_name`, and computes
  the new identity fingerprint with the unchanged v1 canonicalization and
  hashing;
- records the previous and the new fingerprint as aliases of the same agent;
- appends an identity revision with the revision number, changed fields
  (old -> new), actor (`usr_...`), timestamp, and reason; and
- updates the current display name shown by `hubu agent list`.

Owner and `agent_kind` cannot be edited; the request rejects any field other
than `agent_id`, `name`, and `reason`. Version payloads (`agv_...`) stay
immutable: model or runtime changes are a new version, not an edit.

A rename is rejected (HTTP 409) when the new identity fingerprint, or the
normalized new name, already belongs to a different agent of the same owner.
Names are normalized by trimming and comparing case-insensitively
(`hubu_core::registration::normalize_agent_name`). Previous names stay
reserved for their agent: another agent of the same owner cannot be renamed
into one, and registration cannot create a new agent under one. An agent may
rename back to one of its own previous names. Agents are never merged.

### Re-registration after a rename

Registration resolves the submitted identity fingerprint through the alias
table before creating anything:

| Submitted identity | Result |
| --- | --- |
| Current name of a renamed agent | Same agent and account; identical version payloads reuse the same `agv_...`. `identity_resolution: "alias"`, no warning. |
| A previous name of a renamed agent | Same agent and account, `identity_resolution: "alias"`, plus a `stale_agent_identity` warning naming the current name. No identity, name change, or revision is created. |
| Fingerprint of a never-renamed agent | Same agent, version, and account (`identity_resolution: "reused"`), new session, no warning. |

Clients should show the warning to the human and update their configured agent
name. The registration guidance exposes these rules under
`identity_resolution` and `rename`.

Agents registered before identity payloads were stored are renamed by
rebuilding the payload Hubu's own clients send and accepting it only when it
reproduces the stored fingerprint exactly. Otherwise the rename fails with
`agent_rename_identity_payload_unavailable`.

## Per-agent credentials

Since v0.3.1 (HUB-357) each agent can hold its own credential, bound to its one
account. The v0.3.1 tech spec, section 5, is the design; this section describes
what the server enforces today.

The client mints the secret and Hubu never sees it. A credential is
`hubu_agent_<credential_id>_<secret>`: a 16-byte id and a 32-byte secret, both
lowercase base32 without padding. The client writes it to the agent's own
credential file (mode 0600) and registers with only the id and the
verification hash, `sha256:` plus the hex SHA-256 of the secret:

```json
{
  "name": "design",
  "version": "v1",
  "agent_credential": {
    "credential_id": "<26 base32 characters>",
    "verification_hash": "sha256:<64 hex characters>",
    "harness": "claude"
  }
}
```

`agent_credential` sits beside either the envelope or the simple fields.

| Step | Route | Who |
|---|---|---|
| Record a registration credential (`hubu_reg_...`) minted by `hubu init <harness>` | `POST /agents/registration-credentials` with `harness`, `credential_id`, `verification_hash` | Owner credential; no prompt, recorded as `registration_credential.create` |
| Register an agent with its credential hash | `POST /agents/register` | Registration credential (must bring `agent_credential`), owner, or the agent's own credential |
| Confirm one or more pending agents | `POST /agents/confirm` with `agent_ids` | Owner action (`agent.confirm`), one dialog for the whole list |
| Revoke an agent's credential | `POST /agents/credential/revoke` with `agent_id` | Owner action (`agent.credential.revoke`) |
| Replace an agent's credential | `POST /agents/credential/rotate` with `agent_id`, `credential_id`, `verification_hash` | Owner action (`agent.credential.rotate`) |

Rules:

- A new credential is **pending**. Every request with it is refused with
  `agent_pending_owner_confirmation` until the owner confirms. At most 10
  registrations wait at once (`too_many_pending_agents`), and an unconfirmed
  one expires after 7 days.
- A registration credential reaches only `POST /agents/register`; every other
  route answers `registration_credential_scope`. Recording a new one for the
  same harness revokes the previous one.
- Re-registering the same identity with the same credential returns the
  existing ids. A different credential while one is pending or active is
  refused with `agent_already_registered`; the owner rotates or revokes
  instead. After a revoke the agent can register a new credential, which waits
  for confirmation again. An agent's own credential can re-register only its
  own agent.
- A revoked credential is refused on its next request with
  `agent_credential_revoked`. Budget, ledger and ids stay.
- On `POST /spend/authorize`, `/spend` and `/spend/void`, an agent credential
  that names another account is refused with `agent_account_mismatch` before
  anything is reserved. The shared v0.3.0 agent credential no longer works for
  an account that has a credential of its own; agents without one keep it until
  the migration retires it.
- `GET /agents` and the registration response report `credential_status`:
  `pending`, `active`, `revoked` or `expired`.

Hubu stores the verification hash in `agent_credentials` and
`registration_credentials`, never a credential value.

## Persistence flow

The server flow is:

```text
validate envelope
  -> recompute fingerprints
  -> resolve identity fingerprint through rename aliases
  -> resolve or create AgentIdentity
  -> resolve or create AgentVersion
  -> resolve or create AgentAccount
  -> create AgentSession
  -> return public identifiers
```

The local server persists registration records in the SQLite database selected
by `HUBU_DB_PATH`, defaulting to `hubu.sqlite3` in the server working directory.
Rename support adds `agent_identities.identity_payload_json` and two
append-only tables, `agent_identity_aliases` and `agent_identity_revisions`,
created idempotently at startup; triggers reject updates and deletes.
The core manager also has an in-memory store for tests and embedded experiments.

The implementation entry point is
[`crates/hubu-core/src/registration`](../crates/hubu-core/src/registration),
and the HTTP representation is owned by
[`crates/hubu-api`](../crates/hubu-api).
