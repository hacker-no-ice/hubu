# Local stack quick start

This guide takes you from a fresh Mac to an agent making its first governed
request against a local Hubu stack. The stack is three cooperating services:
Hubu decides whether an agent may spend, Gongbu runs the approved provider
work, and Temporal keeps that work durable. Your agent reaches them through one
MCP server, `hubu-unified-mcp`.

## Before you start

- **macOS only for now.** The supported install path builds from source on a
  Mac.
- **Prerequisites:** Git, Xcode Command Line Tools (`xcode-select -p` must
  succeed), `rustup` (the checkout's `rust-toolchain.toml` pins the exact Rust
  toolchain), and the Protocol Buffers compiler `protoc` (for example
  `brew install protobuf`). The Temporal CLI is also required;
  `hubu stack init --install-temporal` below can install it with Homebrew.
- **Build time:** installation is a release build of the whole workspace, so
  the first run downloads dependencies and can take a while.
- **Cost:** this guide uses *sandbox* mode, which replaces the external AI
  provider with a deterministic local fixture. It needs no provider
  credentials and cannot incur charges.

## Install

Pick an exact release tag. On that tag's GitHub Release page, copy the full
40-character `Source commit`, then clone the tag and run the installer:

```sh
tag=vX.Y.Z
expected_commit=FULL_40_CHARACTER_COMMIT_SHA

git clone --depth 1 --branch "$tag" https://github.com/hacker-no-ice/hubu.git
cd hubu
./scripts/install-from-source.sh --expected-commit "$expected_commit"
```

The installer refuses to build if the checkout does not match that commit. It
installs `hubu`, `hubu-server`, `gongbu-server`, and `hubu-unified-mcp` into
`~/.local/bin` by default (`--prefix` chooses another absolute prefix). Put
`PREFIX/bin` on your `PATH` and confirm that all four report the same release:

```sh
export PATH="$HOME/.local/bin:$PATH"
for binary in hubu hubu-server gongbu-server hubu-unified-mcp; do
  command -v "$binary"
  "$binary" --version
done
```

