# Threat model and trust boundaries

This page says what Hubu protects against when an agent runs on your Mac,
and what it doesn't. Read it before you decide how much to hand over to an
agent.

The short version: today an agent that can run shell commands as your macOS
user can do anything you can do with the `hubu` CLI, including raising its own
budget. The v0.3.0 release (Aegislash) is designed to close that gap for owner
actions. It is still in progress, and this page marks which protections have
shipped.

## Adversary

Hubu's threat model has one adversary: **an agent with a shell running as the
owner's macOS user**. The agent may be mistaken, or it may be acting on a
prompt injection. It can run any program you can run, read any file your user
can read, and reach any local service your user can reach.

Out of scope:

- Social engineering of the owner. If you approve something because an agent
  talked you into it, Hubu records the approval as yours.
- Other macOS users, malware with root, and physical access to the Mac.
- Provider invoices. Hubu's claims cover its own ledger and records; see the
  [system card](../break-hubu/README.md).

## Design rule

> Anything that grants authority needs the owner's presence. Everything else
> must be tamper-evident.

Granting authority means registering agents, applying or loosening policies,
creating or raising budgets, changing spending targets, approving spend, and
reconciling claims. Agents may request spend, read their own state, and draft
changes; they may not grant themselves authority.

## Today (v0.2.5 and current `main`)

### What binds an agent

These controls hold for an agent that only talks to Hubu through the unified
MCP server, with no other route to Hubu's credentials:

- **Policy evaluation.** Every spend request is evaluated against the agent's
  policy before any provider work starts. A denial can't be bypassed through
  the API ([C2.1](../break-hubu/ep05-denied-operation/README.md)).
- **Budgets and holds.** Hubu reserves budget before execution and records
  overruns ([C1](../break-hubu/README.md)).
- **Human approval for `needs_approval`.** A request that policy sends for
  approval stops until a human resolves it, and one approval covers exactly
  one operation and amount
  ([C2.2](../break-hubu/ep06-approval-reuse/README.md)).
- **Operator review.** You can check what an agent changed with
  `hubu agent list`, `hubu policy show`, `hubu budget list` and
  `hubu spend authorizations`, or in the [admin web UI](admin-ui.md).

### The MCP admin gate is not a security boundary

`hubu init codex` asks once whether Codex may use Hubu's setup and admin
tools, and `--trust-client-approval` / `--no-trust-client-approval` change
the answer. Turning them off hides registration, policy, budget,
spending-target and reconciliation tools from the MCP surface. See the
[approval boundary](unified-mcp.md#approval-boundary).

That gate only binds an agent with no other way to reach the same authority.
In the common local setup, Codex runs in your terminal and can run the `hubu`
CLI. The CLI uses the selected stack profile's owner credential files, so the
agent can apply policies and create or raise budgets whether or not the MCP
admin tools are enabled. The v0.2.5 system card records this as
[C5.1](../break-hubu/ep09-raise-own-budget/README.md): **does not hold**.

For an agent with a shell, the gate is a convenience and a statement of
intent, not a security boundary. It is a real boundary when the agent has no
shell and can't read the profile's credential files or reach the Hubu server
directly: for example ChatGPT through the
[MCP tunnel](operations/chatgpt-mcp-tunnel.md), Claude Desktop with no shell
tools, or a Codex sandbox that can't read the stack profile.

### What an agent with a shell can reach today

- **Owner credentials on disk.** The profile's Hubu bearer, approval and
  reconciliation credential files are readable by your macOS user, and so by
  any agent running as you.
- **The CLI and the API.** The `hubu` CLI and the local Hubu HTTP API accept
  those credentials for every owner action.
- **The admin web UI.** Whoever can run `hubu ui` gets a one-time login
  link.
- **Other agents' credentials.** Agents sharing your macOS user can read each
  other's credential files ([C3](../break-hubu/README.md) does not hold).

## v0.3.0 (Aegislash): planned protections

v0.3.0 is designed to make C5 ("an agent can't change its own governance")
hold against the same-user shell adversary. Each row links the issue that
ships it; until that issue is done, the protection is not in place.

| Protection | What it does | Status |
| --- | --- | --- |
| Owner and agent credential classes | Splits the shared bearer so an agent credential can't perform owner actions. | In progress ([HUB-345](https://linear.app/hubu/issue/HUB-345)) |
| Agent allowlist on unified MCP | Reduces the MCP surface to the agent tools and removes the admin gate described above. | Planned ([HUB-346](https://linear.app/hubu/issue/HUB-346)) |
| Owner presence with Touch ID | Owner actions need a Touch ID dialog, followed by a five-minute owner window. | Planned ([HUB-347](https://linear.app/hubu/issue/HUB-347)) |
| Admin portal with Touch ID | The portal signs in and acts with Touch ID and retires the one-time login link. | Planned ([HUB-348](https://linear.app/hubu/issue/HUB-348)) |
| Owner presence in the CLI | Owner CLI commands require owner presence. | Planned ([HUB-349](https://linear.app/hubu/issue/HUB-349)) |
| Agents draft, the owner applies | Agents write policy and budget changes as files; the owner applies them on the portal. | Planned ([HUB-350](https://linear.app/hubu/issue/HUB-350)) |

With these in place, an agent with a shell can still request spend within its
policy and budget, but it can't register, approve, raise a budget or loosen a
policy without you at the Mac.

### Later releases

- **Agent identity.** Binding agent account use to an authenticated agent
  session, so one agent can't spend as another
  ([HUB-162](https://linear.app/hubu/issue/HUB-162)).
- **Key vault.** Loading Hubu and unified MCP credentials from the macOS
  Keychain instead of files
  ([HUB-112](https://linear.app/hubu/issue/HUB-112)).
- **Tamper evidence and approval attestation.** Recording who approved what,
  with evidence that can be checked later
  ([HUB-87](https://linear.app/hubu/issue/HUB-87)).

## Not protected, even after v0.3.0

- **Agents sharing one macOS user can read each other's credential files.**
  Until agent identity and the key vault land, agent-to-agent isolation needs
  separate macOS users or sandboxes.
- **Your signed-in browser and provider CLIs.** An agent can use a browser
  session, desktop app or provider CLI that you've already signed in to, and
  spend outside Hubu entirely. Hubu only governs spend that goes through it.
- **Same-user memory reads.** A process running as your user that can read
  the Hubu, Gongbu or unified MCP process memory can recover what they hold.

## Recommended setup

- **Run agent harnesses with their default permissions**, not full access or
  "skip approvals" modes, so shell commands that touch Hubu still ask you.
- **Keep provider accounts prepaid with no auto top-up.** The provider balance
  is the hard ceiling if everything else fails.
- **Keep provider logins out of the agent's reach.** Don't leave provider
  CLIs signed in or provider dashboards open in a browser the agent can drive.
  Let Gongbu hold provider credentials.
- **Choose a setup that fits how far you trust the agent:**
  - *Trusted local agent.* Enable the admin tools at `hubu init codex` and let
    the agent do setup. Review the result with `hubu policy show` and
    `hubu budget list`. Assume the agent can also reach the CLI.
  - *CLI-only administration.* Answer no at `hubu init codex` and do all
    setup yourself from the terminal ([CLI administration](cli.md)). This
    stops honest mistakes through MCP, but not an agent that runs the CLI.
  - *Sandboxed or shell-less agent.* Run the agent where it can't read the
    stack profile or run `hubu`. This is the only setup today where the MCP
    gate is a real boundary.
- **Report a gap** through the [security policy](../SECURITY.md).
