# HUB-97 unified MCP immediate cutover

Effective when HUB-97 merges, `hubu-unified-mcp` is the only supported
agent-facing MCP surface. The reviewed
[HUB-111 GO record](canaries/HUB-111-final-unified-mcp-cutover-go.md) authorizes
this immediate zero-user cutover.

Release archives and generated client configuration include only the unified
agent surface. `hubu-mcp-server` and `gongbu-mcp` are deprecated, unsupported,
and excluded from primary packaging. Direct source-built invocation prints a
static warning and points to the
[migration guide](unified-mcp-migration.md); it never prints configuration or
credential values.

This cutover does not merge runtimes. `hubu-server` and `gongbu-server` remain
separate supported processes with separate credentials, databases, provider
execution, artifacts, lifecycle, and failure domains.

Standalone adapter source remains only as removal staging. HUB-98 removes it
after this cutover; retaining source until then is not a support or
compatibility commitment. The reviewed HUB-111 artifact remains the immutable
rollback evidence required through HUB-98 verification.
