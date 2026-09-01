# Hubu / 户部

Hubu is an open-source spending control plane for AI agents.

It gives agents permission to use paid services without giving them direct
control of payment keys, provider credentials, or unrestricted budgets.

> **Experimental and local-first**
>
> Hubu currently provides a local environment for developing and testing
> governed agent workloads. Provider-backed workloads remain experimental.
> Hubu is not approved for real-money, money-grade production use, and its
> current security boundaries should not be treated as production financial
> infrastructure.

## Why Hubu and Gongbu?

The names come from two ministries in imperial China.

**Hubu (户部)** was the Ministry of Revenue. It managed state finances,
taxation, budgets, and household records.

**Gongbu (工部)** was the Ministry of Works. It organized construction,
infrastructure, equipment, and the execution of public projects.

That historical division captures the spirit of the system:

> **Hubu governs resources. Gongbu performs the work.**

Hubu decides whether work may consume money. Gongbu carries out the authorized
work with the provider. The two cooperate, but retain separate responsibilities,
credentials, storage, and failure boundaries.

[Explore the interactive architecture →](../architecture/index.html)

## The core idea

Agents can have budgets, but they should not hold private keys.

A human establishes the operating boundary: who the agent is, what it may
purchase, how much it may spend, and when human approval is required.

When an agent requests paid work, Hubu evaluates the request against that
boundary. An allowed request reserves budget before execution begins. A denied
request stops without consuming funds. A request that needs approval waits for
an explicit human decision.

Gongbu receives authorized work, communicates with the configured provider,
manages retries and artifacts, and reports the outcome. Hubu then settles the
actual cost, releases unused budget, or preserves the operation for
reconciliation when the provider's billing result is uncertain.

## One governed workload

The complete idea can be followed as one continuous lifecycle:

```text
Agent request
    ↓
Identity, policy, and budget evaluation
    ↓
Spend authorization and budget reservation
    ↓
Provider execution through Gongbu
    ↓
Settlement, release, or reconciliation
```

Every logical operation has a stable identity so retries recover the same
financial state instead of creating accidental duplicate spend. Decisions and
successful money movement are recorded for later inspection.

The result is a separation of authority:

- The agent proposes work.
- Hubu governs whether resources may be used.
- Gongbu performs the authorized work.
- Humans retain policy and approval authority.

## One agent-facing surface

Agents interact with Hubu and Gongbu through the unified MCP server.

The unified surface makes both governance and execution tools discoverable in
one place, while preserving the two backend boundaries. It routes each request
to its actual owner rather than combining financial authority and provider
execution inside one process.

Codex is one supported MCP client, but the architecture is designed around the
protocol rather than a single agent harness.

## Start exploring

- [Start the local Hubu stack →](local-stack.md)
- [Operate supported live providers →](operations/live-providers.md)
- [Understand policy, budgets, and spend lifecycle →](spend-lifecycle.md)
- [See how Gongbu executes authorized work →](gongbu-execution.md)
- [Integrate through unified MCP →](unified-mcp.md)

For operational procedures, protocol details, and troubleshooting, continue
through the documentation navigation.
