# Agent Registration Protocol

This document defines the first Hubu agent registration protocol. The goal is
to let an agent prepare registration details with little human friction while
giving the server enough structured data to verify fingerprints and detect
conflicts.

The core rule is:

```text
agent fills, human reviews, server verifies
```

## Design Goals

- Keep the first Codex-agent registration flow small and easy to try.
- Give agents precise guidance for filling registration fields.
- Let Hubu clients compute stable identity and version fingerprints.
- Let the Hubu server recompute fingerprints from the submitted content.
- Avoid requiring cryptographic signatures in v1, while leaving a clean place
  to add them later.

## Human Flow

A new user should not need to understand fingerprints. For the initial CLI and
Codex-agent path, the human can prepare only:

```text
agent name
version label
```

Example:

```sh
hubu register agent --name codex-agent --version dev
```

An agent can also let the CLI infer both fields:

```sh
hubu register agent
```

By default, the CLI binds the registration to the current Hubu user context,
uses the guidance-provided
`agent_name.default_template` as `agent_name` and the current git short SHA as
`version_label`, falling back to `dev` when git is not available. In the demo
guidance, the template is `{vendor}-{workspace}` and the vendor is `codex`, so
the default name in this repository is `codex-hubu`. Other agents can publish
their own vendor/runtime identity through guidance. To inspect the computed
protocol envelope before submitting:

```sh
hubu register agent --dry-run
```

The Hubu client includes the current owner context in the registration envelope,
computes fingerprints, and prints a compact review when submitting. Agents and
humans can use `--dry-run` to inspect the full envelope before submission.

Example review:

```text
Registration review
  agent_name: codex-agent
  owner: Alice Example (usr_...)
  agent_kind: codex_agent
  version_label: dev
  runtime.provider: codex
  runtime.environment: development
  identity_fingerprint: sha256:...
  version_fingerprint: sha256:...
```

The human or agent can override the inferred name/version, run `--dry-run`, or
cancel before submission. Advanced clients may also expose editable code, model,
runtime, tool, or permission fields, but those fields should not be required for
the low-friction path.

The demo CLI keeps the same low-friction defaults for name and version:

```sh
hubu register agent
```

## Registration Envelope

The client submits one envelope:

```json
{
  "protocol_version": "hubu-agent-registration-v1",
  "identity": {
    "payload": {},
    "fingerprint": "sha256:..."
  },
  "version": {
    "payload": {},
    "fingerprint": "sha256:..."
  },
  "review": {
    "display_name": "codex-agent",
    "description": null
  },
  "signature": null
}
```

`review` fields are human-facing metadata. They can be stored on the resulting
Hubu records, but they do not define identity unless they are also present in
the fingerprinted payloads.

## Identity Payload

The identity payload answers: "Who is this logical agent?"

For v1, these fields are required:

```json
{
  "protocol_version": "hubu-agent-registration-v1",
  "owner": {
    "type": "hubu_user",
    "pub_id": "usr_6qqcj94w6pr5"
  },
  "agent_name": "codex-agent",
  "agent_kind": "codex_agent"
}
```

Field meanings:

- `protocol_version`: registration protocol version used for canonicalization
  and hashing.
- `owner.type`: owner namespace. Initial value: `hubu_user`.
- `owner.pub_id`: public Hubu user ID that owns the agent. The client should
  fetch or infer this from the active Hubu session so the human does not need to
  type it.
- `agent_name`: stable, user-chosen logical name.
- `agent_kind`: agent category. Initial value: `codex_agent`.

Optional identity fields may be included when known:

```json
{
  "source_repository_url": "https://github.com/example/agent",
  "package_ref": "github.com/example/agent",
  "issuer": "github.com/example",
  "agent_public_key_id": "did:key:..."
}
```

Optional fields are fingerprinted when present. Clients should omit unknown
fields instead of sending empty strings.

The identity fingerprint is:

