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

Registration guidance, derived fields, fingerprints, and record reuse are
documented in [Agent registration](agent-registration.md).

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

Budgets are cumulative limits for exact agents. `--amount` is a USD amount;
Hubu prints the resulting public budget ID and active period.

```sh
hubu budget create --agent-id agt_FIRST_AGENT_ID --amount 10
hubu budget create --agent-id agt_SECOND_AGENT_ID --amount 10
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

## Ledger and authorization record history

`hubu ledger list` prints canonical JSON for wallet payments, provider expenses
and corrections. Select an agent budget with `--agent-id` and `--budget-id`, and
page with `--limit` and `--cursor`. `hubu spend authorizations` lists authorization records;
`hubu spend show --authorization-id ID` inspects one public authorization record. See
[ledger and authorization records](ledger-history.md) for examples and cost semantics.

## Watch budgets during the demo

```sh
hubu hud
hubu hud --once
hubu hud --currency USD
```

The HUD reads the selected profile's authenticated Hubu endpoint once per
second. Each agent has available, frozen, consumed, and ALLOW / APPROVAL /
BLOCKED columns, followed by its latest event. A `*` and the accent color mark
changed values for one refresh; Ctrl-C exits. `--once` prints a plain snapshot
for scripts. Failed polls retain the last snapshot with a STALE notice.

Amounts in the row describe the displayed operation's currently effective
budget in the selected currency (USD by default), falling back to the tightest
current cap when no operation allocation exists. Overlapping budget limits are
never added; when several budgets exist, the display names the selected allocation. Revoked,
scheduled, and expired budgets are excluded. An agent without a current budget
shows unavailable amounts and BLOCKED. Expired unclaimed holds are projected
back into available capacity without modifying storage; claimed provider
uncertainty remains frozen until resolution. APPROVAL means at least one operation
is still awaiting approval; otherwise the most recent operation's denial shows
BLOCKED. ALLOW describes observed governance, not a guarantee for future spend.

The event shows trusted provider attribution, the deciding matched rule (or
`default / budget`), the reservation, and exact settled vendor cost. The
conservative rounded budget charge appears separately. Hubu does not persist
image dimensions in its spend contract, so `size —` means unavailable; the HUD
does not infer dimensions from prompts. No runtime, tokens, provider calls,
approval actions, or process control are included.

The authenticated `GET /hud?currency=usd` endpoint returns `hubu-hud-v1`, an
owner-scoped consistent budget/decision/hold/receipt snapshot. Denied decisions
are intentionally included here while existing authorization-history discovery
keeps its original behavior. Raw reasons, operation keys, authorization tokens,
provider references, pricing evidence, and artifacts are excluded.
