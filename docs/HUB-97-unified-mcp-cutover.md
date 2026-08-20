# HUB-97 unified MCP immediate cutover (historical)

Effective when HUB-97 merges, `hubu-unified-mcp` is the only supported
agent-facing MCP surface. The reviewed
[HUB-111 GO record](canaries/HUB-111-final-unified-mcp-cutover-go.md) authorizes
this immediate zero-user cutover.

Release archives and generated client configuration include only the unified
agent surface. `hubu-mcp-server` and `gongbu-mcp` are deprecated, unsupported,
and excluded from primary packaging. At that stage, direct source-built
invocation printed a static warning directing operators to the migration
procedure without printing configuration or credential values.

This cutover does not merge runtimes. `hubu-server` and `gongbu-server` remain
separate supported processes with separate credentials, databases, provider
execution, artifacts, lifecycle, and failure domains.

Standalone adapter source remained only as removal staging until HUB-98 removed
it. The reviewed HUB-111 artifact is the immutable rollback evidence retained
through HUB-98 verification.