```text
sha256:<hex sha256 of canonical identity payload>
```

## Version Payload

The version payload answers: "What exact version/configuration is this agent
running?"

For v1, these fields are required:

```json
{
  "protocol_version": "hubu-agent-registration-v1",
  "identity_fingerprint": "sha256:...",
  "version_label": "dev",
  "runtime": {
    "provider": "codex",
    "environment": "development"
  },
  "hubu_client": {
    "name": "hubu-cli",
    "version": "0.1.0"
  }
}
```

Field meanings:

- `identity_fingerprint`: fingerprint of the identity payload in the same
  envelope.
- `version_label`: user- or agent-provided release label such as `dev`, `v1`,
  or a commit-like label.
- `runtime.provider`: runtime that prepared the registration. Initial value:
  `codex`.
- `runtime.environment`: `development`, `staging`, or `production`.
- `hubu_client.name`: client that prepared the envelope.
- `hubu_client.version`: client version that prepared the envelope.

Optional version fields may be included when known:

```json
{
  "code": {
    "repository_url": "https://github.com/example/agent",
    "commit_sha": "abc123",
    "artifact_digest": "sha256:..."
  },
  "model": {
    "provider": "openai",
    "model": "gpt-5",
    "version": "2026-06-03"
  },
  "tool_manifest_digest": "sha256:...",
  "permission_manifest_digest": "sha256:...",
  "config_digest": "sha256:..."
}
```

Optional fields are fingerprinted when present. Secrets must never be included
in the payload. If configuration affects behavior, hash a redacted config
document and submit only the digest.

The version fingerprint is:

```text
sha256:<hex sha256 of canonical version payload>
```

## Canonicalization

Hubu v1 uses canonical JSON for fingerprints.

Rules:

- Encode payloads as UTF-8 JSON objects.
- Sort object keys lexicographically at every level.
- Preserve array order.
- Use the shortest JSON representation for strings, numbers, booleans, and
  nulls accepted by the implementation.
- Omit unknown or unavailable optional fields.
- Do not include empty strings for unknown values.
- Do not include server-generated IDs, timestamps, account IDs, session IDs, or
  signatures in fingerprint payloads.

The client computes fingerprints from canonical payloads. The server applies the
same canonicalization and recomputes both fingerprints.

## Server Validation

On registration, the server must:

1. Parse the registration envelope.
2. Validate `protocol_version`.
3. Resolve `identity.payload.owner.pub_id` to a registered Hubu user.
4. Canonicalize `identity.payload`.
5. Recompute `identity.fingerprint`.
6. Reject the request if the recomputed value does not match.
7. Confirm `version.payload.identity_fingerprint` matches
   `identity.fingerprint`.
8. Canonicalize `version.payload`.
9. Recompute `version.fingerprint`.
10. Reject the request if the recomputed value does not match.
11. Resolve or create `AgentIdentity`.
12. Resolve or create `AgentVersion` within that identity.
13. Resolve or create `AgentAccount`.
14. Create a fresh `AgentSession`.

Registration remains idempotent for identity, version, and account. It always
creates a new session.

## Conflict Rules

Registration fails when:

- either fingerprint is missing or invalid
- the server-recomputed identity fingerprint differs from the submitted value
- the server-recomputed version fingerprint differs from the submitted value
- the version payload references a different identity fingerprint
- an existing identity fingerprint for the owner maps to different identity
  content
- an existing version fingerprint for the agent maps to different version
  content

## Guidance Interface

Hubu publishes compact registration guidance so agents can fill envelopes
without reading this prose document or hard-coding assumptions. The prose
protocol is for humans and implementers. Agents should consume a small
structured guidance object.

Available interfaces:

```sh
hubu protocol agent-registration
```

or HTTP:

```text
GET /registration/guidance
GET /.well-known/hubu-agent-registration.json
```

The guidance response should be directly actionable:

