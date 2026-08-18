# Unified Hubu–Gongbu MCP contract

Status: accepted design for `hubu-gongbu-mcp-v1`; routing is not implemented by
this document.

This document fixes the public names, schemas, ownership, discovery handshake,
compatibility rules, and standalone-server migration gates for one supported MCP
surface. The surface is an adapter and router only. It does not merge Hubu and
Gongbu processes or allow either backend to import the other.

## Decision

The supported unified binary and MCP server are named `hubu-unified-mcp`; it
reports `serverInfo.name = "hubu-unified-mcp"`. The distinct name is required
because `hubu-mcp-server` and `gongbu-mcp` are the standalone compatibility
surfaces. The unified server exposes the union of their current
ownership-qualified `hubu_*` and `gongbu_*` names unchanged. Existing names are
already collision-free and make the security boundary visible; a new generic
tool namespace would add migration cost while hiding ownership.

The router adds one local read-only tool, `hubu_unified_capabilities`. It owns no
governance or execution data. A tool call is validated against the public schema
and forwarded to exactly one owning backend. The router must not compose a
governance mutation and an execution mutation into one tool call.

The existing `hubu-mcp-server` and `gongbu-mcp` binaries remain supported during
the parity and deprecation gates below. Default client configuration may switch
to `hubu-unified-mcp` only at the packaging and migration gates; neither
standalone binary is replaced or removed by this design.

## Boundaries and non-goals

- Hubu remains the control plane and sole owner of humans, agents, policies,
  budgets, spending targets, authorizations, executor claims, reconciliation,
  payments, and the ledger.
- Gongbu remains the execution plane and sole owner of provider credentials,
  provider calls and retries, execution state, Temporal state, provider pricing,
  artifact bytes, and execution recovery.
- The processes have separate databases, bearer credentials, configuration,
  lifecycle supervision, readiness, and failure domains. The unified adapter
  holds two separately configured backend clients and never forwards one
  backend credential to the other.
- The Rust workspace remains unified, but the router must not create a direct
  Cargo dependency between Hubu and Gongbu domain or server crates. Backend
  communication stays HTTP and uses the versioned
  `hubu-spend-executor-v4.2` contract where Hubu and Gongbu interact.
- Routing does not copy artifact bytes into Hubu or provider credentials into
  either the router or Hubu. `gongbu_get_artifact` streams its existing safe MCP
  content from Gongbu.
- Provider selection, authorization, operation-key allocation, retries, and
  cross-tool workflow orchestration are not added to the router.

## Common MCP behavior

The server implements MCP JSON-RPC `initialize`, `ping`, `tools/list`, and
`tools/call` over stdio using protocol version `2024-11-05`. Unknown methods use
`-32601`; malformed call parameters use `-32602`. The router advertises
`capabilities.tools.listChanged = true` because backend readiness can change the
callable catalog.

All input schemas below are JSON Schema objects with
`additionalProperties: false`. `required(...)` lists the schema-required fields;
other listed fields are optional. The router must publish the full constraints
from the owning standalone adapter, not merely the compact notation in the
tables. It must preserve Hubu tool annotations, including approval metadata.
Gongbu tools are open-world operations but accept no endpoint, credential,
account, amount, operation-key, retry, or artifact-path overrides.

Successful backend calls preserve the standalone MCP result shape byte-for-byte
at the JSON value level:

- Hubu results contain a text block with pretty-printed JSON and identical JSON
  in `structuredContent`. Spend results retain `requires_human_approval` and
  `approval_reason` behavior.
- Gongbu JSON results contain one compact JSON text block and `isError: false`.
  `gongbu_get_artifact` contains the existing safe metadata text block followed
  by the image content block. Gongbu application errors remain `isError: true`
  with `{ "error": { "code", "message" } }` in the text block.

The router must not add a success envelope, rename response fields, translate
currency units, or convert a Gongbu tool error into a successful Hubu-style
payload. The schema identifiers in the routing tables name these existing
result contracts.

## Tool routing map

### Router-owned discovery

| Unified tool | Input | Response | Owner and route |
| --- | --- | --- | --- |
| `hubu_unified_capabilities` | `{}` | `UnifiedCapabilitiesV1` | unified adapter; local snapshot, no backend call |

