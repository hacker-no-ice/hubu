import { Search } from "./Search";
import { adjacentDocuments, navGroups, searchDocuments, type Doc } from "../lib/docs";

function Navigation({ current }: { current: string }) {
  return (
    <nav className="docs-nav" aria-label="Documentation">
      {navGroups.map((group) => (
        <section key={group.label}>
          <h2>{group.label}</h2>
          {group.items.map(([label, slug]) => (
            <a aria-current={slug === current ? "page" : undefined} href={getDocumentHref(slug)} key={slug}>{label}</a>
          ))}
        </section>
      ))}
      <section><h2>Visual guide</h2><a href="/architecture/">Interactive architecture <span>↗</span></a></section>
    </nav>
  );
}

function getDocumentHref(slug: string) {
  return searchDocuments.find((document) => document.slug === slug)?.href ?? `/docs/${slug}`;
}

export function DocsShell({ document }: { document: Doc }) {
  const { previous, next } = adjacentDocuments(document.slug);
  return (
    <div className="docs-shell">
      <header className="docs-header">
        <a className="brand" href="/" aria-label="Hubu documentation home"><span className="brand-seal" aria-hidden="true">户</span><span>Hubu <i>/ docs</i></span></a>
        <Search documents={searchDocuments} />
        <a className="github-compact" href="https://github.com/hacker-no-ice/hubu">GitHub ↗</a>
      </header>
      <details className="mobile-nav"><summary>Browse documentation</summary><Navigation current={document.slug} /></details>
      <aside className="sidebar"><Navigation current={document.slug} /></aside>
      <main className="doc-main" id="main-content">
        <div className="doc-status"><span>EXPERIMENTAL · LOCAL-FIRST</span><p>Evaluate carefully. Live-provider paths are experimental and are not money-grade production infrastructure.</p></div>
        <article className="markdown-body" dangerouslySetInnerHTML={{ __html: document.html }} />
        <div className="source-row"><a href={document.sourceUrl}>Edit this page on GitHub ↗</a><span>Canonical source: {document.sourcePath}</span></div>
        <nav className="doc-pagination" aria-label="Previous and next pages">
          {previous ? <a href={previous.href}><small>← Previous</small><strong>{previous.title}</strong></a> : <span />}
          {next ? <a href={next.href}><small>Next →</small><strong>{next.title}</strong></a> : <span />}
        </nav>
      </main>
      <aside className="toc" aria-label="On this page">
        <h2>On this page</h2>
        {document.headings.map((heading) => <a href={`#${heading.id}`} key={heading.id}>{heading.text}</a>)}
      </aside>
    </div>
  );
}
