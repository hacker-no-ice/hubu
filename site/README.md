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

Internal page changes intentionally use native document navigation. The current
vinext client router fails on deployed dynamic documentation routes; the smoke
tests guard this fallback until the upstream Link runtime is safe to restore.

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
to production credentials. Production builds stamp `x-hubustack-revision` with
the source commit, and the workflow verifies that exact revision after deploy.

The `hubustack.dev` GitHub environment must define `CLOUDFLARE_ACCOUNT_ID` and
`CLOUDFLARE_API_TOKEN`. Restrict the token to the target Cloudflare account and
the `hubustack.dev` zone, with Workers Scripts edit, Workers Routes edit, and
Zone read permissions. Never add those values to the repository.

Production routing lives in `wrangler.jsonc`. It uses a Worker route in front of
the existing proxied `hubustack.dev` DNS record, so deployment does not require
a DNS cutover. The `.openai/hosting.json` file is retained only as metadata for
the legacy OpenAI Sites project; the automatic workflow does not publish to it.

The canonical public origin is `https://hubustack.dev`. The original
`hubu-docs.water-no-ice.chatgpt.site` hostname is retained only to redirect
existing links to the same path on the canonical domain. That legacy Sites
deployment remains frozen unless it is published manually.