The tool is read-only and returns the object as both `structuredContent` and a
JSON text block. `UnifiedCapabilitiesV1` is:

```json
{
  "contract_version": "hubu-gongbu-mcp-v1",
  "routing_revision": 1,
  "generated_at": "RFC3339 timestamp",
  "backends": {
    "hubu": {
      "state": "available | degraded | unavailable | incompatible | unconfigured",
      "product_version": "string or null",
      "source_commit": "string or null",
      "contract_versions": { "executor": "string or null" },
      "reason_code": "string or null"
    },
    "gongbu": {
      "state": "available | degraded | unavailable | incompatible | unconfigured",
      "product_version": "string or null",
      "source_commit": "string or null",
      "api_schema_version": "integer or null",
      "mcp_schema_version": "integer or null",
      "contract_versions": { "executor": "string or null" },
      "reason_code": "string or null"
    }
  },
  "tools": [
    { "name": "string", "owner": "router | hubu | gongbu", "available": true,
      "reason_code": "string or null" }
  ]
}
```

The response contains all 33 unified names, including unavailable backend tools,
in lexical order. It contains no endpoint, credential, account claim, provider
secret, or raw backend error. `generated_at` describes the snapshot and is not a
compatibility input.

### Hubu-owned governance tools

Each route is made with only the Hubu backend credential. `HubuJson(T)` means
the existing Hubu MCP result with HTTP response `T` as `structuredContent` and
as the text block. Source response structures remain authoritative in
[`crates/hubu-api/src/lib.rs`](../crates/hubu-api/src/lib.rs).

| Unified and standalone name | Exact input schema (compact) | Response schema | Hubu route |
| --- | --- | --- | --- |
| `hubu_health` | `{}` | `HubuJson({status:string})` | `GET /health` |
| `hubu_registration_guidance` | `{}` | `HubuJson(RegistrationGuidance)` | `GET /registration/guidance` |
| `hubu_client_approval_profile` | `{}` | `HubuJson(ClientApprovalProfile)` | router copy of Hubu MCP profile; Hubu-owned contract |
| `hubu_list_users` | `{}` | `HubuJson(UserList)` | `GET /users` |
| `hubu_register_human` | `{username?:string, display_name?:string, email?:string}` | `HubuJson(InitUser)` | `POST /init` |
| `hubu_register_agent` | `{owner_user_id?:string, name?:string, version?:string}` | `HubuJson(RegisteredAgent)` | `POST /agents/register` |
| `hubu_add_policy` | `{policy_yaml?:string, daily_limit_cents?:integer}` | `HubuJson(AppliedPolicy)` | `POST /policies` |
| `hubu_apply_policy` | `{policy_yaml:string, declarative_key?:string, display_name?:string, agent_id?:string, expected_revision?:integer, expected_hash?:string}`; `required(policy_yaml)` | `HubuJson(AppliedPolicy)` | `POST /policies` with router-owned `source:"mcp"` |
| `hubu_show_policy` | `{policy_id?:string, agent_id?:string}` | `HubuJson(PolicyView)` | `GET /policies/show` |
| `hubu_export_policy` | `{policy_id?:string, agent_id?:string}` | `HubuJson(PolicyExport)` | `GET /policies/export` |
| `hubu_policy_history` | `{policy_id?:string, agent_id?:string}` | `HubuJson(PolicyHistory)` | `GET /policies/history` |
| `hubu_policy_diff` | `{policy_id?:string, agent_id?:string, from_revision:integer, to_revision?:integer}`; `required(from_revision)` | `HubuJson(PolicyDiff)` | `GET /policies/diff` |
| `hubu_create_budget` | `{amount_cents?:integer, agent_id?:string, starting_at?:string, ending_before?:string}` | `HubuJson(CreatedBudget)` | `POST /budgets` |
| `hubu_create_recurring_budget` | `{amount_cents?:integer, agent_id?:string, recurrence?:daily\|monthly\|yearly, period_count?:integer, starting_at?:string}` | `HubuJson(CreatedBudgetSeries)` | `POST /budgets/series` |
| `hubu_revoke_budget` | `{budget_id?:string}` | `HubuJson(RevokedBudget)` | `POST /budgets/revoke` |
| `hubu_replace_budget` | `{budget_id?:string, amount_cents?:integer}` | `HubuJson(ReplacedBudget)` | `POST /budgets/replace` |
| `hubu_set_spending_target` | `{amount_cents?:integer, starting_at?:string, ending_before?:string}` | `HubuJson(SetSpendingTarget)` | `POST /user/spending-target` |
| `hubu_revoke_spending_target` | `{target_id?:string}` | `HubuJson(RevokedSpendingTarget)` | `POST /user/spending-target/revoke` |
| `hubu_show_spending_targets` | `{include_all?:boolean}` | `HubuJson(SpendingTargetList)` | `GET /user/spending-target[?all=true]` |
| `hubu_submit_spend` | `{account_id:string, amount_cents:integer, reason:string, merchant?:string, execution_scope?:ExecutionScopeV1, workload_profile?:string}`; `required(account_id,amount_cents,reason)` | `HubuJson(SpendDecision + approval hints)` | `POST /spend` |
| `hubu_authorize_spend` | same as `hubu_submit_spend` | `HubuJson(SpendDecision + approval hints)` | `POST /spend/authorize` |
| `hubu_list_agents` | `{}` | `HubuJson(AgentList)` | `GET /agents` |
| `hubu_list_budgets` | `{include_all?:boolean}` | `HubuJson(BudgetList)` | `GET /budgets[?all=true]` |
| `hubu_list_ledger` | `{}` | `HubuJson(Ledger)` | `GET /ledger` |
| `hubu_get_executor_claim` | `{claim_id:string}`; `required(claim_id)` | `HubuJson(ExecutorClaim)` | `GET /spend/executor/claim` |
| `hubu_list_claims_requiring_reconciliation` | `{}` | `HubuJson(ExecutorClaimList)` | `GET /spend/executor/reconciliation` |
| `hubu_reconcile_vendor_billed_claim` | `{claim_id:string, provider_reference:string, evidence:string, receipt:SettlementReceipt}`; all required | `HubuJson(ExecutorSettlement)` | `POST /spend/executor/settle` with separate reconciliation capability |
| `hubu_reconcile_vendor_did_not_bill_claim` | `{claim_id:string, provider_reference:string, evidence:string}`; all required | `HubuJson(ExecutorClaim)` | `POST /spend/executor/release` with separate reconciliation capability |

