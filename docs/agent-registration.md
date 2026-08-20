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
registration creates a new session.

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
  version content.

Matching identity and version content is reused. Conflicting content is never
silently merged or overwritten.

## Persistence flow

The server flow is:

```text
validate envelope
  -> recompute fingerprints
  -> resolve or create AgentIdentity
  -> resolve or create AgentVersion
  -> resolve or create AgentAccount
  -> create AgentSession
  -> return public identifiers
```

The local server persists registration records in the SQLite database selected
by `HUBU_DB_PATH`, defaulting to `hubu.sqlite3` in the server working directory.
The core manager also has an in-memory store for tests and embedded experiments.

The implementation entry point is
[`crates/hubu-core/src/registration`](../crates/hubu-core/src/registration),
and the HTTP representation is owned by
[`crates/hubu-api`](../crates/hubu-api).
