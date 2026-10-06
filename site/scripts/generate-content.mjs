import { cp, mkdir, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { Marked, Renderer } from "marked";
import { createSlugger, headingText } from "./heading-ids.mjs";

const siteRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(siteRoot, "..");
const docsRoot = path.join(repoRoot, "docs");
const githubRoot = "https://github.com/hacker-no-ice/hubu/blob/main/";
const siteOrigin = "https://hubustack.dev";

async function markdownPaths(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const nested = await Promise.all(entries.map((entry) => {
    const target = path.join(directory, entry.name);
    return entry.isDirectory() ? markdownPaths(target) : entry.name.endsWith(".md") ? [target] : [];
  }));
  return nested.flat().sort();
}

// Maintainer-only or internal docs. They stay in the repository but are not
// published: no page, search entry, sitemap URL, or navigation item. Because
// they are absent from sourceToSlug, links from published pages to them fall
// back to GitHub blob URLs.
const unpublishedSources = new Set([
  "docs/budget-architecture.md", // budget internals and lock order
  "docs/ledger-accounting.md", // ledger posting internals
  "docs/operations/repository-security.md", // repository/CI hardening for maintainers
  "docs/operations/benchmarking.md", // developer benchmarking
  "docs/operations/gongbu-sandbox.md", // developer manual-test sandbox
  "docs/operations/live-provider-testing.md", // superseded; the site redirects to live-providers
  "docs/operations/publishing-releases.md", // maintainer release publication runbook
]);

const repoRelative = (file) => path.relative(repoRoot, file).split(path.sep).join("/");
const allSourceFiles = await markdownPaths(docsRoot);
for (const excluded of unpublishedSources) {
  if (!allSourceFiles.some((file) => repoRelative(file) === excluded)) {
    throw new Error(`Unpublished docs list names a missing file: ${excluded}`);
  }
}
const sourceFiles = allSourceFiles.filter((file) => !unpublishedSources.has(repoRelative(file)));
const sourceToSlug = new Map(sourceFiles.map((file) => {
  const sourcePath = path.relative(repoRoot, file).split(path.sep).join("/");
  const slug = sourcePath
    .replace(/^docs\//, "")
    .replace(/\/index\.md$/, "")
    .replace(/\.md$/, "");
  return [sourcePath, slug];
}));

function publicHref(slug) {
  return slug.startsWith("configuration/local-stack/v1") ? `/${slug}` : `/docs/${slug}`;
}

function escapeAttribute(value) {
  return value.replaceAll("&", "&amp;").replaceAll('"', "&quot;").replaceAll("<", "&lt;");
}

function plainText(markdown) {
  return markdown
    .replace(/```[\s\S]*?```/g, " ")
    .replace(/`([^`]+)`/g, "$1")
    .replace(/!\[[^\]]*\]\([^)]*\)/g, " ")
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(/[#>*_|~-]/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

// Meta descriptions end on a word boundary instead of mid-word.
function summarize(text, limit = 160) {
  if (text.length <= limit) return text;
  const cut = text.slice(0, limit - 1);
  const boundary = cut.lastIndexOf(" ");
  return `${(boundary > limit / 2 ? cut.slice(0, boundary) : cut).replace(/[\s,;:.]+$/, "")}…`;
}

// Returns the page HTML and its level-2 headings (for "On this page"), both
// from one render pass so table-of-contents links always match heading ids.
function renderMarkdown(markdown, sourcePath) {
  const renderer = new Renderer();
  const slug = createSlugger();
  const headings = [];
  renderer.heading = function ({ tokens, depth }) {
    const inner = this.parser.parseInline(tokens);
    const text = headingText(this.parser, tokens);
    const id = slug(text);
    if (depth === 2) headings.push({ text, id });
    return `<h${depth} id="${id}">${inner}<a class="heading-anchor" href="#${id}" aria-label="Link to ${escapeAttribute(text)}">#</a></h${depth}>`;
  };
  renderer.code = function (token) {
    const block = Renderer.prototype.code.call(this, token);
    return `<div class="code-block">${block}<button class="copy-code" type="button" data-copy-code hidden>Copy</button></div>`;
  };
  renderer.link = function ({ href, title, tokens }) {
    const text = this.parser.parseInline(tokens);
    let resolved = href;
    if (href.startsWith("#")) resolved = href;
    else if (!/^[a-z]+:/i.test(href) && !href.startsWith("//")) {
      const [linkPath, hash = ""] = href.split("#", 2);
      const sourceTarget = path.posix.normalize(path.posix.join(path.posix.dirname(sourcePath), linkPath));
      if (sourceToSlug.has(sourceTarget)) resolved = `${publicHref(sourceToSlug.get(sourceTarget))}${hash ? `#${hash}` : ""}`;
      else if (sourceTarget === "architecture" || sourceTarget === "architecture/index.html") resolved = `/architecture/${hash ? `#${hash}` : ""}`;
      else {
        let targetIsDirectory = false;
        try {
          targetIsDirectory = statSync(path.join(repoRoot, sourceTarget)).isDirectory();
        } catch {
          // Missing repository targets remain blob links so stale links are visible.
        }
        const githubTargetRoot = targetIsDirectory ? githubRoot.replace("/blob/", "/tree/") : githubRoot;
        resolved = `${githubTargetRoot}${sourceTarget}${hash ? `#${hash}` : ""}`;
      }
    }
    return `<a href="${escapeAttribute(resolved)}"${title ? ` title="${escapeAttribute(title)}"` : ""}>${text}</a>`;
  };
  const html = new Marked({ renderer, gfm: true }).parse(markdown);
  return { html, headings };
}

const documents = [];
for (const file of sourceFiles) {
  const sourcePath = path.relative(repoRoot, file).split(path.sep).join("/");
  const markdown = await readFile(file, "utf8");
  const title = markdown.match(/^#\s+(.+)$/m)?.[1]?.replace(/`/g, "") ?? path.basename(file, ".md");
  const body = markdown.replace(/^#\s+.+$/m, "");
  documents.push({
    slug: sourceToSlug.get(sourcePath),
    href: publicHref(sourceToSlug.get(sourcePath)),
    title,
    excerpt: plainText(body).slice(0, 190),
    description: summarize(plainText(body)),
    ...renderMarkdown(markdown, sourcePath),
    sourcePath,
    sourceUrl: `${githubRoot}${sourcePath}`,
  });
}

await writeFile(
  path.join(siteRoot, "app/generated-docs.ts"),
  `// Generated from ../docs/**/*.md. Do not edit.\nexport const documents = ${JSON.stringify(documents)} as const;\n`,
);

const sitemapPaths = ["/", "/architecture/", ...documents.map((document) => document.href)];
await writeFile(
  path.join(siteRoot, "public/sitemap.xml"),
  `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n${sitemapPaths
    .map((pathname) => `  <url><loc>${siteOrigin}${pathname}</loc></url>`)
    .join("\n")}\n</urlset>\n`,
);

const architectureTarget = path.join(siteRoot, "public/architecture");
await rm(architectureTarget, { recursive: true, force: true });
await mkdir(architectureTarget, { recursive: true });
await cp(path.join(siteRoot, "architecture"), architectureTarget, { recursive: true });

const internalArchitectureTarget = path.join(architectureTarget, "internal");
await mkdir(internalArchitectureTarget, { recursive: true });
await cp(path.join(repoRoot, "architecture"), internalArchitectureTarget, { recursive: true });

console.log(`Generated ${documents.length} documentation pages, the sitemap, and both architecture visualizers.`);
