# Provider catalog check

The provider catalog check is a weekly job that keeps Hubu's verified provider
packs current. It compares each model that `packs/verified/*.toml` ships
against the provider's official pages and reports three kinds of change:

- **Price drift:** a published rate differs from the pack's pricing rule.
- **New models:** the provider offers an image model, tier or variant that no
  verified pack covers.
- **Deprecations:** a model we ship has an announced shutdown date or a named
  replacement.

The job runs as a scheduled Claude Code routine on a fresh checkout of `main`.
This page is its instruction set: the routine's prompt only says to follow it,
so changes to the check go through review like code. A maintainer can also run
the same procedure by hand.

The job reads public documentation only. It never makes a live or paid
provider call, never uses provider credentials, and never merges anything.

## Sources

Read these official pages on every run. Prefer them over third-party pricing
sites, resellers and blog posts, which can lag or disagree.

| Provider | Page | What it answers |
| --- | --- | --- |
| Google | [Gemini models](https://ai.google.dev/gemini-api/docs/models) | Image model IDs, stability, newly released models |
| Google | [Gemini pricing](https://ai.google.dev/gemini-api/docs/pricing) | Per-image output prices by resolution, input token prices |
| Google | [Gemini deprecations](https://ai.google.dev/gemini-api/docs/deprecations) | Shutdown dates and recommended replacements |
| Black Forest Labs | [BFL documentation](https://docs.bfl.ai/) and its [index](https://docs.bfl.ml/llms.txt) | Model families, endpoint paths, preview versus stable endpoints |
| Black Forest Labs | [BFL release notes](https://docs.bfl.ai/release-notes) | New models, endpoint changes, deprecations |
| Black Forest Labs | [BFL pricing](https://bfl.ai/pricing) | Per-megapixel and per-tier rates |

If a page can't be fetched, say so in the report and check the rest. Never
treat an unreachable page as "no change".

## Procedure

1. Start from a fresh checkout of `main`.
2. List what Hubu ships. For each file in `packs/verified/`, record its
   `pack_id`, `pack_version`, request path, every `[[models]]` entry with its
   `model`, `pricing_version`, `pricing_reviewed_on` and pricing rules, and its
   image sizes or size presets.
3. Read every source above.
4. Compare and classify each finding:
   - **Price drift:** a published rate for a shipped model differs from the
     pack rule, or a price now depends on something the rule doesn't model
     (for example a new resolution tier). Compare exact amounts. Where a
     provider states both a per-image price and token rates that imply a
     slightly different figure, report both and note which one the pack uses.
   - **New model:** an official, generally available image model, tier or
     variant from Google or BFL that no verified pack covers. Note whether the
     provider calls it stable or preview. Hubu ships stable endpoints only, so
     a preview line is worth noting but not adding. BFL's `flux-2-pro`, for
     example, is a fixed snapshot, and `flux-2-pro-preview` receives updates.
   - **Deprecation:** a shipped model has an announced shutdown date, a named
     replacement, or a "limited access" or legacy label. Record the date and
     the replacement.
   - **Size change:** the provider's published output sizes for a shipped
     model differ from the pack's certified sizes or presets.
5. Write the report (see below) and file the follow-ups.

## Reporting

Every run produces one report, even when nothing changed, so that a run that
silently failed is visible. Prefix every comment, issue and PR body with
`🤖 **Claude:**`.

The report lists:

- the run date and the `main` commit it checked;
- each source page and whether it was read;
- each finding, with its kind, the model, what Hubu ships, what the provider
  publishes, and the source URL;
- "No changes found" when that is the case.

File follow-ups as follows:

- **New models and deprecations:** one issue per finding in the current
  release project in Linear, when a Linear connector is available to the
  routine. Otherwise put them in a GitHub issue titled `Provider catalog
  check YYYY-MM-DD`. Search for an open issue about the same model first and
  comment on it instead of filing a duplicate.
- **Price drift and size changes:** open a pull request against `main` from a
  `catalog-check/YYYY-MM-DD` branch that updates the affected pack. Bump the
  pack's `pack_version`, add a new `pricing_version` and set
  `pricing_reviewed_on` to the run date, and cite the source URLs in a comment
  in the pack and in the PR body. If the change alters a shipped contract, add
  a new contract revision rather than editing the old one, and recompute the
  SHA-256 of `contracts/provider-contracts-v1.json`, which is pinned in
  `crates/gongbu-api/src/provider/provider_contracts.rs` and
  `crates/hubu-cli/src/stack.rs`. See [provider packs](../provider-packs.md)
  for how packs and their contract projections must agree.
  Run whatever checks the environment allows; CI is the gate. If the change
  can't be made safely, file it as an issue instead of a partial PR.
- If neither a PR nor an issue can be created, end the run with the full
  report so it appears in the routine's run log.

Never merge a PR, close an issue, or change a pricing rule without a PR. A
human reviews every change.

## Schedule

The routine runs weekly on Mondays at 17:00 UTC (9:00 Pacific standard time,
10:00 Pacific daylight time). Manage it at
[claude.ai/code/routines](https://claude.ai/code/routines).

## Related

- [Provider packs](../provider-packs.md)
- [Live provider operations](live-providers.md)
- [FLUX provider contract](flux-provider-contract.md)
- [Gemini provider contract](gemini-provider-contract.md)
