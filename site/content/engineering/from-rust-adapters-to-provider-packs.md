*Adding a provider to Hubu used to mean writing Rust. Now it means writing a TOML file.*

Gongbu is the part of Hubu that actually spends money for an agent: it calls the paid API (FLUX, Gemini, and soon paid search) after Hubu approves the request. Until v0.2.4, every provider Gongbu could call was a hand-written Rust adapter. In v0.2.5 I replaced those adapters with **provider packs**: one TOML file per integration, run by a small set of reviewed engines.

The short version:

- **Two adapters deleted.** The FLUX adapter (3,681 lines) and the Gemini adapter (740 lines) are gone. Both now run from verified packs, with the same contract IDs, prices and behavior.
- **New models are mostly data.** Since the migration, FLUX.2 Klein, Flex and Max, FLUX 3 Image and Klein, Gemini 3 Pro Image and Nano Banana 2.1 all landed as packs. Gongbu now ships 13 verified packs.
- **Safety moved into the engines.** A pack says *what* a provider is. The engine decides *how* to call it safely, and a pack can't override that.
- **It sets up the next step.** If a provider is data, adding one can eventually move out of releases and into a web UI.

## Why code adapters stopped scaling

I started with hand-written Rust adapters on purpose, knowing their limits. Early on, the only question was whether governed spend worked at all, and one adapter per provider was the fastest way to find out. Now the focus is on making Hubu useful in real workflows, which means many more models, configs and providers. That made this the right time to invest in the refactor.

Each adapter was a Rust module that did everything for one provider: build the request, send it, poll if needed, parse the response, find the images, and price the call. The FLUX adapter alone grew to 3,681 lines.

As the number of integrations grew, that had three costs.

1. **Every change was a code change.** A new model, a new resolution tier or a price update meant editing Rust, passing review and cutting a release. Image providers ship new models and prices often, so this never stopped.
2. **Every adapter was its own safety surface.** Hubu's guarantees live in the call path: never retry a paid mutation, send ambiguous outcomes to reconciliation, settle only from the price frozen at approval, send credentials only to allowed hosts. Each adapter had to get all of these right again, and each new one was a new place to get them wrong.
3. **Most of the code was the same.** Strip away the field names and almost every image provider is one of two shapes: one request returns the result, or you submit a job and poll for it. I was rewriting the same two state machines with different JSON paths.

## The design: packs declare *what*, engines own *how*

A provider pack is one TOML file. It declares the endpoint and allowed hosts, which credential alias to use, the parameters an agent may set (with types and bounds), how to build the request body, where to find the request ID, usage and images in the response, and the pricing rules for each model.

What it can't declare is anything about *how a paid call is made safely*. That belongs to the engine.

<figure>
  <img src="/blogs/from-rust-adapters-to-provider-packs/02-before-after.png" alt="Gongbu before and after provider packs. Before, in v0.2.4: after Hubu approves the spend, Gongbu holds a FLUX adapter of 3,681 lines of Rust and a Gemini adapter of 740 lines, each doing its own request, polling, parsing, pricing and safety, and calling the BFL and Google Gemini APIs. A new provider needs a new adapter and a release. After, in v0.2.5: Gongbu holds provider packs as TOML data, 13 verified, which feed two reviewed Rust engines, sync_fixed and async_poll, that own retries, settlement and credentials and call the same APIs. A new model or provider is a new pack." width="1520" height="944" loading="lazy" decoding="async">
  <figcaption>Two reviewed engines replace one Rust adapter per provider. The packs carry the provider details; the engines carry the safety rules.</figcaption>
</figure>

| Owned by the engine | Declared by the pack |
| --- | --- |
| Zero retries on paid mutations; ambiguous outcomes go to reconciliation | Endpoint, API version, allowed HTTPS hosts |
| Durable checkpoints and read-only resume of polling | Auth header and credential alias |
| Settling only from the frozen pricing snapshot | Agent parameters with types and bounds |
| Credentials sent only to allowed hosts | Pricing rules and review date |
| Artifact media type, size and host checks | Response paths for request ID, usage, cost, artifacts |

Engines are grouped by **archetype**, the shape of the provider's API:

