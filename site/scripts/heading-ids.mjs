// GitHub-compatible heading ids. Docs are written and read on GitHub, so site
// ids must match GitHub's for in-repo #anchor links to work on the site.

function decodeEntities(text) {
  return text
    .replace(/&quot;/g, '"')
    .replace(/&#0?39;/g, "'")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&amp;/g, "&");
}

// github-slugger's slug: lowercase, keep letters, marks, numbers, connector
// punctuation, spaces and hyphens, then turn spaces into hyphens.
export function githubSlug(text) {
  return text.toLowerCase().replace(/[^\p{L}\p{M}\p{N}\p{Pc}\- ]/gu, "").replace(/ /g, "-");
}

// One slugger per page. Like github-slugger, a repeated slug gets -1, -2, ...,
// skipping any suffix already taken (for example by a literal "Foo-1" heading).
export function createSlugger() {
  const occurrences = new Map();
  return (text) => {
    const original = githubSlug(text) || "section";
    let slug = original;
    while (occurrences.has(slug)) {
      const next = occurrences.get(original) + 1;
      occurrences.set(original, next);
      slug = `${original}-${next}`;
    }
    occurrences.set(slug, 0);
    return slug;
  };
}

// Plain heading text as GitHub renders it, using the active parser's own text
// renderer so nested inline markup (emphasis, links, code) is handled.
export function headingText(parser, tokens) {
  return decodeEntities(parser.parseInline(tokens, parser.textRenderer)).trim();
}
