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
create an operator-owned stack profile. Before testing, verify that
`command -v hubu` resolves to this installation and that `hubu --version`
reports the release you intended to build. This quick start uses the generated
managed-Hubu endpoint and database; for an external Hubu or a custom database,
follow the [local stack guide](docs/local-stack.md) and configure explicit
external-service credential references.

Initialize the profile, complete the non-secret topology and provider choices,
and start the principal-neutral stack:

```sh
profile=/absolute/path/to/profile
hubu stack init --profile "$profile"
hubu stack select --profile "$profile"
# Edit stack.toml and providers.toml. Add credentials.toml provider references
# only for live targets; managed service credential paths are internal.
hubu stack start
hubu stack status
hubu protocol agent-registration
hubu register human --username alice-example --display-name "Alice Example"
hubu register agent --name local-agent --version local-dev
```

After `stack start`, ordinary server-bound CLI commands use the selected
profile's authenticated client handoff, including its Hubu endpoint and
authentication, approval, and reconciliation credential files. Stale
`HUBU_URL` and token environment variables therefore cannot split a command
across different profiles. An explicit global `--url` opts into manual mode
and keeps the legacy environment/file credential behavior.

Registration happens against the running Hubu service and does not require a
new stack render, activation, stop, or restart. Replace `agt_...` below with the
printed agent ID, then apply a starter policy and create the agent's active USD
budget:

```sh
hubu policy new-template --path "$profile/starter-policy.yaml"
hubu policy validate --path "$profile/starter-policy.yaml"
hubu policy apply --path "$profile/starter-policy.yaml"
hubu budget create --agent-id agt_... --amount 25
```

`stack start` validates and renders the profile, starts the final managed Hubu
process, lets it create its private capabilities at profile-owned locations,
completes Gongbu's internal credential handoff, and then starts Gongbu and, when
configured, its managed Temporal runtime. Connect the running stack to Codex:

```sh
hubu init codex --stack-profile "$profile"
```

After restarting Codex, agents can request governed provider-backed work
through Hubu's unified MCP tools: Hubu authorizes and reserves budget, Gongbu
executes the work and stores its artifacts, and Hubu records the outcome. Live
provider execution is experimental and can incur charges; use explicit targets
and conservative spend ceilings.

## Documentation

[Read the Hubu documentation](https://hubustack.dev/)
for the project overview, concepts, architecture, setup, operations, and
protocol references.
