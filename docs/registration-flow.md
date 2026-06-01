# Registration Flow

Registration turns an MCP client connection into four records:

- `AgentIdentity`: the logical agent, resolved by `identity_fingerprint`.
- `AgentVersion`: the exact code/model/runtime version, resolved inside one agent lineage.
- `AgentAccount`: the spending account for the agent.
- `AgentSession`: the current connection/session.

The prototype keeps all records in memory with `HashMap` indexes. The shape is
intended to move cleanly to storage later.

Each record has an internal UUID-backed `id` and an external `pub_id`.
Internally, Hubu uses the UUID as the stable unique identifier. Public API and
CLI surfaces use the shorter public IDs with meaningful prefixes, such as
`agt_codex_agent`, `agv_demo_agent_codex_agent_1_0`, `aga_agt_codex_agent`, and
`ags_000001`.

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
