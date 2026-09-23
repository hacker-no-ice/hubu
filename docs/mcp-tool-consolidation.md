# Core MCP tool set

Hubu exposes one supported tool set through `hubu-unified-mcp`. Identity,
spending authority, policy and budget management, approvals and recovery are
available without adopting Gongbu. Managed execution is optional.

The v0.2.2 catalog contains **39 tools**, with routing revision **12**. Backend
availability and the existing human-approval gates determine which tools are
usable. There are no standard/advanced exposure profiles or tool-exposure
configuration flags. See the [unified MCP reference](unified-mcp.md) for schemas,
configuration and transport behavior.

## Supported tools

**Hubu** and **Gongbu** mean a compatible, available backend. Gongbu permits
reads while degraded; creating execution requires a ready Gongbu and available
Hubu. **Store** means the persistent MCP operation store is available. It records
operation identity and recovery state across restarts for retries, status and
approval resume. Hubu owns financial state; Gongbu owns managed execution.

Prerequisites below describe availability, not permission. Credential scopes,
request validation and human-approval gates still apply. A listed protected
tool does not imply that a client has configured its human prompt.

| Tool | Prerequisites | Purpose |
| --- | --- | --- |
| `gongbu_create_execution` | Gongbu + Hubu + store | Continue an existing authorization into managed execution |
| `gongbu_get_artifact` | Gongbu | Retrieve original output |
| `gongbu_get_execution` | Gongbu | Read execution status by ID, including when the MCP store is unavailable |
| `gongbu_list_artifacts` | Gongbu | Recover output IDs |
| `gongbu_list_execution_targets` | Gongbu | Approved targets, scope and pricing |
| `hubu_apply_policy` | Hubu | Canonical policy mutation; human gate |
| `hubu_authorize_spend` | Hubu + store | Reserve authority for external or managed execution |
| `hubu_budget_history` | Hubu | Inspect immutable revisions |
| `hubu_create_budget` | Hubu | Owner administration; human gate |
| `hubu_feedback_guidance` | None | Offline support discovery |
| `hubu_get_executor_claim` | Hubu | External executor settlement/recovery inspection |
| `hubu_get_spend_workflow` | Hubu | Owner-scoped public workflow inspection |
| `hubu_get_spend_approval` | Hubu | Immutable approval review |
| `hubu_list_agents` | Hubu | Identity discovery |
| `hubu_list_budgets` | Hubu | Budget visibility |
| `hubu_list_claims_requiring_reconciliation` | Hubu | Find frozen claims without retained IDs |
| `hubu_list_ledger` | Hubu | Canonical recorded spend, agent-budget filters and coverage |
| `hubu_list_spend_workflows` | Hubu | Paginated owner-scoped authorization discovery |
| `hubu_list_users` | Hubu | Human identity selection |
| `hubu_operation_status` | store | Observe a public operation handle in the MCP store |
| `hubu_policy_diff` | Hubu | Compare immutable revisions |
| `hubu_policy_history` | Hubu | Policy audit |
| `hubu_prepare_feedback` | None | Offline reviewed preview; never submits |
| `hubu_reconcile_vendor_billed_claim` | Hubu | Human-gated settlement with evidence |
| `hubu_reconcile_vendor_did_not_bill_claim` | Hubu | Human-gated release with evidence |
| `hubu_register_agent` | Hubu | Structured identity registration; human gate |
| `hubu_register_human` | Hubu | Onboard/select owner; human gate |
| `hubu_registration_guidance` | Hubu | Machine-readable registration protocol |
| `hubu_resolve_spend_approval` | Hubu | Explicit human approve/deny gate |
| `hubu_resume_operation` | Hubu + store | Resume the stored, approved intent |
| `hubu_revoke_budget` | Hubu | Emergency owner control; human gate |
| `hubu_revoke_spending_target` | Hubu | Advisory target administration; human gate |
| `hubu_set_spending_target` | Hubu | Advisory target administration; human gate |
| `hubu_show_policy` | Hubu | Canonical inspection; optional YAML |
| `hubu_show_spending_targets` | Hubu | Advisory targets and allocations |
| `hubu_submit_governed_execution` | Hubu + Gongbu + store | Optional managed execution; internally composes primitives |
| `hubu_submit_spend` | Hubu + store | Execute a synchronous mock payment; no Gongbu or provider work |
| `hubu_unified_capabilities` | None | Inspect backend compatibility, availability and supported tools |
| `hubu_update_budget` | Hubu | Version-pinned cap change; human gate |

