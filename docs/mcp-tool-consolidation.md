# MCP consolidation and default authority contract

Status: **HUB-203 design contract for v0.2.2; not implemented by this change.**
Runtime implementation belongs to HUB-204, HUB-205 and HUB-207; qualification
belongs to HUB-206/HUB-208. The [implemented surface](unified-mcp.md) remains
authoritative until those changes land.

[Linear HUB-203](https://linear.app/hubu/issue/HUB-203) ·
[Product decision](https://app.notion.com/p/3e26b9239bee8105838add98b50bd19e)

## Decision

Hubu-only identity and spending authority are a default workflow.
`hubu_authorize_spend` stays first-class alongside owner registration, policy,
budget and approval tools. Gongbu execution is optional; an external executor
can consume the same authority without adopting Gongbu.

Reduce discovery through three explicit categories: standard tools, advanced
tools, and deprecated compatibility names. Advanced adds specialist controls
to standard; it is not an owner privilege or a separate server. Keep claim
inspection and reconciliation **standard in v0.2.2**: external settlement is
not fully projected into router operation status, and hiding recovery now
would impair the standalone flow. Human gates and credential scopes still apply.

## Baseline and caller evidence

Inventory checked on 2026-09-21:

| Baseline | Routing revision | Unique names |
| --- | --- | --- |
| Shipped v0.2.1, source `13b14e191fdc6eb7853d40e4b1c02bc04f88d53b` | 8 | 44 |
| Main `f027866f2dd968ef1c790d72a6314fbd760b0b2b` | 9 | 43 |

The difference is `hubu_create_recurring_budget`, already removed by HUB-189.
It is not a new compatibility alias and must not be restored. Duplicate
connector namespaces in a client are not additional server tools.

Sources and known consumers:

- [Routing inventory](../fixtures/unified-mcp-routing-v1.json),
  [Hubu catalog](../crates/hubu-unified-mcp/src/hubu/catalog.rs),
  [Gongbu catalog](../crates/hubu-unified-mcp/src/gongbu/catalog.rs).
- [Golden parity](../crates/hubu-unified-mcp/tests/golden_parity.rs) explicitly
  calls all four consolidation candidates. [Hubu routing tests](../crates/hubu-unified-mcp/src/hubu/tests.rs)
  assert forwarding and response behavior, including policy inspection.
- [Generated Codex configuration](../crates/hubu-cli/src/codex_mcp.rs) includes
  an auto-approval entry for `hubu_submit_spend`; it must follow exposure.
- [Durable operation registry](../crates/hubu-unified-mcp/src/operation_registry.rs)
  and [approval resume](../crates/hubu-unified-mcp/src/resume_operation.rs)
  persist/dispatch the exact `hubu_submit_spend` origin. Renaming or translating
  it would risk old operation recovery.
- [E2E tests](../crates/hubu-unified-mcp/tests/unified_mcp_e2e.rs) exercise
  synchronous spend/approval behavior. [Current public documentation](unified-mcp.md)
  lists the old names, so absence of production telemetry is not proof of no users.

No external usage telemetry was available. Preserve compatibility through the
whole 0.2.x line rather than delete handlers based on repository usage alone.
These inventory counts are baseline evidence, not permanent test constants.

## Complete v0.2.2 disposition

H = compatible, available Hubu; G = compatible Gongbu (degraded permits reads);
R = available durable router operation registry; Local = no backend needed.
Creation through Gongbu requires ready G and available H. Existing runtime
credential, schema, ownership and approval checks apply in addition to these
discovery prerequisites. A listed protected tool is not a promise that the
client has configured its human gate.

Standard tools are listed and callable in both modes. Advanced tools are listed
and admit new work only in advanced mode; persisted mock-operation recovery has
the narrow exception below. Compatibility tools are omitted from both
lists but remain callable in both modes for the transition period, with their
existing backend prerequisites and gates. Unknown/retired names stay rejected.

| Exact tool name | Prerequisites | Disposition | Purpose / replacement |
| --- | --- | --- | --- |
| `gongbu_create_execution` | G+H+R | Advanced | Continue authorization; no replacement for standalone authorize |
| `gongbu_get_artifact` | G | Standard | Retrieve original output |
| `gongbu_get_execution` | G | Advanced | Execution-ID diagnostics; normal path uses operation status |
| `gongbu_get_provider_catalog` | G | Advanced | Contract/readiness diagnostics |
| `gongbu_get_redaction_attestation` | G | Advanced | Specialized FLUX evidence |
| `gongbu_list_artifacts` | G | Standard | Recover output IDs |
| `gongbu_list_execution_targets` | G | Standard | Approved targets, scope and pricing |
| `hubu_add_policy` | H | Compatibility | Use hubu_apply_policy with explicit YAML |
| `hubu_apply_policy` | H | Standard | Canonical policy mutation; human gate |
| `hubu_authorize_spend` | H+R | Standard | Reserve authority for external or managed execution |
| `hubu_budget_history` | H | Standard | Inspect immutable revisions |
| `hubu_client_approval_profile` | H | Advanced | Harness configuration; preserve current backend gate |
| `hubu_create_budget` | H | Standard | Owner administration; human gate |
| `hubu_export_policy` | H | Compatibility | Use hubu_show_policy with include_yaml=true |
| `hubu_feedback_guidance` | Local | Standard | Offline support discovery |
| `hubu_get_executor_claim` | H | Standard | External executor settlement/recovery inspection |
| `hubu_get_spend_approval` | H | Standard | Immutable approval review |
| `hubu_health` | H | Compatibility | Use hubu_unified_capabilities; legacy response unchanged |
| `hubu_list_agents` | H | Standard | Identity discovery |
| `hubu_list_budgets` | H | Standard | Budget visibility |
| `hubu_list_claims_requiring_reconciliation` | H | Standard | Find frozen claims without retained IDs |
| `hubu_list_ledger` | H | Standard | Recorded spend visibility |
| `hubu_list_users` | H | Standard | Human identity selection |
| `hubu_operation_status` | R | Standard | Public-handle observation, not external settlement tracking |
| `hubu_policy_diff` | H | Standard | Compare immutable revisions |
| `hubu_policy_history` | H | Standard | Policy audit |
| `hubu_prepare_feedback` | Local | Standard | Offline reviewed preview; never submits |
| `hubu_reconcile_vendor_billed_claim` | H | Standard | Human-gated settlement with evidence |
| `hubu_reconcile_vendor_did_not_bill_claim` | H | Standard | Human-gated release with evidence |
| `hubu_register_agent` | H | Standard | Structured identity registration; human gate |
| `hubu_register_human` | H | Standard | Onboard/select owner; human gate |
| `hubu_registration_guidance` | H | Standard | Machine-readable registration protocol |
| `hubu_resolve_spend_approval` | H | Standard | Explicit human approve/deny gate |
| `hubu_resume_operation` | H+R (*) | Standard | Resume existing immutable intent, including legacy mock work |
| `hubu_revoke_budget` | H | Standard | Emergency owner control; human gate |
| `hubu_revoke_spending_target` | H | Standard | Advisory target administration; human gate |
| `hubu_set_spending_target` | H | Standard | Advisory target administration; human gate |
| `hubu_show_policy` | H | Standard | Canonical inspection; optional YAML |
| `hubu_show_spending_targets` | H | Standard | Advisory targets and allocations |
| `hubu_submit_governed_execution` | H+G+R | Standard | Optional managed execution; internally composes primitives |
| `hubu_submit_spend` | H+R | Advanced/demo | Retain exact name and mock-payment semantics |
| `hubu_unified_capabilities` | Local | Standard | Health, compatibility and effective exposure |
| `hubu_update_budget` | H | Standard | Version-pinned cap change; human gate |

(*) Resume discovery requires H+R; resuming a stored governed-execution intent
also requires ready G. Existing `hubu_submit_spend` operations remain readable
and recoverable in standard mode; the advanced restriction governs new admission.

With all prerequisites satisfied: 34 standard names, six advanced additions
(including the demo payment tool), and three callable-only compatibility names.
The removed recurring-budget name is historical and outside these 43 names.
Backend outages can reduce the actual lists.

## Canonical schemas and compatibility rules

### Policy application

Keep `hubu_apply_policy` and its current schema: required `policy_yaml`;
optional `declarative_key`, `display_name`, `agent_id`,
`expected_revision`, `expected_hash`; reject additional properties.
Do not add `daily_limit_cents` to the canonical interface.

Keep `hubu_add_policy` callable with its old schema and response through 0.2.x,
including backend default/source behavior; do not silently substitute a
different audit actor/source or CAS rule. New callers prepare YAML and use
`hubu_apply_policy`. For old shortcut callers, migration exports/inspects the
existing generated policy rather than guessing a daily allowance: the starter
policy denies the blocked merchant, allows a **single spend** within the
threshold, and defaults to needs_approval. Preserve those rules and identities;
a cumulative daily budget is not an equivalent translation.

### Policy inspection/export

Extend `hubu_show_policy` with optional boolean `include_yaml`, default
`false`. Keep optional `policy_id` / `agent_id` selectors mutually exclusive;
keep existing default selection and reject unknown fields or non-boolean flags.

- False/omitted: existing show response, unchanged.
- True: same metadata, policy and assignments, plus string `policy_yaml`;
  use the existing backend export route/serializer so YAML round-trips
  semantically to the same policy.
- `hubu_export_policy` remains callable with its old selector-only schema and
  old response through 0.2.x. Its replacement is show with `include_yaml=true`.

The flag is router-owned: remove it before forwarding selectors. No new policy
storage model or backend endpoint is necessary.

### Health

New callers use `hubu_unified_capabilities` to inspect both backends and
compatibility. Legacy `hubu_health({})` continues calling Hubu `GET /health`
with its existing response/errors; it is not a response-compatible alias for
the capabilities object. Keep HTTP health and readiness probes unchanged.

### Mock payment

Retain `hubu_submit_spend` under **advanced/demo**, not as a deprecated alias.
Its description must lead with: "Execute a synchronous mock payment in Hubu;
does not invoke a provider or Gongbu." Keep its existing request/response,
trusted operation identity, approval, ledger and hold semantics.

No new mock-tool name, automatic replacement, or removal date is introduced.
There is no equivalent migration to authorize/governed execution: users must
choose the intended workflow. New calls fail with an exposure error in standard
mode; opting into advanced permits new admissions. Already-persisted mock
operations remain readable and resumable in both modes. Exact redelivery after
an ambiguous original result is also permitted under the recovery-only rule
below, without requiring a connection restart or creating another hold.

### Timeline and notices

In v0.2.2, hide the three compatibility names from discovery and retain their
handlers. Add `annotations.x_hubu_deprecated=true` and
`annotations.x_hubu_replacement` to their stored definitions, and describe the
deprecation in capabilities/migration docs. Do not change legacy tool-result
envelopes merely to append warnings.

No handler deletion in any 0.2.x release. Earliest removal is 0.3.0, **not an
automatic deadline**: require an explicit release decision, migration notes,
fixture/client migration evidence and a review of known consumers. Existing
durable mock operations must remain recoverable even if that tool is retired
in a separate future decision.

## Exposure and client configuration

Name the setting **tool exposure**, avoiding confusion with stack profiles.
Use `HUBU_MCP_TOOL_EXPOSURE=standard|advanced`. Unset means standard; invalid
or empty values fail startup with a configuration error rather than widening
access. Both modes include authority; there is no required authority profile.

HUB-207 adds `--tool-exposure standard|advanced` to the existing
`hubu init codex` command. It writes this variable in the managed
`[mcp_servers.hubu.env]` block, alongside existing settings. These are target
commands, not executable instructions until HUB-207 ships:

```sh
hubu init codex --tool-exposure standard
hubu init codex --config /absolute/path/config.toml --tool-exposure advanced
```

For an existing managed block, an explicit exposure option with no connection
overrides updates exposure and generated per-tool approval entries only: do not
regenerate credentials, endpoints, state paths, trust flags or registration.
An omitted exposure option during full regeneration preserves the existing
valid managed value; a fresh configuration defaults to standard. Explicit
connection overrides retain the existing full-generation path. Document this
distinction in help, keep --dry-run nonmutating, and keep the unmanaged-table
--force requirement. No separate general-purpose configure command is needed.

Generate spend auto-approval entries only for exposure-enabled spend tools;
omit `hubu_submit_spend` in standard mode. Preserve all human-prompt rules;
choosing advanced must not set trust flags or grant credentials. The approval
profile's recommendations must agree with effective exposure (including
callable compatibility mutations), without changing backend permission checks.

Settings apply per configured connection, fixed at startup. Reconnect/restart
to change them; no model-callable mode switch. A shared client connection does
not provide per-chat isolation. Multiple globally enabled connections expose
their union; detect duplicate Hubu entries and explain cleanup, but never
silently remove user-managed or plugin configuration. The remote gateway or
stdio launcher must set exposure out of band; model arguments cannot select it.

## Discovery, calls, and capability consistency

Use one authoritative disposition map, independent of backend ownership.
Keep `hubu-gongbu-mcp-v1` and add
`tool_exposure_contract_version="hubu-mcp-tool-exposure-v1"` to capabilities.
Implementation increments the current routing revision when the surface lands;
do not change runtime revisions in this design-only PR.

1. `tools/list` returns exposure-listed names intersected with existing backend
   and registry availability rules.
2. `tools/call` first resolves the name and exposure. Except for verified
   recovery-only redelivery below, reject a known restricted name before any
   operation allocation or backend call: JSON-RPC `-32011`,
   message "Tool is not enabled for this connection", data
   `{code:"tool_exposure_restricted",tool:NAME,required_exposure:"advanced",retryable:false}`.
   Unknown/retired tools retain the current unknown-tool error.
3. Allowed names, including compatibility handlers, proceed through existing
   schema, operation identity, approval and backend guards. Do not replace
   their backend errors with exposure errors.
4. Grandfather already-persisted mock work across upgrade/exposure changes.
   `hubu_resume_operation` remains available in standard mode for the original
   immutable mock intent, after the existing approval and replay checks; it
   cannot accept replacement arguments or allocate a new logical operation.
   For an ambiguous original call without resumable intent, allow exact
   `hubu_submit_spend` redelivery in standard mode only when a read-only lookup
   finds its already-persisted normalized harness identity, exact tool name and
   canonical argument hash. Never allocate on lookup miss, rebind a terminal
   denial, infer identity from payload similarity, or permit changed scope.
   After a verified match, use the existing retry/replay path and original
   private operation key, preserving approval, hold and payment idempotency.
   Unknown identities and changed requests are rejected before mutation.
   Status remains read-only and must explain resume versus exact redelivery.
   Existing workers keep completing accepted work; no restart into advanced
   mode is needed to recover previously persisted work.
5. Standard governed execution can internally compose authorization and
   execution even though the public primitive `gongbu_create_execution` is
   advanced. The restriction is on public tool invocation, not internal
   backend calls under the validated immutable composite intent.

Capabilities retain every recognized name and owner. Add top-level
`tool_exposure`, and per-tool fields `exposure`
(`standard|advanced|compatibility`), `listed`, `callable`,
`deprecated`, `replacement` (name or null), and `backend_available`.
Here backend_available includes registry prerequisites but not client approval
configuration. Preserve the same meaning for callable: router exposure and
prerequisites allow dispatch, subject to existing credentials/approval/schema
checks for a new call. Set legacy `available=callable`. Add
`recovery_only_callable` (false except for restricted mock spend with usable
H+R) to describe conditional exact redelivery; it does not advertise general
admission. The server must still verify each recovery-only call's durable match.

Restricted names have callable/listed/available false and reason_code
`tool_exposure_restricted`; backend availability remains separately visible.
Compatibility names have listed=false, deprecated=true and callable based on
prerequisites. Other unavailable names preserve existing reason codes.

Compute `notifications/tools/list_changed` from the effective listed catalog,
including schemas/descriptions/annotations, not only backend health. Hidden-only
backend changes must not imply a changed tool list. Keep capabilities fresh
even when a change affects only a hidden name. Startup-fixed exposure changes
take effect on reconnection; no live config watcher is required.

## Standalone authority and recovery boundary

The [executor contract](spend-executor-contract.md) remains
`hubu-spend-executor-v4.3`; no new decision-only or non-reserving API is added.

An allow result can freeze budget and provide an opaque authorization
continuation. A trusted executor authenticates independently, resolves that
continuation using `POST /spend/executor/resolve`, durably stores the private
operation identity/scope, then claims via `POST /spend/executor/claim` before
irreversible work. Read-only validation alone is insufficient. The executor
settles precise actual cost or releases a non-billed claim through the existing
contract. Ambiguous billing requires reconciliation, not speculative release or
a replacement paid call. No executor credentials/private operation keys become
model-authored arguments.

Pending human approval uses existing inspect/resolve/status/resume tools.
Standalone resume replays the authorization intent; it must not start Gongbu.
Definitive denial is terminal; corrected work uses a new logical invocation.
Ambiguous outcomes retain exact-call recovery and no-replacement guidance.

**Known gap:** router status projects a primitive allow as `authorized`; it
does not automatically ingest external executor terminal settlement into a
Gongbu-style lifecycle. Do not tell users to poll that handle until external
work becomes succeeded. Use the executor's durable result and
`hubu_get_executor_claim` for the financial result; list reconciliation claims
when IDs are missing. This is why all four claim/reconciliation tools remain
standard. Their mutating calls retain explicit human confirmation, separate
reconciliation credentials and evidence requirements. CLI recovery also remains
available via `hubu spend claim` and `hubu spend reconcile`.

HUB-206 qualifies the external flow using HUB-36's conformance fixtures.
Current scope catalogs and credential provisioning must be checked with that
fixture; do not claim arbitrary executors are already drop-in compatible.
HUB-34 owns broader workflow/receipt discovery. Neither is permission to build a
new execution state machine inside the MCP router.

## Implementation ownership and release gates

| Owner | Required deliverable |
| --- | --- |
| HUB-203 | This matrix, exact schema/config/error decisions and compatibility policy |
| HUB-204 | Canonical show/YAML behavior, compatibility handlers, mock description and focused tests |
| HUB-205 | Disposition map, list/call/resume guards, capabilities and effective-list notifications |
| HUB-206 | Hubu-only external executor example and gap qualification, reusing HUB-36 |
| HUB-207 | Minimal init/config persistence and exposure-aware client approval entries |
| HUB-208 | Combined migration/E2E evidence and final user documentation/visualizer |

Each implementation PR updates its own tests and fixtures. Final qualification
must cover both exposure modes with Hubu-only, both backends, degraded/missing
backends and unavailable registry; all compatibility names; unknown retired
recurring-budget calls; blocked new mock admissions; unchanged human gates;
and advanced-to-standard restart while accepted work exists. Specifically test
pre-upgrade/advanced mock approval followed by approval and resume in standard,
ambiguous original-result redelivery in standard, wrong identity/changed scope
rejection, and terminal denial replay. Prove no duplicate payment/hold and no
new operation allocation through either recovery exception.

Validate policy YAML semantic equality, CAS/assignment preservation, health
response compatibility, and no duplicate holds/consumption on recovery. Run
Cargo tests from the unified workspace root with package selectors and protoc
installed when Gongbu/Temporal builds are involved.

This PR intentionally leaves the architecture visualizer showing implemented
behavior. HUB-205/HUB-208 update it when public routing/discovery actually
changes. Documentation edits require the link checker and retired-repository
reference audit. No new provider work, cross-backend Rust dependencies, full
onboarding redesign, v0.3 enforcement, or deferred budget lifecycle expansion
is part of this contract.
