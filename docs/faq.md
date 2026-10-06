# FAQ

## Is Hubu production-ready?

No. Hubu is experimental and local-first. It is not approved as money-grade
financial infrastructure, and the built-in direct payment path is a mock. Live
providers do incur real charges, so start in sandbox mode and use conservative
policies and strict budgets when you switch to live mode.

## Does Hubu hold my money?

No. Hubu enforces budgets and keeps a ledger of decisions and money movement.
Provider charges are billed to your own provider accounts. No production
payment rail is supported yet.

## What does Hubu support today?

| Area | Supported |
| --- | --- |
| Platform | macOS, installed from an exact [release tag](operations/releases.md#install-an-exact-release-from-source-macos) |
| Agent clients | Codex with guided setup through `hubu init codex`. Other MCP clients with [manual configuration](unified-mcp.md#setup). |
| Live providers | Gemini Developer API and FLUX.2 Pro; see [live provider operations](operations/live-providers.md) |
| Sandbox | The complete stack with a provider fixture that never bills |

The [overview](overview.md#what-works-today) keeps the full list current.

## Can I use Hubu without Gongbu?

Yes. `hubu-only` mode runs registration, policy, authorization, and budgets
without Gongbu or provider execution. You can also connect
[your own executor](external-executor.md) through the versioned executor
contract.

## How do approvals work in my agent client?

When a policy requires approval, Hubu holds the request and returns a review
to your client. No payment, execution, or provider call happens yet. After you
decide, the agent forms a protected tool call carrying your approve or deny
decision, and your client shows its own confirmation prompt before it runs.

- Check that the confirmation prompt shows the decision you chose. The agent
  writes the approve or deny value into the call, so confirm only if it
  matches.
- Canceling that prompt sends no decision. The request stays pending; it is
  never treated as approved or denied.
- The authority to resolve approvals comes from a separate local credential.
  The model never sees or supplies it.
- Administrative actions such as changing policies or budgets through MCP stay
  disabled unless you explicitly opt in.

See the [approval boundary](unified-mcp.md#approval-boundary) for details.

## What leaves my machine?

Hubu does not send telemetry, analytics, crash reports, or update checks. Its
logs are written to local files.

| Outbound traffic | When |
| --- | --- |
| Provider API calls from Gongbu | Only for requests that policy allowed, or that you approved, and only to the hosts configured for that provider target |
| Generated images downloaded from a provider | Only from that provider's approved download hosts |
| Homebrew installing the Temporal CLI | Only if you pass `--install-temporal` |
| Downloading Rust dependencies | When you build Hubu from source |
| A feedback report | Only after you review it and explicitly submit it; `hubu feedback prepare` never sends anything ([send feedback](feedback.md)) |

Hubu and Gongbu listen only on loopback addresses, so other machines cannot
reach them.

Your agent client is separate. It sends its own conversation, including the
results Hubu returns to it, to whichever model provider it uses.

## Where are my provider keys stored?

In your macOS Keychain. Configuration files hold only the Keychain service and
account names, and Gongbu reads the key when it calls the provider. Agents
never receive provider keys or payment authority.

## What happens if a provider call fails after funds are reserved?

- If the call fails, or the provider confirms nothing was billed, the
  reservation is released.
- If it succeeds, Hubu settles the actual cost and releases the unused part of
  the reservation.
- If the billing result is uncertain, for example after a timeout, the
  reservation is kept and marked for reconciliation. It is never released just
  because a call timed out.
- To recover after an uncertain result, continue with the public handle the
  first call returned, for example by checking `hubu_operation_status`. Do not
  submit the same request again as a new tool call: that creates a new
  operation and can be charged again.

See [failure and reconciliation](spend-lifecycle.md#failure-and-reconciliation-invariants).

## How do I report a problem or contribute?

Run `hubu feedback` or see [send feedback](feedback.md). Report suspected
vulnerabilities privately as described in the
[security policy](../SECURITY.md). If you want to help build Hubu, see the
[roadmap](roadmap.md).
