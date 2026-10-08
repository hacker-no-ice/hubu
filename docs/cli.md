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
Hubu may charge a rounded 6 cents against the budget. Size must match the stored
submitted selector; use `custom` if no selector was submitted. The tier is your
reviewed local label. Re-exporting unchanged evidence reuses the same file.

The command only reads backend state and does not submit, approve, or resume
provider work. Image bytes never pass through the model. For manual backend
environment configuration, use `hubu-unified-mcp gallery export` with the same
export arguments. See the [gallery skill](../skills/hubu-demo-gallery/SKILL.md)
and [unified MCP reference](unified-mcp.md) for validation and supported formats.
