# CLI administration reference

For bugs, ideas, billing or sensitive reports, use [Send feedback](feedback.md).
`hubu feedback` and the unified `hubu_feedback_guidance` /
`hubu_prepare_feedback` tools prepare local previews without sending reports.


Use the `hubu` CLI when you prefer to keep human administration in a terminal,
or to independently verify changes proposed and applied through MCP. With a
selected, running stack profile, these commands automatically use that
profile's authenticated client handoff. Run `hubu <command> --help` for the
exact syntax supported by your installed release.

## Register the owner and agents

Register the human owner once, then register each agent separately. The agent
command prints the public `agent_id` used by policy and budget assignment and
the `account_id` used by governed spend requests.

```sh
hubu protocol agent-registration
hubu register human \
  --username alice-example \
  --display-name "Alice Example"

hubu register agent --name image-researcher --version local-dev
hubu register agent --name image-designer --version local-dev

hubu agent list
```

Agent names are unique per owner, ignoring case and surrounding spaces.
`hubu register agent` with a new identity under an existing agent's name fails
and names that agent's `agt_...` ID; choose a different `--name` or rename the
existing agent. Repeating a registration with the same identity reuses the
existing records.

Registration guidance, derived fields, fingerprints, and record reuse are
documented in [Agent registration](agent-registration.md).

## Rename an agent

Fix an agent's owner-facing name without changing its `agt_...` ID. Budgets,
policy assignments, the account, and ledger history stay attached. Rename is
human-owner only: it sends the human approval capability
(`HUBU_APPROVAL_TOKEN` or `HUBU_APPROVAL_TOKEN_FILE`) and is not available
through the unified MCP.

```sh
hubu agent rename --agent-id agt_EXACT_AGENT_ID --name research-agent --reason "fix typo"
hubu agent history --agent-id agt_EXACT_AGENT_ID
```

