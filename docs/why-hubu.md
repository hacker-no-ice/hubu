# Why Hubu

Most ways of controlling agent spending cap how much can be spent in total.
Hubu decides whether each individual spend should happen, before it happens,
without the agent ever holding a key.

If one of the simpler approaches below already covers your situation, use it.
Hubu is worth running when you need more than a spending ceiling.

## What Hubu gives you

- **Per-agent identity.** Each agent is
  [registered](agent-registration.md) with a reviewed identity and version, so
  every request is attributed to a specific agent.
- **Policy with human approval.** Every spend request is
  [evaluated](policy-engine.md) and is allowed, denied, or held until a human
  decides. Holding for approval is the recommended default.
- **Funds reserved before execution.** An allowed request
  [reserves](spend-lifecycle.md) its maximum cost before any provider work
  starts. The actual cost is settled afterwards and any unused amount is
  released, so agents running at the same time cannot overshoot the budget.
- **One ledger across providers.** Decisions and money movement are recorded in
  one [ledger](ledger-history.md), whichever provider did the work.
- **Agents never hold keys.** Provider keys stay with
  [Gongbu](gongbu-execution.md), read from your macOS Keychain. Payment
  authority stays with Hubu. The agent only submits structured requests.

## How it compares

| Approach | Good at | Does not cover | Better choice when |
| --- | --- | --- | --- |
| Provider spend limits and billing alerts | Nothing to install or run | Limits apply to a whole account or project, not to each agent. Alerts and limits act after the money is spent. Every provider works differently. | You run one agent against one provider. |
| API gateways with per-key budgets, such as LiteLLM or OpenRouter | Mature, support many models, and offer hosted options | The agent still holds a usable key. There is no step that pauses a request for a human decision. Spend is usually counted after each call returns, so parallel requests can overshoot. | You need many models, or your team already runs a gateway. |
| Prepaid credits or capped virtual cards | Strictly limit how much you can lose | No policy on what the money is spent on and no approval step. The record shows charges, not which agent asked for what. | You are comfortable losing the prepaid amount. |
| Agent payment toolkits, such as Stripe's agent toolkit | Actually moving money | They do not decide whether a spend should happen; that is the problem Hubu solves, so the two can work together. | You need real payments today. Hubu has no production payment rail yet. |
| Building it yourself with a policy engine and a database | Full control | Reservations, settlement, retries, and crash recovery under concurrent requests are the hard part. The [executor conformance suite](executor-conformance.md) lists the edge cases Hubu covers. | Your requirements are unusual enough that no shared tool fits. |

## Where Hubu is not the right fit today

- Hubu is experimental and local-first. It is not money-grade financial
  infrastructure, and direct payments are a mock.
- It runs on macOS only, guided setup exists only for Codex, and the supported
  live providers are Gemini and FLUX.2 Pro.
- You run the stack yourself. There is no hosted service.

See the [FAQ](faq.md) for details and the [roadmap](roadmap.md) for what comes
next.