| Archetype | Hold | Settle | Fits |
| --- | --- | --- | --- |
| `sync_fixed` | Exact price from the request | Same frozen price | One request returns the result (Gemini images, search APIs) |
| `async_poll` | Frozen price × units | Frozen price + durable job ID | Submit a job, then poll (FLUX) |
| `metered` | Worst case, e.g. max output tokens × rate | Reported usage, never above the hold | Model calls billed by usage |

`sync_fixed` and `async_poll` are built and run every shipped contract. `metered` is defined in the schema for token-billed calls.

Here is the heart of the FLUX 3 Image pack, trimmed:

```toml
pack_id = "bfl-flux-3-image"
archetype = "async_poll"

[transport]
endpoint = "https://api.bfl.ai"
allowed_hosts = ["api.bfl.ai", "api.*.bfl.ai"]
auth_header = "x-key"
credential = "bfl_flux3_image"

[request]
path = "/v1/flux-3-image"

[request.body]
prompt = "{{prompt}}"
resolution = "{{image_size}}"
aspect_ratio = "{{aspect_ratio}}"
grounding = false

[[parameters]]
name = "image_size"
type = "enum"
values = ["1k", "2k"]
required = true
selector = "image_size"

[poll]
job_id = "/id"
path = "/v1/get_result"
query = { id = "{{job_id}}" }
status = "/status"
pending = ["pending", "reasoning", "generating"]
ready = ["ready", "succeeded", "completed"]

# USD 0.048 per 1k image, USD 0.10 per 2k image
[[models.pricing_rules]]
selector = { image_size = "1k" }
components = [{ unit = "image", rate_numerator_minor = 48, rate_denominator = 10 }]
```

There is no code here. The `async_poll` engine reads this file, submits once, checkpoints the job, polls under a fixed deadline, and settles from the price frozen when Hubu approved the spend. The full format is in the [provider pack schema](/docs/provider-packs).

## Data, but not a loophole

Moving logic into config files usually trades safety for flexibility. For a system that spends money, that trade is unacceptable, so the schema is strict on purpose. Gongbu validates every pack before it can run: it rejects any field that touches engine-owned behavior like retries or settlement, requires every priced input to be bounded and priced exactly once, keeps credentials on declared hosts, and pins each pack by digest so an edit never changes work already in flight.

The design goal: a mistake in a pack can make a provider call fail, but it can't make Gongbu retry a paid call, overspend a hold, or leak a key.

Still, moving to data does add exposure: the pack data itself is now something to attack, and I don't want to pretend otherwise. One of the Break Hubu claims is that packs must not become a way for an agent to work around Hubu's controls. That claim is not fully proven yet, and I will keep investing in challenges against it. I still think the trade is worth it. In the long run, packs are far more extensible than hand-written adapters, and guarding the data is a solvable problem.

## The migration

I built the schema and both engines first, proved them on sandbox fixtures, then moved Gemini and FLUX over one at a time. The bar was strict: same contract IDs, same prices, same behavior. Only the target settings changed, to name the pack and its digest, and a test fails if a contract and its pack ever disagree. Once both providers ran from packs, I deleted their adapters.

<figure>
  <img src="/blogs/from-rust-adapters-to-provider-packs/03-migration-timeline.png" alt="Timeline of the pack migration in October 2026. Oct 19: pack schema v1 and the sync_fixed engine. Oct 20, highlighted: the async_poll engine, Gemini and FLUX migrated, Rust adapters deleted. Together these two days are the switch. Oct 21: FLUX 3 Image, FLUX.2 Pro v2 and Gemini image editing. A shaded gap marks a trip to Austin for the USGP weekend. Oct 28: FLUX.2 Klein, Flex and Max, Gemini 3 Pro Image and Nano Banana 2.1. Oct 29: FLUX 3 Image Klein as one 142-line pack. From Oct 21 on, new models arrive as data with no new Rust adapters." width="1520" height="648" loading="lazy" decoding="async">
  <figcaption>Two days to switch, then every new model shipped as a pack, with a USGP weekend in between.</figcaption>
</figure>

The next day, FLUX 3 Image, FLUX.2 Pro v2 and Gemini image editing landed as the first new packs. Then I flew to Austin for the USGP weekend and didn't touch the code for a few days. When I got back, the payoff showed up fast. FLUX.2 Klein, Flex and Max, Gemini 3 Pro Image and Nano Banana 2.1 all landed as packs in one day, and FLUX 3 Image Klein the day after. Klein was just a 142-line TOML file plus a contract entry and tests: no polling code, no pricing code, no new Rust adapter. The pack even carries its own notes for a reviewer, like which resolution tier is held back and when the prices were last checked. Honestly, that felt a lot nicer than writing yet another adapter.

