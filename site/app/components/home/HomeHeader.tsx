"use client";

import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { NavLink, NavPanel, SearchEntry } from "../../lib/home";
import { HubuWordmark } from "../HubuWordmark";

type Props = { panels: NavPanel[]; searchIndex: SearchEntry[]; popular: string[] };

export function HomeHeader({ panels, searchIndex, popular }: Props) {
  const [open, setOpen] = useState<string | null>(null);
  const closeTimer = useRef<number | undefined>(undefined);
  const openTimer = useRef<number | undefined>(undefined);
  const headerRef = useRef<HTMLElement>(null);

  const show = useCallback((id: string | null) => {
    window.clearTimeout(closeTimer.current);
    setOpen(id);
  }, []);
  const hideSoon = () => {
    window.clearTimeout(openTimer.current);
    window.clearTimeout(closeTimer.current);
    closeTimer.current = window.setTimeout(() => setOpen(null), 180);
  };

  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      if (event.key === "Escape" && open) {
        const id = open;
        setOpen(null);
        headerRef.current?.querySelector<HTMLButtonElement>(`.ncat[data-panel="${id}"]`)?.focus();
      }
      const target = event.target as HTMLElement | null;
      if (event.key === "/" && !target?.closest("input, textarea, [contenteditable]")) {
        event.preventDefault();
        setOpen(null);
        const wide = window.matchMedia("(min-width: 901px)").matches;
        if (!wide) headerRef.current?.querySelector<HTMLDetailsElement>(".mmenu")?.setAttribute("open", "");
        headerRef.current?.querySelector<HTMLInputElement>(wide ? "#header-search" : "#menu-search")?.focus();
      }
    }
    function onFocus(event: FocusEvent) {
      if (open && !headerRef.current?.contains(event.target as Node)) setOpen(null);
    }
    document.addEventListener("keydown", onKey);
    document.addEventListener("focusin", onFocus);
    return () => {
      document.removeEventListener("keydown", onKey);
      document.removeEventListener("focusin", onFocus);
    };
  }, [open]);

  function onCategoryKey(event: React.KeyboardEvent<HTMLButtonElement>, index: number) {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      show(panels[index].id);
      window.requestAnimationFrame(() => headerRef.current?.querySelector<HTMLAnchorElement>(`#np-${panels[index].id} a`)?.focus());
    }
    if (event.key === "ArrowRight" || event.key === "ArrowLeft") {
      event.preventDefault();
      const next = (index + (event.key === "ArrowRight" ? 1 : panels.length - 1)) % panels.length;
      headerRef.current?.querySelectorAll<HTMLButtonElement>(".ncat")[next]?.focus();
      if (open) show(panels[next].id);
    }
  }

  return (
    <>
      <header className="topbar home-header" ref={headerRef}>
        <div className="home-wrap tb">
          <div className="header-branding">
            <a className="brand" href="/" aria-label="Hubu home">
              <HubuWordmark className="brand-wordmark" decorative />
            </a>
            <a className="header-version" href="/releases/v0.3.0" aria-label="Read Hubu v0.3.0 release notes and system card">v0.3.0 <span aria-hidden="true">↗</span></a>
          </div>
          <nav className="nbar" aria-label="Primary navigation">
            <ul>
              {panels.map((panel, index) => (
                <li key={panel.id}>
                  <button
                    className="ncat"
                    type="button"
                    data-panel={panel.id}
                    aria-expanded={open === panel.id}
                    aria-controls={`np-${panel.id}`}
                    onPointerEnter={(event) => {
                      if (event.pointerType === "touch") return;
                      window.clearTimeout(openTimer.current);
                      openTimer.current = window.setTimeout(() => show(panel.id), open ? 0 : 90);
                    }}
                    onPointerLeave={hideSoon}
                    onClick={() => show(open === panel.id ? null : panel.id)}
                    onKeyDown={(event) => onCategoryKey(event, index)}
                  >
                    {panel.label}<i aria-hidden="true" />
                  </button>
                </li>
              ))}
            </ul>
          </nav>
          <div className="nright">
            <HeaderSearch id="header-search" index={searchIndex} popular={popular} />
            <a className="ngh" href="https://github.com/hacker-no-ice/hubu" aria-label="Hubu on GitHub">
              <svg viewBox="0 0 16 16" width="18" height="18" aria-hidden="true"><path fill="currentColor" d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z" /></svg>
            </a>
            <details className="mmenu site-menu">
              <summary>Menu</summary>
              <nav aria-label="Primary navigation (mobile)">
                <div className="msearch"><HeaderSearch id="menu-search" index={searchIndex} popular={popular} inline /></div>
                {panels.map((panel) => (
                  <details className="mgroup" key={panel.id}>
                    <summary>{panel.label}</summary>
                    <div>
                      {panel.columns.flatMap((column) => column.links).map((link) => <a className="nl" href={link.href} key={link.href}><b>{link.meta ? `${link.meta.label} · ${link.title}` : link.title}</b></a>)}
                      <a className="nl" href={panel.feature.href}><b>{panel.feature.cta}</b></a>
                    </div>
                  </details>
                ))}
                <a className="mlink" href="https://github.com/hacker-no-ice/hubu">GitHub ↗</a>
              </nav>
            </details>
          </div>
        </div>
        <div className="npanels">
          {panels.map((panel) => (
            <div className="npanel" id={`np-${panel.id}`} key={panel.id} hidden={open !== panel.id} onPointerEnter={() => window.clearTimeout(closeTimer.current)} onPointerLeave={hideSoon}>
              <div className={`home-wrap ngrid ng-${panel.id}`}>
                {panel.columns.map((column) => (
                  <div className="ncol" key={column.heading}>
                    <h4>{column.heading}</h4>
                    {column.links.map((link) => <PanelLink link={link} rows={column.layout === "rows"} key={link.href} />)}
                  </div>
                ))}
                <a className="nfeat" href={panel.feature.href}>
                  <small>{panel.feature.kicker}</small>
                  <b>{panel.feature.title}</b>
                  <span>{panel.feature.description}</span>
                  {panel.feature.code && <code>{panel.feature.code}</code>}
                  <em>{panel.feature.cta}</em>
                </a>
              </div>
            </div>
          ))}
        </div>
      </header>
      <div className={`nscrim${open ? " on" : ""}`} aria-hidden="true" onClick={() => setOpen(null)} />
    </>
  );
}

