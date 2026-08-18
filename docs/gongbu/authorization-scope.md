# Hubu authorization scope preview

Before issuing a Hubu token, an authenticated operator or agent platform asks
Gongbu to derive the exact authorization request for the planned execution:

```http
POST /v1/authorization-scopes/preview
Authorization: Bearer OPERATOR_GONGBU_CAPABILITY
Content-Type: application/json

{
  "schema_version": 1,
  "operation_key": "codex:tool-call:01JABC123",
  "input": {"prompt": "Draw a blue circle", "image_count": 1},
  "input_schema_version": 1,
  "workload_type": "image_generation",
  "provider": "google",
  "adapter": "gemini_developer_image",
  "model": "gemini-3.1-flash-image-preview"
}
```

The response contains a versioned `authorization_scope` audit object and a
`hubu_authorize_request` ready for `POST /spend/authorize`. The audit object
shows the agent binding; the Hubu request correctly supplies only `account_id`,
because Hubu resolves the registered agent from that operator-owned account and
rejects caller-supplied `agent_id`. Gongbu derives the
account from the authenticated principal, the agent from server configuration,
the amount and currency from the pinned pricing catalog, the typed
`execution_scope` from the operator-selected target, and expiry guidance from
Hubu's discovered workload timing. The caller supplies only the planned work
and immutable operation key; it cannot replace or broaden those operator-owned
fields.
The request body is also available as
[`examples/gongbu/authorization-scope-preview.json`](../../examples/gongbu/authorization-scope-preview.json).

For v1, `task.task_id`, `task.reason`, the Hubu authorization `reason`, and the
Gongbu `operation_key` are the same exact string. The workload profile is the
canonical Gongbu workload type (`image_generation` for image execution). The
complete cross-component fixture is
[`fixtures/hubu-authorization-scope-v1.json`](../../fixtures/hubu-authorization-scope-v1.json).

The same operation is exposed by the `gongbu_preview_authorization_scope` MCP
tool. After Hubu returns a token, submit the execution with the same operation
key and planned target. Gongbu recomputes the canonical scope and validates the
token with Hubu before it persists or schedules the execution. A mismatch
returns authenticated `authorization_scope_mismatch` diagnostics and no
provider work can start. Ordinary execution failure responses remain coarse and
do not expose Hubu response bodies.

At startup and during dependency health checks, Gongbu also verifies Hubu's
executor contract, authorization-scope schema version, execution-scope schema
and catalog entries, required workload timing profiles, and the configured
active account-to-agent registration binding. Legacy Hubu
authorization and execution requests remain readable, but production Gongbu no
longer invents or sends a raw `gongbu.execution` merchant fallback.
