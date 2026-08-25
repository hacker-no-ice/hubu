"use client";

import { useMemo, useState } from "react";

export type SearchDocument = { slug: string; href: string; title: string; excerpt: string };

export function Search({ documents, large = false }: { documents: SearchDocument[]; large?: boolean }) {
  const [query, setQuery] = useState("");
  const results = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (needle.length < 2) return [];
    return documents
      .filter((doc) => `${doc.title} ${doc.excerpt}`.toLowerCase().includes(needle))
      .slice(0, 7);
  }, [documents, query]);

  return (
    <div className={`search ${large ? "search-large" : ""}`}>
      <label>
        <span className="sr-only">Search Hubu documentation</span>
        <span className="search-icon" aria-hidden="true">⌕</span>
        <input
          type="search"
          placeholder="Search policies, budgets, MCP, operations…"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          aria-controls="search-results"
        />
        <kbd>/</kbd>
      </label>
      {results.length > 0 && (
        <div className="search-results" id="search-results">
          {results.map((result) => (
            <a href={result.href} key={result.slug}>
              <strong>{result.title}</strong><span>{result.excerpt}</span>
            </a>
          ))}
        </div>
      )}
    </div>
  );
}