`ExecutionScopeV1` requires `schema_version:1`, `provider`, `executor`,
`capability`, and `billing_merchant`, all non-empty strings.
`SettlementReceipt` requires non-negative `actual_vendor_cost_cents`,
`provider_request_id`, `artifact_reference`, and `price_model_snapshot`.
The snapshot requires `provider`, `model`, non-negative `unit_price_cents`,
`pricing_unit`, and `currency:"usd"`. Both objects reject extra fields.

The named Hubu response schemas above have these top-level fields. Nested public
records retain the fields serialized by the linked source types; `?` means
nullable or conditionally present. This field-set table is normative for the v1
router and prevents an implementation from substituting a summary response.

| Schema | Required top-level fields |
| --- | --- |
| `RegistrationGuidance` | the complete `hubu-agent-registration-v1` guidance object defined in [agent-registration-protocol.md](agent-registration-protocol.md), including protocol, human/client inputs, canonicalization, fingerprinting, review, and submission guidance |
| `ClientApprovalProfile` | `protocol_version`, `summary`, `client_policy`, `response_contract`, `annotation_fields`, `tools` |
| `UserList` | `users[]`; each user has `user_id`, `username?`, `display_name`, `email?`, `status`, `current`, `created_at` |
| `InitUser` | `user_id`, `username?`, `display_name` |
| `RegisteredAgent` | `user_id`, `agent_id`, `agent_pub_id`, `version_id`, `account_id`, `session_id` |
| `AppliedPolicy` | `scope`, `agent_id?`, `policy_id`, `declarative_key`, `display_name`, `revision`, `payload_hash`, `policy_version`, `default_decision`, `changed`, `assignment_changed` |
| `PolicyView` | `policy_id`, `declarative_key`, `display_name`, `revision`, `payload_hash`, `created_at`, `updated_at`, `policy`, `assignments[]` |
| `PolicyExport` | every `PolicyView` field plus `policy_yaml` |
| `PolicyHistory` | `policy_id`, `revisions[]`, `audit[]` |
| `PolicyDiff` | `policy_id`, `from_revision`, `to_revision`, `from_hash`, `to_hash`, `changed_paths[]`, `from`, `to` |
| `CreatedBudget` | `budget`, `spending_target_warnings[]` |
| `CreatedBudgetSeries` | `budgets[]`, `spending_target_warnings[]` |
| `RevokedBudget` | `budget` |
| `ReplacedBudget` | `revoked_budget`, `budget`, `spending_target_warnings[]` |
| `BudgetList` | `budgets[]` |
| `SetSpendingTarget` | `target` |
| `RevokedSpendingTarget` | `target` |
| `SpendingTargetList` | `targets[]` |
| `AgentList` | `agents[]`; each agent has `agent_id`, `display_name`, `description?`, owner identity, agent/account status, `account_id`, and `created_at` |
| `Ledger` | `transactions[]`; each transaction has owner identity, `external_ref?`, description, timestamp, and balanced `entries[]` containing account, direction, amount, and currency |
| `SpendDecision` | `operation_key`, `task_id?`, `reason`, `account_id`, `agent_id`, `decision_id`, `decision`, `reasons[]`, `scope_inputs`, `policy_decision`, `auth_token_id?`, `execution_scope?`, `workload_profile`, `authorization_expires_at?`, `budget_hold?`, `payment?`, `revision`, `idempotent_replay`, `retry_guidance`, `attempt_history[]`; MCP additionally supplies `requires_human_approval` and conditionally `approval_reason` |
| `ExecutorClaim` | `operation_key`, `claim_id`, `workload_profile`, `status`, claim/finalization timestamps, `settlement_id?`, reconciliation fields, and nested `spend` authorization snapshot |
| `ExecutorClaimList` | `claims[]` of `ExecutorClaim` |
| `ExecutorSettlement` | `operation_key`, `settlement_id`, `claim_id`, `status`, `receipt`, `spend`; receipt contains authorized, actual, and released amounts, currency, provider request, price snapshot, artifact reference, and timestamp |

