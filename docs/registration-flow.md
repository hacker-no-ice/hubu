# Registration Flow

Registration turns an MCP client connection into four records:

- `AgentIdentity`: the logical agent, resolved by `identity_fingerprint`.
- `AgentVersion`: the exact code/model/runtime version, resolved inside one agent lineage.
- `AgentAccount`: the spending account for the agent.
- `AgentSession`: the current connection/session.

See [Agent Registration Protocol](agent-registration-protocol.md) for the v1
client/server protocol, fingerprint payloads, canonicalization rules, and
low-friction human review flow.

The local server persists registration records in the shared SQLite
database selected by `HUBU_DB_PATH`, defaulting to `hubu.sqlite3` in the server
working directory. The core registration manager also keeps an in-memory store
for unit tests and embedded experiments.

Each record has an internal UUID-backed `id` and an external `pub_id`.
Internally, Hubu uses the UUID as the stable unique identifier. Public API and
CLI surfaces use shorter public IDs with meaningful prefixes, such as
`agt_8x7k2m4q9v1c`, `agv_5f0p3tn8wqj2`, `aga_c6q3d9m1v8ra`, and
`ags_2h7rx0cq4p9w`. The suffix is derived from the internal UUID, so it is
short and copyable but does not encode display name, fingerprint, model, owner,
or other agent metadata.

## Flow

```text
RegisterAgentRequest
  -> validate request
  -> resolve/create AgentIdentity
  -> resolve/create AgentVersion
  -> resolve/create AgentAccount
  -> create AgentSession
  -> RegisterAgentResponse
```

## Fingerprints

`identity_fingerprint` is globally unique for one logical agent. If the same
identity fingerprint is seen again, the stored identity is reused only when key
identity fields still match.

`version_fingerprint` is scoped to one agent. The same version fingerprint can
exist in two different agent lineages, but one agent cannot use the same version
fingerprint for different code/model/runtime config.

This gives the important invariant:

```text
(agent_id, version_fingerprint) identifies one AgentVersion
```

Here, `agent_id` means the internal UUID-backed `AgentId`. Public requests pass
the `agt_...` public agent ID, which the API resolves before invoking core
policy, spend, wallet, or registration internals.

## Conflict Handling

Registration fails when:

- either fingerprint is empty
- an existing identity fingerprint resolves to a different owner or agent type
- an existing version fingerprint for the same agent resolves to different config

Successful registration is idempotent for identity, version, and account. It
always creates a new session.