Resuming a stored governed-execution intent also requires ready Gongbu.
Backend outages can reduce discovery. The current inventory is maintained in
the [routing fixture](../fixtures/unified-mcp-routing-v1.json),
[Hubu catalog](../crates/hubu-unified-mcp/src/hubu/catalog.rs) and
[Gongbu catalog](../crates/hubu-unified-mcp/src/gongbu/catalog.rs).

## Removed tools and replacements

These names are removed from discovery, capabilities and public dispatch in
v0.2.2. Calls fail as unknown tools before backend access. There are no hidden
aliases, compatibility handlers or grace period. Reconnect clients to refresh
cached discovery and update callers to the supported interfaces.

| Removed tool | Supported interface |
| --- | --- |
| `hubu_add_policy` | `hubu_apply_policy` with explicit policy YAML |
| `hubu_export_policy` | `hubu_show_policy` with `include_yaml: true` |
| `hubu_health` | `hubu_unified_capabilities`; adopt its response shape |
| `gongbu_get_provider_catalog` | `gongbu_list_execution_targets` for agent-selectable targets/pricing; operator catalog via `hubu stack catalog`, `hubu stack doctor` or authenticated `GET /v1/provider-catalog` |
| `gongbu_get_redaction_attestation` | Operator qualification via authenticated `GET /v1/executions/{id}/redaction-attestation` |
| `hubu_client_approval_profile` | Tool approval annotations, generated client configuration and the [approval boundary](unified-mcp.md#approval-boundary) |

The operator endpoints retain their authentication, validation and safety
checks. Removing MCP wrappers does not remove stored policy records, backend
HTTP APIs or CLI functionality. Recurring-budget creation was already removed
and is not restored by this consolidation.

### Policy migration

`hubu_apply_policy` requires explicit `policy_yaml`; human approval and optional
revision/hash checks remain. Inspect existing policies when replacing the old
`daily_limit_cents` shortcut: it represented a **single-spend** threshold with a
blocked-merchant denial and a needs-approval fallback. A cumulative daily budget
is not an equivalent translation.

`hubu_show_policy` accepts optional boolean `include_yaml`, default `false`.
False or omitted preserves the existing response. True returns the same policy,
metadata and assignments plus `policy_yaml` from the canonical backend
serializer. Optional `policy_id` and `agent_id` selectors are mutually exclusive;
omitting both retains default selection. The YAML flag is local to the MCP
adapter and is not forwarded as a backend query parameter.

### Retained specialist workflows

`gongbu_create_execution` consumes an already-issued authorization continuation.
`hubu_submit_governed_execution` starts a composite authorization/execution
request; it does not replace continuation of an existing authorization.

`gongbu_get_execution` reads a known execution ID even when the persistent MCP
operation store is unavailable. `hubu_operation_status` requires that store
and a public operation handle. Both are useful recovery paths.

`hubu_submit_spend` executes a synchronous **mock payment**, with Hubu policy,
approval and budget accounting. It does not invoke Gongbu or a provider. It
remains supported for demos and exact-call recovery; it is not an alias for
real execution or standalone authorization. Ambiguous original calls can
require redelivery with the same trusted harness identity and unchanged
arguments. Approval resume alone does not replace that path.

## Authority, approval and recovery

`hubu_authorize_spend` can reserve budget and return an opaque continuation.
External executors use the authenticated [executor contract](spend-executor-contract.md)
to resolve, claim and settle or release that operation. Read-only validation
alone is insufficient before irreversible provider work. Executor credentials
and private operation keys are not model-authored tool arguments.

Human approval uses the existing inspect, resolve, status and resume tools.
Resolution records a decision; resume continues the same stored immutable
intent. Resuming standalone authorization must not start Gongbu. Exact replay
must not create a second logical operation or duplicate payment/hold. A
terminal denial requires a new logical invocation for corrected work.

Router operation status and backend financial history have different scopes.
A standalone allow can remain `authorized` in the MCP store after external
execution. Use the executor's durable outcome, claim inspection and
[workflow/ledger history](ledger-history.md) to inspect settlement; do not poll
the router handle expecting it to become a managed-execution result.
Ambiguous billing requires evidence-based reconciliation. Claim discovery and
both reconciliation tools remain available, with their existing human gates
and separate reconciliation capability.

Tool removal does not change the separate Hubu/Gongbu processes, credentials,
databases or failure domains. Capabilities and discovery retain existing
backend-availability behavior and list-change notifications.
