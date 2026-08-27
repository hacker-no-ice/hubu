# Hubu documentation site

This directory contains the public Hubu documentation website. The site is a
vinext application deployed with OpenAI Sites.

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

Deployment metadata lives in `.openai/hosting.json`. Build and publish through
OpenAI Sites so the Cloudflare Worker-compatible output and static assets are
packaged together. Do not add secrets to this repository; hosted runtime values
belong in Sites.

The canonical public origin is `https://hubustack.dev`. The original
`hubu-docs.water-no-ice.chatgpt.site` hostname is retained only to redirect
existing links to the same path on the canonical domain.