```json
{
  "protocol_version": "hubu-agent-registration-v1",
  "fingerprint": {
    "algorithm": "sha256",
    "encoding": "hex",
    "prefix": "sha256:",
    "canonicalization": "canonical_json_v1"
  },
  "signature_policy": "not_supported",
  "human_inputs": [
    {
      "name": "agent_name",
      "required": true,
      "prompt": "Agent name",
      "default_strategy": "workspace_or_runtime_name"
    },
    {
      "name": "version_label",
      "required": true,
      "prompt": "Version",
      "default_strategy": "git_commit_or_dev"
    }
  ],
  "client_filled": {
    "agent_identity.vendor": "codex",
    "agent_name.default_template": "{vendor}-{workspace}",
    "agent_kind": "codex_agent",
    "runtime.provider": "codex",
    "runtime.environment": "development",
    "hubu_client.name": "current_client_name",
    "hubu_client.version": "current_client_version"
  },
  "identity_payload": {
    "required": [
      "protocol_version",
      "owner",
      "agent_name",
      "agent_kind"
    ],
    "optional": [
      "source_repository_url",
      "package_ref",
      "issuer",
      "agent_public_key_id"
    ]
  },
  "version_payload": {
    "required": [
      "protocol_version",
      "identity_fingerprint",
      "version_label",
      "runtime",
      "hubu_client"
    ],
    "optional": [
      "code",
      "model",
      "tool_manifest_digest",
      "permission_manifest_digest",
      "config_digest"
    ]
  },
  "review_fields": [
    "agent_name",
    "owner",
    "agent_kind",
    "version_label",
    "runtime.provider",
    "runtime.environment",
    "code.repository_url"
  ]
}
```

An agent can follow this response mechanically:

1. Collect only `human_inputs` that it cannot infer safely.
2. Put the current Hubu user context into `identity_payload.owner.pub_id`.
3. Fill `client_filled` values from the active runtime.
4. Add optional fields only when confidently known.
5. Canonicalize and hash `identity_payload`.
6. Put that fingerprint into `version_payload.identity_fingerprint`.
7. Canonicalize and hash `version_payload`.
8. Show the compact `review_fields` summary to the human.
9. Submit the envelope after confirmation.

For v1, `signature_policy` should be `not_supported`.

## Signature Extension

Cryptographic signatures are deferred in v1. The envelope reserves a
`signature` field for future authenticity checks:

```json
{
  "algorithm": "ed25519",
  "key_id": "did:key:...",
  "signed_payload": "registration_envelope_without_signature",
  "signature": "base64..."
}
```

Fingerprints provide integrity and stable identity keys. Signatures will provide
authenticity once Hubu supports public key discovery and trust policies.

The future signature should cover the canonical registration envelope excluding
the `signature` field itself.

## Minimal Codex-Agent V1

For the initial use case, a Codex agent can prepare:

```json
{
  "protocol_version": "hubu-agent-registration-v1",
  "identity": {
    "payload": {
      "protocol_version": "hubu-agent-registration-v1",
      "owner": {
        "type": "hubu_user",
        "pub_id": "usr_6qqcj94w6pr5"
      },
      "agent_name": "codex-agent",
      "agent_kind": "codex_agent"
    },
    "fingerprint": "sha256:..."
  },
  "version": {
    "payload": {
      "protocol_version": "hubu-agent-registration-v1",
      "identity_fingerprint": "sha256:...",
      "version_label": "dev",
      "runtime": {
        "provider": "codex",
        "environment": "development"
      },
      "hubu_client": {
        "name": "hubu-cli",
        "version": "0.1.0"
      }
    },
    "fingerprint": "sha256:..."
  },
  "review": {
    "display_name": "codex-agent",
    "description": null
  },
  "signature": null
}
```

The agent computes and fills both fingerprints, shows the human the compact
review, then submits the envelope. The Hubu server recomputes the same
fingerprints before creating or reusing registration records.
