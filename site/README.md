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
