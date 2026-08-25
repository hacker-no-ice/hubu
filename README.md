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

Install all four binaries from one
[verified Hubu release](docs/operations/releases.md#pin-verify-and-install), then
install them into `~/.local/bin`. For example, on a Mac with Apple silicon,
replace `vX.Y.Z` with an exact release:

```sh
tag=vX.Y.Z
asset="hubu-${tag}-aarch64-apple-darwin.tar.gz"

gh release download "$tag" --repo hacker-no-ice/hubu \
  --pattern "$asset" --pattern SHA256SUMS
grep "  $asset" SHA256SUMS | \
  (command -v sha256sum >/dev/null && sha256sum -c - || shasum -a 256 -c -)
tar -xzf "$asset"
mkdir -p "$HOME/.local/bin"
install -m 0755 "${asset%.tar.gz}"/{hubu,hubu-server,hubu-unified-mcp,gongbu-server} \
  "$HOME/.local/bin/"
```

Ensure `~/.local/bin` is on your `PATH`, then create an operator-owned stack
profile. This quick start uses the generated managed-Hubu endpoint and database;
for an external Hubu or a custom database, follow the
[local stack guide](docs/local-stack.md) and configure explicit
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

[Read the Hubu documentation](https://hubu-docs.water-no-ice.chatgpt.site/)
for the project overview, concepts, architecture, setup, operations, and
protocol references.