Two things to know if you run Hubu:

- **Verified packs still ship with the binary.** Today they are compiled into Gongbu and reviewed like code. That is deliberate: verified pricing should go through review. What changed is what the review covers.
- **The upgrade needs a drain.** There is no digest continuity between an old adapter target and its new pack target, so Gongbu refuses to start while an unfinished execution points at a target revision that no longer exists. The [upgrade guide](/docs/operations/upgrading-to-v0.2.5) says to let work finish or reconcile first.

## Keeping verified prices honest

A verified pack is only as good as its prices, and providers change them without telling me. Because prices now live in one data file per provider, checking them is something I can automate.

Every Monday, a scheduled Claude Code routine runs a [catalog check](/docs/operations/catalog-check) on a fresh checkout of `main`. It reads every verified pack, then reads Google's and BFL's official model, pricing and deprecation pages, and looks for four things:

- **Price drift:** a published rate no longer matches the pack's pricing rule.
- **New models:** a stable model, tier or variant that no pack covers yet.
- **Deprecations:** a shipped model with a shutdown date or a named replacement.
- **Size changes:** published output sizes that differ from what the pack certifies.

Price drift comes back as a pull request that updates the pack, bumps its `pricing_version`, sets the new review date and cites the source page. New models and deprecations become issues. The routine only reads public pages: no provider credentials, no paid calls, and it never merges anything. I review every change, the same way I'd review code.

<figure>
  <img src="/blogs/from-rust-adapters-to-provider-packs/04-catalog-check.png" alt="Flow of the weekly catalog check. The verified packs, 13 TOML files with pricing rules, and the official provider pages from Google and BFL covering models, prices and shutdowns both feed the weekly catalog check, a Claude Code routine that runs on Mondays, reads public pages only and never merges. It produces a pull request for price drift or size changes, an issue for a new model or deprecation, and a report on every run, even with no change. Pull requests and issues go to human review, and merged changes flow back into the verified packs." width="1520" height="880" loading="lazy" decoding="async">
  <figcaption>A weekly check turns price drift into a reviewed pull request. Nothing changes a price without review.</figcaption>
</figure>

It writes a report even when nothing changed, so a run that quietly broke is easy to spot. That already paid off once: BFL's pricing table is filled in by JavaScript, so the first runs saw empty cells and couldn't verify any FLUX price. The fix was to read the prices from structured data that the page also embeds, and to report a model as "not verifiable" instead of "unchanged" when that data is missing.

## What's next: add your own provider from a web UI

The real reason to turn providers into data is where it can go next. Today a new pack still ships in a release, but because it's data, it doesn't have to stay that way.

<figure>
  <img src="/blogs/from-rust-adapters-to-provider-packs/05-roadmap.png" alt="Roadmap in four stages. Done in v0.2.4: Rust adapters, one per provider, needing code and a release. Shipping in v0.2.5, highlighted: provider packs, TOML data run by two engines, with 13 verified packs. Next, as a vision: a web UI to link an account or add your own pack. Maybe, not committed: a provider catalog database serving packs." width="1520" height="416" loading="lazy" decoding="async">
  <figcaption>From code to data, and next to a UI. The catalog database is an idea, not a commitment.</figcaption>
</figure>

**Verified providers.** As the catalog of verified packs grows, most people will switch one on only when they already have an account there. Click **Add provider**, pick BFL, link your account, and your agents can spend on FLUX right away, under Hubu's policy and budgets.

**Your own providers.** For anything not in the catalog, you'll write your own pack in the same UI, ideally with a link to the provider's price page so the drift check can watch it too. It runs on the same engines and validator. The only difference is that you, not Hubu, vouch for its prices.

Either way, no CLI and no Rust. That is the bet behind provider packs: keep moving money in a small amount of reviewed code, and make everything else data that is validated, pinned by digest, and never holds a secret. If you have a paid API you want your agents to use under Hubu, start with the [pack schema docs](/docs/provider-packs), and tell me which providers you need next.
