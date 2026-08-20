import Link from "next/link";
import { Search } from "./Search";
import { adjacentDocuments, navGroups, searchDocuments, type Doc } from "../lib/docs";

function Navigation({ current }: { current: string }) {
  return (
    <nav className="docs-nav" aria-label="Documentation">
      {navGroups.map((group) => (
        <section key={group.label}>
          <h2>{group.label}</h2>
          {group.items.map(([label, slug]) => (
            <Link aria-current={slug === current ? "page" : undefined} href={`/docs/${slug}`} key={slug}>{label}</Link>
          ))}
        </section>
      ))}
      <section><h2>Visual guide</h2><a href="/architecture/">Interactive architecture <span>↗</span></a></section>
    </nav>
  );
}

export function DocsShell({ document }: { document: Doc }) {
  const { previous, next } = adjacentDocuments(document.slug);
  return (
    <div className="docs-shell">
      <header className="docs-header">
        <Link className="brand" href="/" aria-label="Hubu documentation home"><span className="brand-seal" aria-hidden="true">户</span><span>Hubu <i>/ docs</i></span></Link>
        <Search documents={searchDocuments} />
        <a className="github-compact" href="https://github.com/hacker-no-ice/hubu">GitHub ↗</a>
      </header>
      <details className="mobile-nav"><summary>Browse documentation</summary><Navigation current={document.slug} /></details>
      <aside className="sidebar"><Navigation current={document.slug} /></aside>
      <main className="doc-main" id="main-content">
        <div className="doc-status"><span>EXPERIMENTAL · LOCAL-FIRST</span><p>Evaluate carefully. Live-provider paths are experimental and are not money-grade production infrastructure.</p></div>
        {document.slug === "local-stack" && (
          <aside className="availability-note"><strong>Current lifecycle on main</strong><p>Use <code>stack init → edit → stack doctor → stack render → stack doctor → init codex</code>, then follow the service runbooks. Planned <code>stack start</code> and <code>stack status</code> commands are not available on main yet.</p></aside>
        )}
        <article className="markdown-body" dangerouslySetInnerHTML={{ __html: document.html }} />
        <div className="source-row"><a href={document.sourceUrl}>Edit this page on GitHub ↗</a><span>Canonical source: {document.sourcePath}</span></div>
        <nav className="doc-pagination" aria-label="Previous and next pages">
          {previous ? <Link href={`/docs/${previous.slug}`}><small>← Previous</small><strong>{previous.title}</strong></Link> : <span />}
          {next ? <Link href={`/docs/${next.slug}`}><small>Next →</small><strong>{next.title}</strong></Link> : <span />}
        </nav>
      </main>
      <aside className="toc" aria-label="On this page">
        <h2>On this page</h2>
        {document.headings.map((heading) => <a href={`#${heading.id}`} key={heading.id}>{heading.text}</a>)}
      </aside>
    </div>
  );
}
