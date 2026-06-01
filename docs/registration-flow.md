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
CLI surfaces use shorter public IDs with meaningful prefixes, such as
`agt_1p8x7k2m4q9v1c6d`, `agv_0r5f0p3tn8wqj2az`,
`aga_3hc6q3d9m1v8ra7k`, and `ags_9g2h7rx0cq4p9w5e`.

The public ID suffix is derived from the internal UUID by collecting 80 random
UUIDv4 bits while skipping the fixed version nibble and variant bits, then
encoding those bits as 16 groups of 5 bits with the Crockford-style base-32
alphabet `0123456789abcdefghjkmnpqrstvwxyz`. The suffix is therefore short and
copyable, but it does not encode display name, fingerprint, model, owner, or
other agent metadata.

The internal UUID remains the source of identity. A UUIDv4 has roughly 122
random bits, while the public suffix currently carries 80 random UUID-derived bits.
Using birthday-bound estimates, public-ID collision probability is about
`n^2 / (2 * 2^80)` for `n` IDs with the same prefix. That is roughly
0.00000000004% at 1 million IDs, 0.000000004% at 10 million IDs,
0.0000004% at 100 million IDs, and 0.00004% at 1 billion IDs. Revisit this
once public IDs approach large multi-million scale, and enforce a storage-level
unique constraint on `pub_id` with retry metrics before moving beyond the
in-memory prototype.

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
