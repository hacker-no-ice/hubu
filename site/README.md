# Hubu documentation site

This directory contains the public Hubu documentation website. The site is a
vinext application deployed directly to Cloudflare Workers at
`https://hubustack.dev`.

## Canonical content

The repository's `docs/**/*.md` files are the canonical website documentation;
`docs/overview.md` supplies the website overview independently of the
repository's concise top-level `README.md`. The reader-focused dark architecture
guide lives in `site/architecture/`; the repository's detailed engineering
explorer remains canonical under top-level `architecture/`.

`npm run content` generates a temporary TypeScript content index, publishes the
reader guide at `/architecture/`, and copies the engineering explorer to
`/architecture/internal/`. Generated public copies are ignored by Git and must
not be edited directly.

To add a page, add its Markdown file under `docs/`, then place it in the curated
navigation in `app/lib/docs.ts`. The page is searchable automatically after the
next build. Relative Markdown links are translated to site routes; links to
repository source files remain GitHub links.

Demo companion scripts live under `docs/demos/`: `index.md` is served at
`/demos`, and individual scripts at `/demos/<slug>`. They use the same content,
search, navigation, and metadata pipeline as documentation. Keep each script
concise and aligned with its recording, with the video before the script.

Internal page changes intentionally use native document navigation. The current
vinext client router fails on deployed dynamic documentation routes; the smoke
tests guard this fallback until the upstream Link runtime is safe to restore.

## Engineering Blogs

The Engineering Blogs index lives at `/blogs`, with articles at
`/blogs/<slug>`. Add each post's metadata to
`content/engineering/posts.json` and its body to
`content/engineering/<slug>.md`. The metadata file contains an array of objects:

```json
{
  "slug": "budgets-that-keep-up-with-agents",
  "title": "Budgets that keep up with agents",
  "description": "A preview of the accounting precision work underway for Hubu v0.2.4.",
  "publishedOn": "2026-10-14",
  "releaseStatus": "Hubu v0.2.4 · In development",
  "cover": {
    "src": "/blogs/budgets-that-keep-up-with-agents/01-cover.png",
    "width": 1672,
    "height": 941,
    "alt": "A budget moves from coarse cent divisions to precise micro-dollar divisions."
  }
}
```

All fields shown above except `releaseStatus` are required. Omit `releaseStatus`
for posts that are not tied to a release. Use a unique lowercase, hyphen-separated
slug and an explicit, valid calendar date in `YYYY-MM-DD` form. The publication
date appears on the index and article and supplies the article's share metadata;
it stays the same across rebuilds and time zones. Preserve the original date
when editing an article or updating its release status. Adding a metadata entry
includes the post in the next build and sitemap; `publishedOn` is a display date,
not a scheduled publication mechanism.

The page supplies its title as the only H1, so begin the Markdown body with prose
and use H2 headings for sections. Put artwork in
`public/blogs/<slug>/`, use root-relative `/blogs/<slug>/...` image
URLs, and provide descriptive alt text, dimensions, and captions. Inline figures
can use HTML `<figure>`, `<img>`, and `<figcaption>` elements. Keep release status
explicit when describing work that has not shipped.

Use root-relative site URLs for links to published pages, such as
`[Overview](/docs/overview)`. Other relative links resolve to repository source.

`npm run content` also generates `app/generated-engineering.ts`; this file is
ignored by Git and must not be edited directly. Posts appear newest first by
their explicit publication date.

## Local development

From this directory:

```sh
npm install
npm run dev
```

Run `npm test` for a production build and server-render smoke tests. Run
`npm run lint` for source linting.

## Private visitor analytics

The first-party dashboard lives at `https://hubustack.dev/_analytics`. It shows
estimated unique browsers, visits, page views, new and returning browsers,
daily counts, landing referrers, popular pages, countries, and rejected bot
beacons. Period uniques are distinct browser identifiers across the entire
selected period, not a sum of daily uniques. Ranges are 1, 7, or 30 UTC calendar
days including today. A visit is a browser session with a 30-minute idle expiry;
separate tabs can have separate sessions.

The public beacon is `/_telemetry/beacon.js`, and the same-origin collector is
`POST /_telemetry/collect`. Keeping these outside `/_analytics` lets Cloudflare
Access protect the entire private dashboard path without blocking collection.
Both the React pages and the two static architecture guides load the beacon.
The dashboard itself is never tracked. Nothing is collected until deployment
configuration explicitly enables analytics.

Collection uses a random, server-issued, Secure, HttpOnly, first-party
`__Host-hubu_visitor` cookie with a one-year lifetime. Only its SHA-256 hash is
stored in a dedicated D1 database. IP addresses and user-agent strings are not
written to that database, queries and fragments are stripped, and only external
referrer hostnames are retained. Page-view events expire after 90 days. Inactive
visitor identifiers expire after 90 days too; active visitors keep their
first-seen history. “Returning” means first seen before the selected period in
this retained history. Data collection starts after activation; the previous
Cloudflare totals cannot reconstruct these identities.

Known crawler user agents, browser automation, Cloudflare verified bots, and low
Cloudflare bot scores when available are excluded. Fine-grained bot scores are
not available on every Cloudflare plan. This is a best-effort estimate of human
browser traffic, not a guarantee of unique people or perfect bot detection.
Rejected beacons are stored only as anonymous daily reason counts, and do not
include requests blocked by Cloudflare before reaching the collector. A
Cloudflare rate-limit binding limits collector requests per hashed IP key.