`budget` records contain `budget_id`, `agent_id`, limit/currency/window, status,
and consumed/frozen/remaining amounts. `target` records contain target and
allocated amounts, exceeded amount/flag, currency/window, and status. These are
integer minor-unit amounts and are not converted by the router.

For the two spend tools, `operation_key` and optional `task_id` remain trusted
platform metadata under `_meta["hubu.dev/platform-invocation"]`; they are never
added to model-visible arguments. Protected Hubu tools keep the current trusted
human-approval gate and reconciliation keeps its distinct capability.

### Gongbu-owned execution and artifact tools

Each route is made with only the Gongbu backend credential and authenticated
account claim. `GongbuText(T,vN)` means the existing Gongbu text result carrying
the named HTTP response schema and schema version. Source structures remain
authoritative in [`crates/gongbu-mcp/src/lib.rs`](../crates/gongbu-mcp/src/lib.rs)
and [`crates/gongbu-api/src/http/mod.rs`](../crates/gongbu-api/src/http/mod.rs).

| Unified and standalone name | Exact input schema (compact) | Response schema | Gongbu route |
| --- | --- | --- | --- |
| `gongbu_create_execution` | `{schema_version:2, spend_auth_token_id:string(1..255), input:object, input_schema_version:integer>=1, workload_type:string, provider:string, adapter:string, model:string}`; all required, strings non-empty | `GongbuText(ExecutionResponse,v2)` | `POST /v2/executions` |
| `gongbu_get_execution` | `{execution_id:SafeId}`; required | `GongbuText(ExecutionResponse,v1)` | `GET /v1/executions/{execution_id}` |
| `gongbu_list_artifacts` | `{execution_id:SafeId}`; required | `GongbuText(ArtifactListResponse,v1)` | `GET /v1/executions/{execution_id}/artifacts` |
| `gongbu_get_artifact` | `{artifact_id:SafeId}`; required | metadata text `ArtifactContentMetadataV1` plus `image/png` or `image/jpeg` block | `GET /v1/artifacts/{artifact_id}` |

