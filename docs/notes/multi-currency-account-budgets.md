# Multi-Currency Account Budgets

This note captures a future direction for Hubu once agent accounts can represent
different currencies, rails, or spending settings. It is not the current
technical contract. Today the local spend path is effectively USD-only.

## Identity And Spend Source

Keep the nouns separate:

- `agent_id` is the durable identity. It is the right anchor for registration,
  ownership, reputation, policy attachment, and audit subject.
- `account_id` is the spend source. It is the right anchor for spend
  authorization, payment rails, currency, limits, and account-level settings.

The current account-id-only spend change points the CLI, HTTP API, MCP schema,
and executor validation at the spend source instead of the identity. That is the
right default if one long-lived agent later owns multiple accounts.

## Budget Shape

Budgets should stay single-currency. A budget with mixed USD and EUR capacity
would need exchange rates, valuation timestamps, rounding rules, and a reporting
currency before "remaining budget" has a precise meaning.

For an agent with multiple currency accounts, prefer multiple budgets:

```text
agent: agt_writer
  account: aga_usd_main
    currency: USD
    budget: 500 USD / month
  account: aga_eur_main
    currency: EUR
    budget: 300 EUR / month
```

The likely future ownership field is an account ID, for example:

```text
Budget.agent_account_id = account_id
```

This is a little more precise than `AgentCurrency(agent_id, currency)` because
accounts may differ by more than currency: rail, sandbox versus production,
project, approval profile, or risk tier.

## Policy Shape

Policy can remain broader than budget. A user-level or agent-level policy can
evaluate structured spend intent across accounts and currencies, because policy
decides whether a request is allowed while budgets decide whether capacity is
available.

One policy can contain currency-aware rules:

```text
allow USD spend up to 50 USD
allow EUR spend up to 40 EUR
deny blocked merchants
needs_approval above those thresholds
```

Later, Hubu may also support account-scoped policy overrides, but that should be
an additional override layer rather than a requirement for basic multi-currency
spend.

## Suggested Invariants

Future multi-currency spend should keep these checks crisp:

```text
spend.account_id -> account.currency
spend.currency == account.currency
budget.account_id == spend.account_id
budget.currency == spend.currency
holds reserve only inside that account/currency budget
policy may inspect agent_id, account_id, currency, merchant, amount, and task
```

The server should derive `agent_id` from `account_id` for policy lookup and audit
instead of trusting callers to provide both. If a request includes both in a
future API shape, mismatches should be rejected before policy or budget checks.

## Migration Sketch

1. Add account currency to registration/account records.
2. Include request currency in MCP, CLI, HTTP, and executor guidance once more
   than USD is supported.
3. Add account-owned budget creation and lookup.
4. Keep existing agent-owned budgets as the USD-only default until an explicit
   migration maps them to the agent's primary account.
5. Add policy examples for currency-aware limits.

## Open Questions

- Should advisory user spending targets remain per-currency, or should Hubu add
  an optional reporting-currency target with explicit FX rules?
- Should account-scoped policy overrides exist in v1 multi-currency, or are
  user/agent policies with account conditions enough?
- How should ledger views group balances when one agent owns accounts in
  several currencies?
