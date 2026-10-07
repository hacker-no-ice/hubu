# Roadmap

Maintained by [@hacker-no-ice](https://github.com/hacker-no-ice), currently
the sole maintainer. Themes are listed in the order they are planned. They are
directions, not dated commitments, and they change as the project learns from
people using it.

<figure class="roadmap-diagram" aria-label="Three roadmap priorities in planned order">
<ol>
<li><a href="#1-easy-and-safe-for-individuals"><span class="roadmap-stage">01 · INDIVIDUALS</span><svg viewBox="0 0 48 48" width="48" height="48" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2"><rect x="7" y="8" width="34" height="24" rx="4"/><path d="M17 40h14M24 32v8M18 20l4 4 8-8"/></svg><strong>Easy &amp; safe locally</strong><span>Less setup friction.<br>Clear costs. Safe defaults.</span></a></li>
<li><a href="#2-governed-model-calls"><span class="roadmap-stage">02 · WORKLOADS</span><svg viewBox="0 0 48 48" width="48" height="48" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2"><rect x="12" y="12" width="24" height="24" rx="5"/><path d="M18 5v7M30 5v7M18 36v7M30 36v7M5 18h7M5 30h7M36 18h7M36 30h7M19 24h10M24 19v10"/></svg><strong>Govern model calls</strong><span>Advanced models.<br>Same policy and ledger.</span></a></li>
<li><a href="#3-a-deployed-hubu-for-cloud-agents-and-small-teams"><span class="roadmap-stage">03 · DEPLOYMENT</span><svg viewBox="0 0 48 48" width="48" height="48" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2"><path d="M13 30h22a7 7 0 0 0 1-14 12 12 0 0 0-23-1 8 8 0 0 0 0 15ZM24 30v9M12 39h24"/><circle cx="12" cy="40" r="3"/><circle cx="36" cy="40" r="3"/></svg><strong>Cloud agents &amp; teams</strong><span>Linux deployment.<br>Shared budgets and roles.</span></a></li>
</ol>
<figcaption>Planned order, not release dates. Each priority builds on the same spending boundaries.</figcaption>
</figure>

## 1. Easy and safe for individuals

The first priority is making Hubu easy and safe for one person running it on
their own machine.

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

## 3. A deployed Hubu for cloud agents and small teams

Hubu runs on your own machine today, and only that machine can reach it.
Persistent agents such as ChatGPT dots, Grok Bot, and Muse run in
their own cloud computers, where they cannot reach a stack on your laptop, and
teammates cannot share it either.

- A deployed version of Hubu that these persistent cloud agents can connect
  to, so they get the same per-agent identity, policy, approval, budgets, and
  ledger.
- Linux support, since a deployed Hubu runs on servers rather than a Mac.
- Human approvals still come from you or the people you authorize, never from
  the agent's environment.
- One deployment per team, so a few people can share budgets and approvals,
  with roles for who can approve spending or change budgets.

## Not the focus right now

These are reasonable requests, but they are not planned for the near term.
If there is strong demand from people using Hubu, the focus can shift, so
[tell us](#get-involved) what you need.

- Broad provider coverage beyond the image and model workloads above.
- Platforms beyond macOS and the Linux deployment above.
- Guided setup for more local agent clients. Other MCP clients already work
  with [manual configuration](unified-mcp.md#setup).
- A production payment rail.
- Multi-tenant hosting, where many separate organizations share one service.

## Get involved

Hubu welcomes contributors. If this direction is useful to you,
[open an issue](https://github.com/hacker-no-ice/hubu/issues/new?template=idea.md)
to discuss an idea or say what you would like to work on, or
[report a bug](https://github.com/hacker-no-ice/hubu/issues/new?template=bug.md).
