import { cp, mkdir, readdir, readFile, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { Marked, Renderer } from "marked";

const siteRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(siteRoot, "..");
const docsRoot = path.join(repoRoot, "docs");
const githubRoot = "https://github.com/hacker-no-ice/hubu/blob/main/";

async function markdownPaths(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const nested = await Promise.all(entries.map((entry) => {
    const target = path.join(directory, entry.name);
    return entry.isDirectory() ? markdownPaths(target) : entry.name.endsWith(".md") ? [target] : [];
  }));
  return nested.flat().sort();
}

const sourceFiles = await markdownPaths(docsRoot);
const sourceToSlug = new Map(sourceFiles.map((file) => {
  const sourcePath = path.relative(repoRoot, file).split(path.sep).join("/");
  const slug = sourcePath.replace(/^docs\//, "").replace(/\.md$/, "");
  return [sourcePath, slug];
}));

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

function headingId(text) {
  return plainText(text).toLowerCase().replace(/[^a-z0-9\u4e00-\u9fff]+/g, "-").replace(/^-|-$/g, "") || "section";
}

function renderMarkdown(markdown, sourcePath) {
  const renderer = new Renderer();
  const usedIds = new Map();
  renderer.heading = function ({ tokens, depth }) {
    const inner = this.parser.parseInline(tokens);
    const base = headingId(inner);
    const count = usedIds.get(base) ?? 0;
    usedIds.set(base, count + 1);
    const id = count ? `${base}-${count + 1}` : base;
    return `<h${depth} id="${id}">${inner}<a class="heading-anchor" href="#${id}" aria-label="Link to ${escapeAttribute(plainText(inner))}">#</a></h${depth}>`;
  };
  renderer.link = function ({ href, title, tokens }) {
    const text = this.parser.parseInline(tokens);
    let resolved = href;
    if (href.startsWith("#")) resolved = href;
    else if (!/^[a-z]+:/i.test(href) && !href.startsWith("//")) {
      const [linkPath, hash = ""] = href.split("#", 2);
      const sourceTarget = path.posix.normalize(path.posix.join(path.posix.dirname(sourcePath), linkPath));
      if (sourceToSlug.has(sourceTarget)) resolved = `/docs/${sourceToSlug.get(sourceTarget)}${hash ? `#${hash}` : ""}`;
      else if (sourceTarget === "architecture" || sourceTarget === "architecture/index.html") resolved = `/architecture/${hash ? `#${hash}` : ""}`;
      else resolved = `${githubRoot}${sourceTarget}${hash ? `#${hash}` : ""}`;
    }
    return `<a href="${escapeAttribute(resolved)}"${title ? ` title="${escapeAttribute(title)}"` : ""}>${text}</a>`;
  };
  return new Marked({ renderer, gfm: true }).parse(markdown);
}

const documents = [];
for (const file of sourceFiles) {
  const sourcePath = path.relative(repoRoot, file).split(path.sep).join("/");
  const markdown = await readFile(file, "utf8");
  const title = markdown.match(/^#\s+(.+)$/m)?.[1]?.replace(/`/g, "") ?? path.basename(file, ".md");
  const body = markdown.replace(/^#\s+.+$/m, "");
  documents.push({
    slug: sourceToSlug.get(sourcePath),
    title,
    excerpt: plainText(body).slice(0, 190),
    html: renderMarkdown(markdown, sourcePath),
    headings: [...markdown.matchAll(/^##\s+(.+)$/gm)].map((match) => ({ text: plainText(match[1]), id: headingId(match[1]) })),
    sourcePath,
    sourceUrl: `${githubRoot}${sourcePath}`,
  });
}

await writeFile(
  path.join(siteRoot, "app/generated-docs.ts"),
  `// Generated from ../docs/**/*.md. Do not edit.\nexport const documents = ${JSON.stringify(documents)} as const;\n`,
);

const architectureTarget = path.join(siteRoot, "public/architecture");
await rm(architectureTarget, { recursive: true, force: true });
await mkdir(architectureTarget, { recursive: true });
await cp(path.join(repoRoot, "architecture"), architectureTarget, { recursive: true });

console.log(`Generated ${documents.length} documentation pages and synced the architecture visualizer.`);
