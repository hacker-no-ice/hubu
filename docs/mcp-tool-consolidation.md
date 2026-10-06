# Core MCP tool set

Hubu exposes one supported tool set through `hubu-unified-mcp`. Identity,
spending authority, policy and budget management, approvals and recovery are
available without adopting Gongbu. Managed execution is optional.

This page is the canonical catalog of supported tools. Backend availability and
the existing human-approval gates determine which tools are usable. There are
no standard/advanced exposure profiles or tool-exposure configuration flags.
At runtime, `tools/list` returns the currently callable tools, and
`hubu_unified_capabilities` reports the router's `routing_revision` plus every
public tool's owner, availability and reason code. See the
[unified MCP reference](unified-mcp.md) for schemas, configuration and transport
behavior. Tools removed in earlier releases, and their replacements, are listed
in the [changelog](../CHANGELOG.md).

## Supported tools

**Hubu** and **Gongbu** mean a compatible, available backend. Gongbu permits
reads while degraded; creating execution requires a ready Gongbu and available
Hubu. **Store** means the persistent MCP operation store is available. It records
operation identity and recovery state across restarts for retries, status and
approval resume. Hubu owns financial state; Gongbu owns managed execution.

Prerequisites below describe availability, not permission. Credential scopes,
request validation and human-approval gates still apply. A listed protected
tool does not imply that a client has configured its human prompt.

### Discovery and support

| Tool | Prerequisites | Purpose |
| --- | --- | --- |
| `hubu_unified_capabilities` | None | Inspect backend compatibility, availability and supported tools |
| `hubu_feedback_guidance` | None | Offline support discovery |
| `hubu_prepare_feedback` | None | Offline reviewed preview; never submits |

### Identity and registration

| Tool | Prerequisites | Purpose |
| --- | --- | --- |
| `hubu_list_users` | Hubu | Human identity selection |
| `hubu_register_human` | Hubu | Onboard/select owner; human gate |
| `hubu_list_agents` | Hubu | Identity discovery |
| `hubu_registration_guidance` | Hubu | Machine-readable registration protocol |
| `hubu_register_agent` | Hubu | Structured identity registration; human gate |

### Policy

| Tool | Prerequisites | Purpose |
| --- | --- | --- |
| `hubu_show_policy` | Hubu | Canonical inspection; optional YAML |
| `hubu_apply_policy` | Hubu | Canonical policy mutation; human gate |
| `hubu_policy_history` | Hubu | Policy audit |
| `hubu_policy_diff` | Hubu | Compare immutable revisions |

`hubu_apply_policy` requires explicit `policy_yaml`; human approval and optional
revision/hash compare-and-set checks apply.

`hubu_show_policy` accepts optional boolean `include_yaml`, default `false`.
False or omitted returns the standard response. True returns the same policy,
metadata and assignments plus `policy_yaml` from the canonical backend
serializer. Optional `policy_id` and `agent_id` selectors are mutually exclusive;
omitting both uses the default selection. Non-boolean flags and unknown fields
are rejected before dispatch. The YAML flag is local to the MCP adapter and is
not forwarded as a backend query parameter.

### Budgets and spending targets

| Tool | Prerequisites | Purpose |
| --- | --- | --- |
| `hubu_list_budgets` | Hubu | Budget visibility |
| `hubu_create_budget` | Hubu | Owner administration; human gate |
| `hubu_update_budget` | Hubu | Version-pinned cap change; human gate |
| `hubu_budget_history` | Hubu | Inspect immutable revisions |
| `hubu_revoke_budget` | Hubu | Emergency owner control; human gate |
| `hubu_show_spending_targets` | Hubu | Advisory targets and allocations |
| `hubu_set_spending_target` | Hubu | Advisory target administration; human gate |
| `hubu_revoke_spending_target` | Hubu | Advisory target administration; human gate |

### Spend authority

| Tool | Prerequisites | Purpose |
| --- | --- | --- |
| `hubu_authorize_spend` | Hubu + store | Reserve authority for external or managed execution |
| `hubu_submit_spend` | Hubu + store | Execute a synchronous mock payment; no Gongbu or provider work |

### Managed execution

