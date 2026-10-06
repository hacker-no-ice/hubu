# Roadmap

Maintained by [@hacker-no-ice](https://github.com/hacker-no-ice), currently
the sole maintainer. Themes are listed in the order they are planned. They are
directions, not dated commitments, and they change as the project learns from
people using it.

## 1. Easy and safe for individuals and small teams

The first priority is making Hubu easy and safe for one person or a small
team.

**Easier to use**

- A shorter path from installation to the first governed request.
- Clearer setup, diagnostics, and error messages, so problems explain
  themselves.
- A simpler approval flow in the agent client.

**Safer to use**

- Safe defaults: conservative budgets, with approval required unless you
  choose otherwise.
- Clear previews of what a request will cost before you approve it.
- Continued hardening of how keys and approval authority are handled.

**Small teams**

- Shared budgets and approvals for a few people working with the same agents.

## 2. Governed model calls

Today Hubu governs image generation. The next workload is calls to advanced AI
models.

For example, you use a low-cost agent subscription but want the agent to call
a more capable model for some tasks. Hubu would govern those calls the same way
it governs image generation: policy and approval, funds reserved before the
call, and the actual cost settled in the same ledger. The agent still never
holds the API key.

- Gongbu integrations for calling models directly through a provider's API, or
  through an API gateway you already use.
- No change to the architecture: Hubu governs, Gongbu executes, and agents use
  the same MCP surface.

## 3. A deployed Hubu for cloud agents

Hubu runs on your own machine today. Some agents run persistently in their own
cloud virtual machine, where they cannot reach a stack on your laptop.

- A deployed version of Hubu that these persistent cloud agents can connect
  to, so they get the same per-agent identity, policy, approval, budgets, and
  ledger.
- Human approvals still come from you, not from the agent's environment.

## Not the focus right now

These are reasonable requests, but they are not planned for the near term:

- Broad provider coverage beyond the image and model workloads above.
- Guided setup for more local agent clients. Other MCP clients already work
  with [manual configuration](unified-mcp.md#setup).
- A production payment rail.

## Get involved

Hubu welcomes contributors. If this direction is useful to you,
[open an issue](https://github.com/hacker-no-ice/hubu/issues/new?template=idea.md)
to discuss an idea or say what you would like to work on, or
[report a bug](https://github.com/hacker-no-ice/hubu/issues/new?template=bug.md).