`SafeId` is a 1–255 character string matching `^[A-Za-z0-9_-]+$`.
`ExecutionResponse` contains `schema_version`, `execution_id`, `operation_key`,
`status`, nullable `outcome` and `failure`, `authorization {amount_minor,
currency}`, timestamps, and nullable start/completion timestamps.
`ArtifactListResponse` contains `schema_version`, `execution_id`, and safe
artifact metadata only. `ArtifactContentMetadataV1` contains `schema_version:1`,
`artifact_id`, media type, byte size, SHA-256, and `encoding:"base64"`; it never
contains a filesystem path or storage key.

## Capability discovery and version negotiation

Discovery is a snapshot of routing safety, not a promise that a later network
call cannot fail. Before replying to `initialize`, and thereafter on a bounded
refresh interval, the router probes independently:

1. Hubu `GET /health` and `GET /version`.
2. Gongbu `GET /livez`, `GET /readyz`, and `GET /version`.

The initialize result includes `UnifiedCapabilitiesV1` under
`capabilities.experimental["hubu.dev/unified-mcp"]`, with exactly the same
snake-case field names and value schemas shown above except that `generated_at`
is omitted. Its `tools` array still contains all 33 names in lexical order with
each owner, availability flag, and reason code; it is not reduced to a list of
available names. Given a capability-tool response `C`, the initialize extension
must therefore equal `C` after deleting only `generated_at`. Clients compare
`contract_version` exactly and must not infer compatibility from product SemVer.
Clients that cannot consume experimental initialize fields call
`hubu_unified_capabilities`.

The v1 compatibility matrix is fixed as follows:

| Check | Required value |
| --- | --- |
| unified contract | exact `hubu-gongbu-mcp-v1` |
| routing revision | integer `1` |
| MCP protocol | exact `2024-11-05` |
| Hubu executor contract | exact `hubu-spend-executor-v4.2` |
| Gongbu executor contract | exact `hubu-spend-executor-v4.2` |
| Gongbu API schema | exact integer `2` |
| Gongbu MCP schema | exact integer `2` |
| backend product versions | each must exactly equal the unified router's `product_version`, and therefore each other |
| source commits | router, Hubu, and Gongbu values must all be known, non-empty 40-character lowercase Git SHAs and must exactly equal |

An implementation changes a required value only by publishing a new unified
contract version or routing revision with contract tests. It must never use
"latest", accept `unknown` build provenance, silently accept an unknown schema,
or guess from field presence. An unstamped local build is therefore
`incompatible`, not a development exception; local unified testing must stamp
all three binaries from the same workspace commit.

Backend states and catalog behavior are deterministic:

- `unconfigured`: endpoint or credential is missing; all tools for that backend
  are absent from `tools/list`.
- `unavailable`: liveness/health or version transport failed; all its tools are
  absent.
- `incompatible`: any matrix check failed; all its tools are absent and no call
  is forwarded.
- `degraded`: version-compatible and live, but not ready. For Gongbu,
  `gongbu_create_execution` is absent while its three read/artifact tools remain.
  Hubu v1 has no separate readiness signal, so it does not use degraded state.
- `available`: compatible and healthy; all owned tools are listed.

`hubu_unified_capabilities` is always listed. `tools/list` contains it plus only
the backend tools whose snapshot says `available:true`. A state transition emits
`notifications/tools/list_changed`. Calls re-check the selected backend state;
a stale catalog never authorizes routing.

## Partial availability and fail-closed errors

The router process and capability tool remain available when either backend is
down. The healthy backend continues serving its own tools; there is no
cross-backend fallback. Specifically, a Gongbu outage cannot block Hubu policy,
budget, or reconciliation tools, and a Hubu outage cannot redirect governance
calls to Gongbu. Gongbu itself may become not ready because its Hubu executor
dependency is unhealthy; the router reports Gongbu degraded without pretending
that execution admission is safe.

Calls rejected before forwarding use JSON-RPC `-32010` and data:

```json
{
  "code": "backend_unconfigured | backend_unavailable | backend_incompatible | backend_not_ready",
  "tool": "canonical tool name",
  "owner": "hubu | gongbu",
  "retryable": false,
  "capabilities_changed": true
}
```

`retryable` is true only for `backend_unavailable` and `backend_not_ready`.
Messages and reason codes must be sanitized. Calls are never queued, replayed,
or sent to the other backend by the router. Once forwarded, the owning adapter's
existing error contract is preserved. A transport failure after an ambiguous
mutation remains ambiguous; the router must not automatically retry it.

