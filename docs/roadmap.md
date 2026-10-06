# Roadmap

Maintained by [@hacker-no-ice](https://github.com/hacker-no-ice), currently
the sole maintainer. These are directions, not dated commitments; they change
as the project learns from people using it.

## Near-term focus: easy and safe for individuals and small teams

The next stretch of work makes Hubu easy and safe for one person or a small
team, rather than expanding to more platforms and providers.

### Easier to use

- A shorter path from installation to the first governed request.
- Clearer setup, diagnostics, and error messages, so problems explain
  themselves.
- A simpler approval flow in the agent client.

### Safer to use

- Safe defaults: conservative budgets, with approval required unless you
  choose otherwise.
- Clear previews of what a request will cost before you approve it.
- Continued hardening of how keys and approval authority are handled.

### Small teams

- Shared budgets and approvals for a few people working with the same agents.

## Not the focus right now

These are reasonable requests, but they are not planned for the near term:

- More live providers.
- Guided setup for more agent clients. Other MCP clients already work with
  [manual configuration](unified-mcp.md#setup).
- Linux and other platforms.
- A production payment rail.

## Get involved

Hubu welcomes contributors. If this direction is useful to you,
[open an issue](https://github.com/hacker-no-ice/hubu/issues/new?template=idea.md)
to discuss an idea or say what you would like to work on, or
[report a bug](https://github.com/hacker-no-ice/hubu/issues/new?template=bug.md).