`hubu agent history` shows each revision (old -> new values, actor, time,
reason) and the identity fingerprints that resolve to the agent. Clients still
registering under a previous name resolve to the same agent and receive a
warning naming the current name. A rename to a name another agent uses or
previously used is rejected. See
[Renaming an agent](agent-registration.md#renaming-an-agent).

## Draft, validate, and apply a policy

Create or edit a YAML policy, validate it locally, and review its assignment
scope before applying it. Omitting `--agent-id` assigns the policy as the
user default; supplying an exact `agt_...` ID creates an override for only that
agent.

```sh
hubu policy new-template --path policies/image-generation.yaml
hubu policy validate --path policies/image-generation.yaml

# Assign as the user default after reviewing the validated YAML.
hubu policy apply \
  --path policies/image-generation.yaml \
  --name "Image generation guardrails"

# Or assign an override to one exact agent.
hubu policy apply \
  --path policies/image-generation.yaml \
  --name "Image researcher override" \
  --agent-id agt_EXACT_AGENT_ID

hubu policy list
hubu policy show --agent-id agt_EXACT_AGENT_ID
```

Use the [Hubu Policy Authoring skill](../skills/hubu-policy-authoring/SKILL.md)
to translate operator intent into reviewable YAML. See the
[Policy engine](policy-engine.md) for evaluation and revision semantics.

## Create and inspect budgets

Budgets are cumulative limits for exact agents. `--amount` is an exact decimal
with up to 6 decimal places (`10`, `0.50`, `0.005`), and `--currency` is `usd`
(default) or `usdc`. Budgets are per currency: spend only counts against a
budget in its own currency. Hubu prints the resulting public budget ID and
active period.

```sh
hubu budget create --agent-id agt_FIRST_AGENT_ID --amount 10
hubu budget create --agent-id agt_SECOND_AGENT_ID --amount 1 --currency usdc
hubu budget list
```

Update, history, and revocation commands are also available:

```sh
hubu budget update \
  --budget-id bgt_EXACT_BUDGET_ID \
  --amount 50 \
  --reason "Raise the reviewed total cap"
hubu budget history --budget-id bgt_EXACT_BUDGET_ID
hubu budget revoke --budget-id bgt_EXACT_BUDGET_ID
```

See [Spend lifecycle](spend-lifecycle.md) for reservation, settlement, and
ledger behavior. Policies govern individual requests; budgets govern cumulative
spending over their active period.

## Task budgets

A [task budget](task-budgets.md) reserves part of an agent budget for one task.
Spend for that task must fit it, and closing it returns the unused part:

```bash
hubu budget task open --agent-id AGENT_ID --task-id landing-page --amount 3 --reason "landing page design"
hubu budget task list
hubu budget task show --agent-id AGENT_ID --task-id landing-page
hubu budget task close --task-budget-id tbg_0123456789ab
```

`list` shows open task budgets. Use `--all` to include closed ones. Each entry
shows spend so far, frozen, remaining and the agent budget it draws from. A
closed one also shows the amount returned. `hubu watch` shows open task budgets
under their agent.

These commands act directly. An agent instead proposes a task budget or asks
to close one. `list` shows those requests first, and you review and resolve
each `tba_` request with `hubu budget task approval`:

```bash
hubu budget task approval get --approval-request-id tba_0123456789ab
hubu budget task approval approve --approval-request-id tba_0123456789ab
```

Approve a close request only after checking that the task is done. Denying it
keeps the task budget open. Requests expire after 24 hours. A task budget
request never authorizes spend, and `hubu spend approval` refuses `tba_` ids:
spend approval covers spend only.

## Amounts

Every amount the CLI accepts (`--amount` on `budget create`, `budget update`,
`spend`, `spend authorize` and `user spending-target set`) is an exact decimal
with up to 6 decimal places. Signs, exponents and more digits are rejected
with a message that says how to fix the value.

Amounts print exactly, with 2 to 6 decimal places and never rounded: `$10.00`,
`$0.035`, `$0.000001`, and `-$0.0031` for the negative remainder a reconciled
overrun leaves. USDC amounts print as `0.005 USDC`. The CLI reads the exact
money objects in Hubu responses and falls back to the legacy `*_cents` fields
from older servers.

The global `--round cents` option shows whole cents instead and marks every
value it changed with `≈`. It rounds conservatively: consumed, frozen and
held amounts round up, while limits and remaining amounts round down.

```sh
hubu --round cents budget list
```

To record a vendor-billed reconciliation, pass the exact provider cost as a
decimal: `hubu spend reconcile billed … --actual-vendor-cost 0.0031`. The
`--actual-vendor-cost-cents` and `--unit-price-cents` flags remain as
deprecated aliases.

## Ledger and authorization record history

`hubu ledger list` prints canonical JSON for wallet payments, provider expenses
and corrections. Select an agent budget with `--agent-id` and `--budget-id`, and
page with `--limit` and `--cursor`. `hubu spend authorizations` lists authorization records;
`hubu spend show --authorization-id ID` inspects one public authorization record. See
[ledger and authorization records](ledger-history.md) for examples and cost semantics.

## Watch budgets during the demo

```sh
hubu watch
hubu watch --compact
hubu watch --events 10
hubu watch --agent image-agent
hubu watch --once --currency USD
```

`watch` reads the selected profile's authenticated endpoint about once per
second. Bars show consumed (`█`), frozen (`▓`), and free (`░`) capacity. The
display is monochrome: segments differ by texture, and changed amounts and new
or changed decision statuses are bold for one refresh. Ctrl-C exits. `--once` prints one snapshot;
failed polls keep the last snapshot with `STALE · retrying` in the header.

The default layout shows all agents and five recent decisions, trimmed to the
terminal height. `--compact` shows bars only. `--events N` accepts 1–10;
`--agent NAME|ID` selects exactly one agent and filters the feed locally. A
single-agent view expands the bar and shows consumed, frozen, and free amounts.
Amounts are exact, and in the multi-agent view they line up on the decimal
point.
The layout fits the terminal width, from 70 to 100 columns: wider terminals
show longer agent names (up to 24 characters), rule names and bars. Output that
is not a terminal, such as `--once` piped to a file, uses 70 columns.

Each bar describes the latest operation's current effective budget in the
selected currency (USD by default), falling back to the tightest current cap.
Overlapping limits are never added. Revoked, scheduled, and expired budgets are
excluded; agents without an allocation show `no current budget`. Expired
unclaimed holds return to available capacity in the read-only projection;
claimed provider uncertainty remains frozen until resolution. Used means
consumed plus frozen; the percentage is floored against the selected budget's
current version limit. Overruns fill the bar while retaining actual dollar
amounts and percentages above 100%.

The combined feed shows the ten newest operation decisions available from the
server, each with its local date and time (`YYYY-MM-DD HH:MM:SS`),
with current status updated in place: `◐` reservation, `✓` settlement,
`‖` approval, `✗` denial, `○` release/expiry, or `!` reconciliation. Settlements
show exact vendor cost, including sub-cent precision. The first matched rule
appears, or `budget` for capacity denials and `default` when no rule matched.
No runtime telemetry, provider calls, approval actions, or process control are
included.

The authenticated `GET /watch?currency=usd` endpoint returns `hubu-watch-v1`, an
owner-scoped consistent budget/version/decision/hold/receipt snapshot. Rows
include the exact `limit` money object (and the legacy `limit_cents`) from the
current selected budget version; `recent_events`
is newest first and capped at ten across the owner's agents in that currency.
Denied decisions are included while authorization-history discovery retains
its existing behavior. Raw reasons, operation keys, authorization tokens,
provider references, pricing evidence, and artifacts are excluded.

## Save artifacts into a project folder

Allowlist the folders agents may save into. Agents refer to them only by name;
they never choose a path.

```sh
hubu folders allow brand /Users/alice/projects/acme/brand
hubu folders list
```

An agent then calls `hubu_save_artifact` over MCP with the operation handle,
the folder name and the chosen artifact. The same save is available from the
CLI with the selected stack profile:

```sh
hubu artifact save \
  --operation-handle hubu:public-operation:v1:EXACT_COMPLETED_HANDLE \
  --folder brand \
  --artifact-id ARTIFACT_ID \
  --name logo-final
```

This writes `logo-final.png` and `logo-final.png.receipt.json`, which records
the artifact ID, SHA-256 digest and exact settled operation cost. An existing
file with different content is never overwritten. `--artifact-id` may be
omitted when the operation produced one artifact. Remove a folder with
`hubu folders remove brand`. See the
[unified MCP reference](unified-mcp.md#save-artifacts-into-a-project-folder)
for verification and receipt details.

## Export completed images

The native client reads an existing succeeded operation and saves its image
bytes directly to your chosen local directory. Choose that directory once in
your session instructions; it does not need to be configured before starting
the stack. The selected profile supplies both backend credentials and the
operation registry. An explicit `--stack-profile PATH` selects another profile.

```sh
hubu gallery export \
  --operation-handle hubu:public-operation:v1:EXACT_COMPLETED_HANDLE \
  --output /Users/alice/Pictures/hubu-demo \
  --size 2k \
  --tier draft
```

This can produce `01-flux-2k-draft-5.8c.png` with a receipt and hidden gallery
manifest. The cost is the exact settled operation total, not a per-image price;
Hubu charges the budget the same exact amount, rounding up only digits beyond
the sixth decimal place. Size is the `image_size`
you submitted (`custom` if none); the receipt also records Gongbu's pixel
dimensions. The tier is your
reviewed local label. Re-exporting unchanged evidence reuses the same file.

The command only reads backend state and does not submit, approve, or resume
provider work. Image bytes never pass through the model. For manual backend
environment configuration, use `hubu-unified-mcp gallery export` with the same
export arguments. See the [gallery skill](../skills/hubu-demo-gallery/SKILL.md)
and [unified MCP reference](unified-mcp.md) for validation and supported formats.
