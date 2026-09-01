# Hubu / 户部

Hubu is an open-source spending control plane for AI agents. Humans define
policies and budgets; agents submit structured spend requests; and Hubu
authorizes, executes, and records approved spending without giving agents
private keys.

This repository contains Hubu and its separate Gongbu execution plane in one
Rust workspace. Agents interact through the unified MCP server, while the two
systems keep separate processes, storage, credentials, and failure domains.

> [!WARNING]
> **Project status: experimental and local-first.** Hubu currently runs a
> localhost demo server, uses a mock payment rail, and is **not approved for
> real-money production use**. Its policy, budget, authorization, and ledger
> boundaries are a foundation to harden, not a claim of money-grade security.

## Quick Start

### 1. Install and verify the binaries

For `v0.2.1` and later during the initial technical-user phase, build all four
production binaries locally from one exact macOS source release. Choose an
immutable `vX.Y.Z` tag and copy its published full source commit from the
matching GitHub Release, then run the reviewed installer from that checkout:

```sh
tag=vX.Y.Z
expected_commit=FULL_40_CHARACTER_COMMIT_SHA

git clone --depth 1 --branch "$tag" https://github.com/hacker-no-ice/hubu.git
cd hubu
./scripts/install-from-source.sh --expected-commit "$expected_commit"
```

The installer requires macOS, Xcode Command Line Tools, `rustup`, and `protoc`;
it uses the exact Rust toolchain and dependency lockfile in the checkout. It
installs to `~/.local/bin` by default. See
[release installation](docs/operations/releases.md#install-an-exact-release-from-source-macos)
for prerequisite, trust, custom-prefix, update, and uninstall details.

These executables are compiled locally. They are not Developer ID-signed,
Apple-notarized, or otherwise Apple-verified, and the supported flow does not
ask you to bypass Gatekeeper. Ensure `~/.local/bin` is on your `PATH`, then
visually confirm that all four binaries resolve to the installation you just
built and report the intended release:

```sh
for binary in hubu hubu-server gongbu-server hubu-unified-mcp; do
  command -v "$binary"
  "$binary" --version
done
```

### 2. Initialize the intended mode and configuration

Choose `sandbox` for a complete non-billable first run, `local-stack` for
operator-approved live provider targets, or `hubu-only` when you only need the
governance service. This walkthrough uses sandbox mode; see the
[local stack guide](docs/local-stack.md#choose-an-outcome-and-initialize) for
the other modes and their configuration choices.

```sh
profile=/absolute/path/to/profile
hubu stack init --mode sandbox --install-temporal --profile "$profile"
hubu stack select --profile "$profile"
hubu stack doctor
```

After `stack select`, later stack and server-bound CLI commands use that
profile's configuration and, once the stack is running, its authenticated
client handoff. Sandbox needs no provider edits or credentials. Before starting
a `local-stack` profile, follow its generated comments to configure approved
targets and opaque credential references.

### 3. Start the stack and verify its status

```sh
hubu stack start
hubu stack status
```

`stack start` validates and renders the profile, starts the final managed Hubu
process, lets it create its private capabilities at profile-owned locations,
completes Gongbu's internal credential handoff, and then starts Gongbu and, when
configured, its managed Temporal runtime.

### 4. Connect Codex

Configure Codex from the active profile. The optional trust flag exposes setup
and administration tools so the Codex walkthrough below can register identities,
apply policies, and create budgets; Codex still asks for native confirmation
before those tool calls.

```sh
hubu init codex --stack-profile "$profile" --trust-client-approval
```

Restart Codex after the command completes.

### 5. Verify Hubu in a new Codex thread

Open a new Codex thread in any repository—the generated Hubu MCP configuration
is available across projects. Use `/mcp`, or ask Codex:

```text
List the Hubu MCP tools, then call hubu_unified_capabilities and summarize the
readiness of Hubu, Gongbu, the operation registry, and configured execution
targets. Do not make any changes yet.
```

The catalog should include registration, policy, budget, target-discovery, and
`hubu_submit_governed_execution` tools. The exact available set reflects the
selected profile and backend readiness.

### 6. Try a governed workload from Codex

Keep the remaining walkthrough in Codex. First establish the owner and agent
records:

```text
Read hubu_registration_guidance. Register the human Alice Example with username
alice-example, then register two agents named image-researcher and
image-designer with version local-dev. Show me the returned user, agent, agent
account (`account_id`), version, and session IDs before continuing.
```

The returned `agent_id` identifies the agent for policy and budget assignment;
the returned `account_id` identifies its spending account for governed work.
Let Codex carry those exact values into later calls instead of replacing an
unexplained `agt_...` placeholder by hand.

Next ask Codex to read and follow the repository's
[Hubu Policy Authoring skill](skills/hubu-policy-authoring/SKILL.md). Naming its
absolute path makes it available without a separate skill installation, even
when the Codex thread is in another repository:

```text
Read and follow /absolute/path/to/hubu/skills/hubu-policy-authoring/SKILL.md to
draft a user-default policy for these agents: allow image generation only
through the execution targets configured in this profile, cap each request at
$1.00, and require approval for anything unmatched. Validate the policy, show
me the assignment scope and rules, and wait for my approval before applying it.
After I approve it, create a $10 USD budget for each exact agent ID.
```

Finally, discover rather than guess the configured provider target:

```text
Call gongbu_list_execution_targets and show me the available image-generation
targets and prices. Choose the sandbox fixture, or the configured Gemini or
FLUX target I name, then use hubu_submit_governed_execution to generate one
image within the policy and budget. Use the selected agent account ID and the
target's returned execution scope. Show any approval request before asking me
to approve or deny it, and return the resulting artifact.
```

Sandbox uses a deterministic, non-billable provider fixture. Gemini and FLUX
require a `local-stack` profile with an approved live target and credential
reference; live execution is experimental and can incur charges. Begin with
the [live provider operations guide](docs/operations/live-providers.md), use a
conservative budget, and inspect the discovered price before submitting work.

## Documentation

[Read the Hubu documentation](https://hubustack.dev/)
for the project overview, concepts, architecture, setup, operations, and
protocol references.