The collector honors Do Not Track, Global Privacy Control, and the
`hubu_analytics_exclude=1` cookie. Visitors can set that preference at `/privacy`;
the dashboard has the same control for excluding your own browser. This opt-out
controls this first-party dashboard, independently of Cloudflare's existing
Web Analytics service.

### One-time production setup

1. Create a dedicated D1 database with `npx wrangler d1 create hubustack-analytics`
   from `site/`, and record the returned database ID. Do not reuse a Hubu or
   Gongbu database.
2. Create a Cloudflare Access self-hosted application for
   `hubustack.dev/_analytics`. Use an identity-based Allow policy limited to the
   dashboard owner, and copy its application audience (AUD) and your bare team
   domain, such as `example.cloudflareaccess.com`. Do not protect `/_telemetry`.
   The Worker verifies the JWT signature, issuer, audience, expiry, and human
   subject on both dashboard and report requests; a spoofed identity header,
   Bypass policy, service token, or missing configuration does not grant access.
3. Add these **variables** to the GitHub `hubustack.dev` environment:
   `HUBUSTACK_ANALYTICS_DB_ID`, `HUBUSTACK_ANALYTICS_ACCESS_TEAM_DOMAIN`, and
   `HUBUSTACK_ANALYTICS_ACCESS_AUD`. These are configuration identifiers, not
   authentication secrets. The existing `CLOUDFLARE_API_TOKEN` also needs D1 Edit
   permission for this account's analytics database.
4. Merge the reviewed PR or run the existing Docs site workflow on `main`.
   It prepares the built Wrangler configuration, applies the D1 migration, and
   deploys. Open `/_analytics` through Access, then exclude your own browser
   before browsing public pages. Confirm a fresh browser visit appears once and
   multiple pages increase views without increasing period uniques.

With all three analytics variables absent, the standard deployment keeps the
public site working and removes the production D1 binding and analytics cron.
Partial configuration fails before migration/deployment. All three present
activate collection and a daily UTC retention job. To pause collection, remove
all three variables and redeploy; the existing database is preserved. Manual
deployments must use `npm run deploy`, which prepares the generated bundle, and
the same variables. Run `npm run analytics:migrate` before deploying a newly
created database. Avoid invoking `wrangler deploy` directly, since it bypasses
this configuration preparation. The source `wrangler.jsonc` intentionally has
no production database ID; deployments pin the configured database and do not
silently provision a replacement.

### Local verification

Run `npm run cf:typegen` after changing bindings, and `npm run typecheck` for the
application and Worker types. `npm test` includes real SQLite aggregation tests,
JWT signature/claims tests, beacon navigation/privacy tests, and dashboard
rendering tests. Local databases use Wrangler's ignored project-local state.
Apply the schema with `npx wrangler d1 migrations apply ANALYTICS_DB --local`.

For local browser collection, put `ANALYTICS_ENABLED=true` and
`ANALYTICS_ORIGIN=http://localhost:5173` in ignored `site/.dev.vars`, then run
`npm run dev`. The configured origin must match the actual dev-server port.
Only explicit loopback HTTP development is accepted; production uses HTTPS.
The dashboard remains closed unless Access JWT validation is configured; the
offline auth tests verify its success and rejection paths without a production
login or auth bypass. Never add a public demo mode to the production dashboard.

Website analytics is separate from Hubu's governance plane and Gongbu's
execution plane. Their architecture diagrams intentionally remain focused on
those product boundaries; this website-only storage and request flow are
documented here instead.

## Deployment

Merges to `main` that change `site/**`, `docs/**`, or `architecture/**` are
validated and deployed by `.github/workflows/docs-site.yml`. The workflow builds
the Worker and static assets with vinext, then publishes the generated bundle
with the repository-locked Wrangler version. Pull requests that affect the site
run the same lint, build, smoke-test, and Wrangler dry-run checks without access
to production credentials. Production builds publish the exact source revision
at `/.well-known/hubustack-revision`, and the workflow verifies that value after
deploy. No static asset exists at that path, so Cloudflare passes the request to
the Worker while public static assets retain the asset-first serving path. The
workflow polls through eventual edge propagation rather than accepting an older
successful response.

The `hubustack.dev` GitHub environment must define `CLOUDFLARE_ACCOUNT_ID` and
`CLOUDFLARE_API_TOKEN`. Restrict the token to the target Cloudflare account and
the `hubustack.dev` zone, with Workers Scripts edit, Workers Routes edit, and
Zone read permissions. Never add those values to the repository.

Production routing lives in `wrangler.jsonc`. The `hubustack.dev` Worker Custom
Domain makes the deployed Worker the canonical origin. The one-time migration
requires detaching the hostname from OpenAI Sites and removing its existing DNS
record before the first Custom Domain deployment. The `.openai/hosting.json`
file is retained only as metadata for the legacy OpenAI Sites project; the
automatic workflow does not publish to it.

The canonical public origin is `https://hubustack.dev`. The original
`hubu-docs.water-no-ice.chatgpt.site` hostname is retained only to redirect
existing links to the same path on the canonical domain. That legacy Sites
deployment remains frozen unless it is published manually.