function PanelLink({ link, rows }: { link: NavLink; rows: boolean }) {
  if (rows && link.meta) {
    return (
      <a className="nl nrow" href={link.href}>
        <span className="nmeta">
          <em className={`ntag ${link.meta.tone ?? ""}`}>{link.meta.label}</em>
          {link.meta.date && <time dateTime={link.meta.date}>{link.meta.dateLabel}</time>}
        </span>
        <b>{link.title}</b>
      </a>
    );
  }
  return (
    <a className="nl" href={link.href}>
      <b>{link.title}</b>
      {link.description && <span>{link.description}</span>}
    </a>
  );
}

function escapeRegExp(value: string) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function Highlight({ text, tokens }: { text: string; tokens: string[] }) {
  const usable = tokens.filter((token) => token.length > 1);
  if (!usable.length) return <>{text}</>;
  const parts = text.split(new RegExp(`(${usable.map(escapeRegExp).join("|")})`, "ig"));
  return <>{parts.map((part, index) => (index % 2 ? <mark key={index}>{part}</mark> : <Fragment key={index}>{part}</Fragment>))}</>;
}

function HeaderSearch({ id, index, popular, inline = false }: { id: string; index: SearchEntry[]; popular: string[]; inline?: boolean }) {
  const [query, setQuery] = useState("");
  const [expanded, setExpanded] = useState(false);
  const [selected, setSelected] = useState(-1);
  const boxRef = useRef<HTMLDivElement>(null);

  const { tokens, results } = useMemo(() => {
    const tokens = query.toLowerCase().split(/\s+/).filter(Boolean);
    if (!tokens.length) {
      return { tokens, results: popular.map((href) => index.find((entry) => entry.href === href)).filter((entry): entry is SearchEntry => Boolean(entry)).map((entry) => ({ entry, heading: undefined as string | undefined })) };
    }
    const scored: { entry: SearchEntry; heading?: string; score: number }[] = [];
    for (const entry of index) {
      const title = entry.title.toLowerCase();
      const headings = entry.headings.map((heading) => heading.toLowerCase());
      let score = 0;
      let heading: string | undefined;
      let matched = true;
      for (const token of tokens) {
        let tokenScore = title.startsWith(token) ? 9 : title.includes(token) ? 6 : 0;
        const headingIndex = headings.findIndex((text) => text.includes(token));
        if (headingIndex >= 0 && tokenScore < 3) { tokenScore = 3; heading ??= entry.headings[headingIndex]; }
        if (!tokenScore && entry.group.toLowerCase().includes(token)) tokenScore = 1.5;
        if (!tokenScore && entry.excerpt.toLowerCase().includes(token)) tokenScore = 1;
        if (!tokenScore) { matched = false; break; }
        score += tokenScore;
      }
      if (matched) scored.push({ entry, heading, score });
    }
    scored.sort((left, right) => right.score - left.score);
    return { tokens, results: scored.slice(0, 8) };
  }, [index, popular, query]);

  const listId = `${id}-results`;
  const open = expanded;
  const activeIndex = query.trim() && results.length && selected < 0 ? 0 : selected;

  function onKeyDown(event: React.KeyboardEvent<HTMLInputElement>) {
    if (event.key === "ArrowDown") { event.preventDefault(); setExpanded(true); setSelected(Math.min(results.length - 1, activeIndex + 1)); }
    else if (event.key === "ArrowUp") { event.preventDefault(); setSelected(Math.max(0, activeIndex - 1)); }
    else if (event.key === "Enter") {
      event.preventDefault();
      const target = results[Math.max(0, activeIndex)];
      if (target) window.location.href = target.entry.href;
    } else if (event.key === "Escape") {
      event.stopPropagation();
      if (query) { setQuery(""); setSelected(-1); } else { setExpanded(false); event.currentTarget.blur(); }
    }
  }

  return (
    <div
      className={`hsearch${inline ? " inline" : ""}`}
      role="search"
      ref={boxRef}
      onBlur={(event) => { if (!boxRef.current?.contains(event.relatedTarget as Node)) setExpanded(false); }}
    >
      <label className="hs-field" htmlFor={id}>
        <span className="sr-only">Search Hubu documentation</span>
        <span aria-hidden="true" className="hs-icon">⌕</span>
        <input
          id={id}
          type="search"
          placeholder="Search docs"
          autoComplete="off"
          spellCheck={false}
          role="combobox"
          aria-expanded={open}
          aria-controls={listId}
          aria-autocomplete="list"
          aria-activedescendant={open && activeIndex >= 0 ? `${id}-o${activeIndex}` : undefined}
          value={query}
          onChange={(event) => { setQuery(event.target.value); setSelected(-1); setExpanded(true); }}
          onFocus={() => setExpanded(true)}
          onKeyDown={onKeyDown}
        />
        <kbd>/</kbd>
      </label>
      {open && (
        <div className="hs-results" id={listId} role="listbox" aria-label="Search results">
          {!query.trim() && <h5>Popular</h5>}
          {results.length ? results.map(({ entry, heading }, position) => (
            <a
              className="hs-item"
              role="option"
              id={`${id}-o${position}`}
              aria-selected={position === activeIndex}
              href={entry.href}
              key={entry.href}
              onPointerMove={() => setSelected(position)}
            >
              <small>{entry.group}</small>
              <b><Highlight text={entry.title} tokens={tokens} /></b>
              <span>{heading ? <>§ <Highlight text={heading} tokens={tokens} /></> : <Highlight text={entry.excerpt} tokens={tokens} />}</span>
            </a>
          )) : <p className="hs-empty">No pages match “{query}”. Try a command like <code>stack</code> or a concept like <code>budget</code>.</p>}
          <div className="hs-foot" aria-hidden="true"><span>↑↓ to move</span><span>↵ to open</span><span>esc to close</span></div>
        </div>
      )}
    </div>
  );
}