For updates, uninstalling, and the local-build trust model, see
[release operations](operations/releases.md#install-an-exact-release-from-source-macos).

## Create a sandbox profile

A *profile* is a directory that holds one stack's configuration, databases,
artifacts, and logs. Choose an absolute path, initialize it in sandbox mode,
and select it so later commands use it by default:

```sh
profile="$HOME/hubu-sandbox"
hubu stack init --mode sandbox --install-temporal --profile "$profile"
hubu stack select --profile "$profile"
```

On macOS, `--install-temporal` runs Homebrew's official Temporal package only
when the Temporal CLI is missing, then records its path and exact version in
the profile. Omit the flag if you manage Temporal yourself. Initialization
never overwrites existing files and never starts services. It creates:

```text
PROFILE_ROOT/
  README.md
  stack.toml
  credentials.toml
  providers.toml
  generated/
  state/
    credentials/
      .gitignore
```

| File | Purpose |
| --- | --- |
| `stack.toml` | Which binaries to run, service ports, Temporal, and local paths ([reference](https://hubustack.dev/configuration/local-stack/v1/stack-toml)) |
| `credentials.toml` | References to provider secrets for live mode; empty in sandbox ([reference](https://hubustack.dev/configuration/local-stack/v1/credentials-toml)) |
| `providers.toml` | Which provider targets agents may use and their prices; sandbox has one fixture target ([reference](https://hubustack.dev/configuration/local-stack/v1/providers-toml)) |

The sandbox profile works without edits. Never put API keys or tokens in these
files, and do not edit `generated/` or `state/`; Hubu manages them.

Other modes exist: `local-stack` uses real, billable providers you approve, and
`hubu-only` runs governance without Gongbu or Temporal. Keep each mode in its
own profile; the [complete examples](https://hubustack.dev/configuration/local-stack/v1/examples)
show all three.

## Start and check

Check the profile, start the stack, and confirm it is ready:

```sh
hubu stack doctor
hubu stack start
hubu stack status
```

`doctor` is read-only and reports any field that needs attention. A fresh
sandbox profile should report `ready_to_render`. `start` validates the profile,
then launches Hubu, Gongbu, and Temporal and provisions their internal
credentials inside the profile. `status` should then look like this
(illustrative and abbreviated):

```text
Hubu stack status

Summary
  Profile               /Users/you/hubu-sandbox
  Classification        running_ready
  Generation            <64-character id>
  Source/render drift   no
  Restart impact        none

Components
  hubu
    Ownership         managed
    Lifecycle         owned_running
    Ready             yes
    ...
  gongbu
    Ownership         managed
    Lifecycle         owned_running
    Ready             yes
    ...

Temporal
  Ownership             gongbu_managed_local
  Worker ready          yes
  Namespace             default
  Task queue            gongbu-local-executions
  UI                    http://127.0.0.1:8233

Unified MCP
  Lifecycle             client_owned
  Compatible            yes
  Guidance              hubu init codex --stack-profile /Users/you/hubu-sandbox
...
```

If a component is not `owned_running`, its `Guidance` line names the recovery
command, and `hubu stack logs` shows its recent log lines.

## Connect your agent

Write Codex's MCP configuration from the running profile, then restart Codex:

```sh
hubu init codex --stack-profile "$profile"
```

On first run in a terminal, the command asks whether Codex may use Hubu setup
and admin tools (registering agents, applying policies, changing budgets).
Answer yes to do the setup in "Make your first governed request" from Codex
instead of the CLI; Codex still asks you to approve each call. The choice is
remembered on re-runs. Override it with `--trust-client-approval` or
`--no-trust-client-approval`.

Codex starts `hubu-unified-mcp` itself; the stack does not own that process.
Other MCP clients can connect to `hubu-unified-mcp` with manual configuration;
see [Unified MCP setup](unified-mcp.md#setup).

## Make your first governed request

A *governed request* is one where Hubu checks a policy and a budget before any
provider work starts. First create an owner, an agent, a policy, and a budget.
The starter policy allows requests up to $100 and asks for approval otherwise;
the sandbox fixture's synthetic price is one cent per image.

```sh
hubu register human --username alice-example --display-name "Alice Example"
hubu register agent --name image-designer --version local-dev
hubu policy new-template --path policies/policy.yaml
hubu policy validate --path policies/policy.yaml
hubu policy apply --path policies/policy.yaml --name "Starter policy"
hubu budget create --agent-id agt_EXACT_AGENT_ID --amount 1
```

`register agent` prints the `agent_id` (for budgets and policies) and the
`account_id` (for spending). The [CLI administration reference](cli.md)
explains each command.

Then, in a new Codex thread, ask:

```text
Call gongbu_list_execution_targets and choose the sandbox fixture. Then use
hubu_submit_governed_execution with account ID aga_EXACT_ACCOUNT_ID and the
target's returned execution scope to generate one image of a blue circle.
Show any approval request before asking me to approve or deny it, and return
the resulting artifact.
```

The agent discovers the approved target, then submits one request. Hubu
evaluates the policy and reserves budget before Gongbu runs the fixture. The
tool returns one of these outcomes:

- `succeeded`: the image is returned in the same response when it fits.
- `in_progress`: the work continues in the background; ask the agent to check
  it with `hubu_operation_status`.
- `approval_required`: nothing has run yet. Review the request, then say
  `approve` or `deny` in the chat. Codex shows its own confirmation prompt
  before recording your decision; canceling that prompt leaves the request
  pending rather than denying it. After approval, the agent continues with
  `hubu_resume_operation`.
- `denied`: the policy refused the request and nothing ran.
- `failed`: the execution reached a terminal failure. Do not submit a
  replacement request; ask the agent to check the existing operation with
  `hubu_operation_status` and follow its recovery guidance.

See [Composite governed execution](unified-mcp.md#composite-governed-execution)
for the full flow.

Finally, for a request that ran (or was approved and resumed), inspect what
Hubu recorded:

```sh
hubu spend authorizations --limit 5
hubu ledger list --agent-id agt_EXACT_AGENT_ID --budget-id bgt_EXACT_BUDGET_ID --limit 5
```

Both print versioned JSON. The authorization record shows the decision and its
lifecycle status; the ledger shows recorded spend and how much of the budget is
used. [Ledger and authorization records](ledger-history.md) explains the
fields. A `denied` request records no spend and is not listed in authorization
history; ask the agent to check it with `hubu_operation_status` instead.

## Next steps

- [Managing a local stack](operations/managing-a-stack.md): logs, stopping,
  configuration changes, and rollback.
- [Live provider operations](operations/live-providers.md): move to real,
  billable providers in a separate profile.
- [Configuration reference](https://hubustack.dev/configuration/local-stack/v1/)
  and [complete examples](https://hubustack.dev/configuration/local-stack/v1/examples).
- [Policy engine](policy-engine.md): write your own spending rules.
- [Unified MCP surface](unified-mcp.md): every agent-facing tool.