## Migration, collisions, and omissions

All 32 standalone tool mappings are identity mappings: the unified name, input
schema, annotations, result schema, and owner are the same as today. Clients
replace their `hubu-mcp-server` and `gongbu-mcp` entries with one
`hubu-unified-mcp` entry, then require `hubu-gongbu-mcp-v1` during
initialization. Saved allowlists can retain every existing `hubu_*` and
`gongbu_*` entry. Clients should additionally allow the read-only
`hubu_unified_capabilities` tool.

There are no name collisions. The superficially overlapping operations are
intentional and not aliases:

- `hubu_authorize_spend` creates governance authority; it does not execute
  provider work. `gongbu_create_execution` consumes that authority and owns the
  execution.
- `hubu_get_executor_claim` reads Hubu's budget-hold/claim record;
  `gongbu_get_execution` reads Gongbu's execution lifecycle.
- `hubu_health` retains its Hubu-only response for compatibility;
  `hubu_unified_capabilities` is the cross-backend status view.
- `hubu_add_policy` remains the existing compatibility alias and routes to Hubu;
  it is not silently rewritten to `hubu_apply_policy` by the unified router.

Intentional omissions are HTTP-only operator/debug routes, Gongbu reconciliation
routes, backend version/readiness endpoints as model tools, storage keys,
filesystem paths, credentials, account claims, provider retry controls, and MCP
protocol methods such as `ping` (which is a method, not a tool). They are not
supported standalone tools and exposing them would broaden authority or confuse
transport health with product operations.

## Parity, deprecation, and removal gates

Standalone MCP servers are not deprecated merely because this decision is
accepted. Gates are cumulative and require recorded evidence:

1. **Contract gate:** this document is merged and implementation issues cite
   `hubu-gongbu-mcp-v1` without changing names, ownership, or schemas.
2. **Static parity gate:** automated tests compare unified `tools/list` against
   all 28 Hubu and 4 Gongbu standalone definitions, including exact input
   schemas and Hubu annotations, and separately test the router-owned tool.
3. **Behavior parity gate:** each of the 32 mapped tools has a golden success
   result test and representative backend/application error test proving no
   response translation. Approval metadata and artifact image content receive
   dedicated tests.
4. **Compatibility/failure gate:** tests cover every backend state, all matrix
   mismatches, state changes between list and call, one-backend outages,
   Gongbu-not-ready behavior, credential isolation, and ambiguous mutation
   transport failure with no automatic retry.
5. **Canary gate:** at least two consecutive immutable canary releases and 14
   calendar days complete with zero unresolved P0/P1 unified-surface defects,
   successful migration of at least one Hubu and one Gongbu client workflow,
   and a verified rollback to both standalone server configurations.
6. **Deprecation gate:** only after gates 1–5 may release notes and docs mark
   direct standalone configuration deprecated. A stable unified release must be
   available first. Deprecation does not remove either backend process or
   boundary.
7. **Removal gate:** standalone client configuration remains supported for at
   least 90 days and two stable releases after deprecation. Removal additionally
   requires zero unresolved P0/P1 migration defects, updated examples and
   installers, documented operator sign-off, and a retained rollback path.

Failure of any gate pauses deprecation or removal. Lower-priority findings may
be accepted only with an owner, rationale, and target release; security,
credential isolation, mutation replay, schema parity, and P0/P1 findings cannot
be waived by schedule.

## Implementation boundaries for follow-up issues

- The unified server may depend on a small router-owned contract module and
  backend HTTP client modules. It must not import Hubu/Gongbu persistence,
  application, provider, or credential implementation crates.
- Tool definitions should be generated or parity-tested from standalone public
  definitions to prevent drift, while build-time sharing must not introduce a
  direct Hubu–Gongbu crate dependency.
- Backend probes are bounded, independent, sanitized, and do not share circuit
  breakers. A slow backend cannot prevent the other backend or the local
  capability tool from responding.
- Routing is a static name-to-owner table. Prefix inference is insufficient;
  unknown future names fail closed until a routing revision explicitly assigns
  them.
- Any future composed workflow is a new public tool and design decision. It
  cannot be smuggled into one of the 32 identity mappings.

These boundaries let implementation, health reporting, per-backend routing, and
end-to-end migration tasks proceed without reopening public naming or ownership.