| Tool | Prerequisites | Purpose |
| --- | --- | --- |
| `gongbu_list_execution_targets` | Gongbu | Approved targets, scope and pricing |
| `hubu_submit_governed_execution` | Hubu + Gongbu + store | Optional managed execution; internally composes primitives |
| `gongbu_create_execution` | Gongbu + Hubu + store | Continue an existing authorization into managed execution |
| `gongbu_get_execution` | Gongbu | Read execution status by ID, including when the MCP store is unavailable |
| `gongbu_list_artifacts` | Gongbu | Recover output IDs |
| `gongbu_get_artifact` | Gongbu | Retrieve original output |

Provider-contract diagnostics and guarded-FLUX attestation are not MCP tools;
they remain available through authenticated Gongbu operator endpoints.

### Approval, status and resume

| Tool | Prerequisites | Purpose |
| --- | --- | --- |
| `hubu_get_spend_approval` | Hubu | Immutable approval review |
| `hubu_resolve_spend_approval` | Hubu | Explicit human approve/deny gate |
| `hubu_operation_status` | store | Observe a public operation handle in the MCP store |
| `hubu_resume_operation` | Hubu + store | Resume the stored, approved intent |

Resuming a stored governed-execution intent also requires ready Gongbu.

### Ledger, authorization records and reconciliation

| Tool | Prerequisites | Purpose |
| --- | --- | --- |
| `hubu_list_ledger` | Hubu | Canonical recorded spend, agent-budget filters and coverage |
| `hubu_list_authorization_records` | Hubu | Paginated owner-scoped authorization discovery |
| `hubu_get_authorization_record` | Hubu | Owner-scoped public authorization record inspection |
| `hubu_get_executor_claim` | Hubu | External executor settlement/recovery inspection |
| `hubu_list_claims_requiring_reconciliation` | Hubu | Find frozen claims without retained IDs |
| `hubu_reconcile_vendor_billed_claim` | Hubu | Human-gated settlement with evidence |
| `hubu_reconcile_vendor_did_not_bill_claim` | Hubu | Human-gated release with evidence |

Backend outages can reduce discovery. The current inventory is maintained in
the [routing fixture](../fixtures/unified-mcp-routing-v1.json),
[Hubu catalog](../crates/hubu-unified-mcp/src/hubu/catalog.rs) and
[Gongbu catalog](../crates/hubu-unified-mcp/src/gongbu/catalog.rs).

## Specialist workflows

`hubu_submit_governed_execution` is the preferred ordinary path when an agent
has both spend authorization intent and an execution request. The primitive
Hubu and Gongbu tools remain available for recovery, diagnostics, and backward compatibility.

`gongbu_create_execution` consumes an already-issued authorization continuation.
`hubu_submit_governed_execution` starts a composite authorization/execution
request; it does not replace continuation of an existing authorization.

`hubu_resume_operation` resumes an approved pending normalized
operation—primitive spend or composite—by its public handle without requiring
the original harness call identity.

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

A standalone authorization's status comes from Hubu. Any executor may consume
the continuation through Hubu's executor API without the router seeing it, so
`hubu_operation_status` reads the operation's Hubu authorization record instead of its own
store. That applies to an allowed `hubu_authorize_spend` operation that was
never handed to Gongbu:

| Hubu authorization record | `state` | `terminal` | `replacement_safe` |
| --- | --- | --- | --- |
| authorized, unclaimed | `authorized` | no | yes |
| claimed by an executor | `executing` | no | **no** |
| settled | `settled`, with exact cost and budget charge | yes | no |
| released | `released` | yes | no |
| expired, never claimed | `expired` | yes | yes, as a new operation |
| claim lease expired | `reconciliation_required` | no | **no** |

The result carries `authority: {source: "hubu_authorization_record", verified}`. An
executor's claim can outlive the authorization itself, so the router never
reports such an operation as terminal from its own token expiry. If Hubu cannot
be reached, or returns a status the router cannot interpret, the result is
`state: "unverified"`. It is non-terminal and never replacement-safe. The
router does not write financial state; Hubu remains the only writer. See
[Use Hubu with your own executor](external-executor.md). Gongbu-managed and
governed operations keep their managed-execution status.
Ambiguous billing requires evidence-based reconciliation. Claim discovery and
both reconciliation tools remain available, with their existing human gates
and separate reconciliation capability.

The single tool set does not change the separate Hubu/Gongbu processes,
credentials, databases or failure domains. Capabilities and discovery follow
backend availability and emit list-change notifications.
