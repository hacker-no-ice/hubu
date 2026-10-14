Hubu v0.2.4 is being built and has not shipped yet. This post is a heads-up on the direction: moving budget control from cents to micro-dollars before frequent, sub-cent agent payments make rounding a practical limit on useful work.

AI agents are starting to pay for their own work. Not once a month on an invoice, but call by call: a search, an API request, a page of content. Pay-per-call APIs (for example over x402) often charge $0.001–$0.005 per request, and USDC, the stablecoin many of these payments settle in, has 6 decimals. Agents will make many payments, frequently, and most of them below a cent.

Hubu sits between an agent and its spending. Its job is to let the agent keep working while the owner stays in control: every payment is checked against a policy, reserved against a budget, and settled with a receipt. For that to work, a budget has to mean what it says. If I give an agent $1, it should be able to do $1 of real work, no more and no less.

With cent-based budgets, that isn't quite true.

## Why cent-based budgets fall short

Hubu has always recorded what a provider actually charged as precisely as the provider reported it, up to 18 decimal places. Budgets are the simplification: limits, reservations and balances are still tracked in whole cents.

When an exact cost has to become cents, there are only two options. Rounding down is unsafe: a $0.004 call becomes $0.00, and an agent could make unlimited "free" calls. So Hubu rounds costs up for budget control, preventing sub-cent charges from slipping through as zero.

Rounding up has its own cost. A $0.001 call consumes a full cent of budget, so on a $1 budget the agent stops after 100 calls instead of 1,000, ten times too early. At $0.005 a call it's twice too early. The budget runs out while the real spend is a fraction of it. Neither rounding direction works on its own; the unit itself was the problem.

<figure>
  <img src="/engineering/budgets-that-keep-up-with-agents/02-same-budget-more-work.png" alt="With a $1 budget and a provider price of $0.001 per call, cent-based control permits 100 calls costing $0.10, while micro-dollar control permits 1,000 calls costing $1.00. The provider price stays the same." width="1536" height="1024" loading="lazy" decoding="async">
  <figcaption>At $0.001 per call, micro-dollar control lets a $1 budget fund 1,000 calls. Cent-based control stops after 100.</figcaption>
</figure>

## Exact facts, conservative control

The change I'm building gives two different jobs the precision each needs:

- **Facts stay exact.** Receipts and the ledger keep the provider's charge at up to 18 decimals, as they always have.
- **Control will work in micro-dollars.** Budgets, reservations and policy limits will move from cents to µUSD (6 decimals). Prices expressed to six decimals will fit exactly, matching USDC's precision, which keeps the door open for paying directly on a stablecoin rail.

The new control layer will still round, but only at the boundary: once when an exact cost is converted into budget units, always toward less available spend, and never while adding up. Whatever is left over, under a millionth of a dollar per payment, is recorded in the ledger rather than hidden.

I set this up as a principle early, keeping receipts exact even while budgets were in cents, so this change is a migration of the control layer, not a rewrite of the accounting.

<figure>
  <img src="/engineering/budgets-that-keep-up-with-agents/03-two-precision-layers.png" alt="Provider cost branches into exact facts for receipts and the ledger, kept at up to 18 decimal places, and conservative control for policies, budgets, and reservations at 6 decimal places. The example $0.001234567890123456 becomes $0.001235 in control precision." width="1536" height="1024" loading="lazy" decoding="async">
  <figcaption>Receipts and the ledger retain exact provider charges. Budget control uses six decimal places and rounds conservatively at the boundary.</figcaption>
</figure>

## Why micro-dollars, not 18 decimals everywhere

It's tempting to use the highest precision everywhere. But a signed 64-bit integer at 18 decimals tops out around $9.22. Larger budgets would need a different representation, making storage and arithmetic more complex. At 6 decimals, the same integer supports about $9.22 trillion. Budgets don't need 18 decimals; receipts do.

<figure>
  <img src="/engineering/budgets-that-keep-up-with-agents/04-integer-range.png" alt="A hypothetical signed 64-bit integer representation holds approximately $9.22 at 18 decimal places or $9.22 trillion at 6 decimal places." width="1536" height="1024" loading="lazy" decoding="async">
  <figcaption>With a signed 64-bit integer, six decimal places allow budgets of about $9.22 trillion. At 18 decimal places, the same integer tops out around $9.22.</figcaption>
</figure>

## Changing units without silent failures

Amounts flow through about ten places in Hubu: policies, budgets, reservations, receipts, the ledger, the API, MCP tools, the CLI, and the contract between Hubu and the executors that do the work. The hard part isn't any one of them; it's keeping them in agreement.

If one component still sends cents and another reads micro-dollars, every amount is off by 10,000×. Read one way, an agent can overspend; read the other way, a budget vanishes instantly. Rounding can't catch this, because the wrong number is already a whole number. So every amount will carry its scale explicitly, and the [executor contract](/docs/spend-executor-contract) will get a new version. Components that disagree will reject each other's amounts instead of guessing.

The same care applies to people. A limit typed with too many decimals will be rejected rather than rounded, because silently rounding a cap can block valid calls or loosen the limit. And the CLI will show exact amounts, so real spend never appears as "$0.00".

<figure>
  <img src="/engineering/budgets-that-keep-up-with-agents/05-units-must-agree.png" alt="The integer 100 means $1.00 in cents but $0.000100 in micro-dollars, a 10,000-fold difference. A versioned contract between Hubu and its executor carries the amount and scale and rejects scale mismatches." width="1536" height="1024" loading="lazy" decoding="async">
  <figcaption>The same integer can differ by 10,000× when its unit changes. Explicit scale and a versioned contract prevent silent mismatches.</figcaption>
</figure>

## Migrating money you can't rewrite

Receipts and ledger entries in Hubu are immutable by design, policies are versioned and hashed, and budget updates are idempotent so retries are safe. Rewriting any of them in a new format would break the audit trail or make a safe retry fail.

So the migration is designed to add new fields, verify itself before committing, and leave the old cent values behind as conservative copies: an older version reading the data sees less budget, never more. Payments in USDC against a USD budget will lock their conversion rate in when the money is reserved, so the final charge will use exactly the same rate even if the configuration changes in between.

## Budgets that keep up with agents

When an agent is doing useful work, the budget should reflect what that work actually cost, and stop it exactly where the owner said to, not ten times earlier. I’m building exact micro-dollar budgets for Hubu v0.2.4; the release has not shipped yet. Agents paying in fractions of a cent is just the beginning, and I want Hubu's accounting to be ready for it.
