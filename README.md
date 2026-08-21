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
profile:

```sh
hubu stack init --profile /absolute/path/to/profile
```

Edit the generated `stack.toml`, `credentials.toml`, and `providers.toml` to
choose the local topology, identities, provider targets, pricing, credentials,
and spend ceiling. Start the configured services and check readiness:

```sh
hubu stack start --profile /absolute/path/to/profile
hubu stack status --profile /absolute/path/to/profile
```

`stack start` validates and renders the profile, then starts the managed Hubu
control plane and Gongbu execution plane and, when configured, Gongbu's managed
Temporal runtime. Connect the running stack to Codex:

```sh
hubu init codex --stack-profile /absolute/path/to/profile
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
