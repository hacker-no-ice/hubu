import { EngineeringHeader } from "../../components/EngineeringHeader";
import { documentMetadata } from "../../lib/metadata";
import catalog from "../claims.json";
import type { EpisodeId, EpisodeResult, EvidenceVisual, ReleaseReport, Verdict } from "../report-types";

type Kit = { id: EpisodeId; number: string; title: string; question: string; claim: string; setup: string; steps: string[]; verify: string; challenge?: { id: string; title: string; claim: string } };
const kitText = catalog.kits as Record<string, { number: string; title: string; question: string; statement: string; setup: string; steps: string[]; verify: string }>;
const kits: Kit[] = Object.entries(kitText).map(([id, kit]) => ({
  id, number: kit.number, title: kit.title, question: kit.question, claim: kit.statement, setup: kit.setup, steps: kit.steps, verify: kit.verify,
  challenge: catalog.challenges.find((challenge) => challenge.kit === id),
}));
const claimTitle = (id: string) => catalog.claims.find((claim) => claim.id === id)?.title ?? id;

// One analysis page for every measured release. Each release directory holds
// generated report data and a thin report/page.tsx wrapper around this.
export function analysisMetadata(release: string) {
  return documentMetadata({
    title: `${release} system analysis`, href: `/releases/${release}/report`,
    description: "Public, reproducible Hubu experiments against its claims: budget limits, overruns, crash recovery, approvals, identity and governance. Evidence and limits included.",
  });
}

const repository = "https://github.com/hacker-no-ice/hubu";
const verdicts: Record<Verdict, string> = {
  holds: "Holds", "holds-with-caveat": "Holds with caveat", "does-not-hold": "Does not hold", "not-tested": "Not tested",
};

function VerdictLabel({ verdict }: { verdict: Verdict }) {
  return <span className={`proof-verdict proof-verdict-${verdict}`}><span aria-hidden="true" />{verdicts[verdict]}</span>;
}

function usd(microusd: number) {
  return `${microusd < 0 ? "−" : ""}$${(Math.abs(microusd) / 1_000_000).toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 6 })}`;
}

function ObservedVisual({ data, id, report }: { data: EvidenceVisual; id: string; report: ReleaseReport }) {
  const captionId = `${id}-caption`;
  let graphic;
  if (data.kind === "consumption") {
    const maxX = Math.max(1, ...data.points.map((point) => point.attempt));
    const maxY = Math.max(1, data.budgetMicrousd, ...data.points.map((point) => point.consumedMicrousd));
    const x = (value: number) => 70 + value / maxX * 740;
    const y = (value: number) => 230 - value / maxY * 170;
    const path = data.points.map((point, index) => `${index ? "L" : "M"}${x(point.attempt)},${y(point.consumedMicrousd)}`).join(" ");
    graphic = <>
      <svg viewBox="0 0 880 290" role="img" aria-labelledby={`${id}-plot-title ${captionId}`} className="proof-line-chart">
        <title id={`${id}-plot-title`}>Observed cumulative budget consumption by authorization attempt</title>
        <line x1="70" x2="810" y1="230" y2="230" className="proof-chart-grid" />
        <line x1="70" x2="810" y1={y(data.budgetMicrousd)} y2={y(data.budgetMicrousd)} className="proof-chart-budget" />
        <text x="70" y="28">Consumed · USD</text>
        <text x="810" y={y(data.budgetMicrousd) - 12} textAnchor="end">{usd(data.budgetMicrousd)} budget</text>
        <text x="53" y="235" textAnchor="end">$0</text>
        <path d={path} className="proof-chart-line" />
        {data.points.map((point) => <circle key={point.attempt} cx={x(point.attempt)} cy={y(point.consumedMicrousd)} r="3" className="proof-chart-point" />)}
        <text x="70" y="260">0</text><text x="810" y="260" textAnchor="end">{maxX}</text>
        <text x="440" y="280" textAnchor="middle">Authorization attempt</text>
      </svg>
      <p className="proof-chart-note">{data.denied} denied attempts recorded. The line shows exported consumption observations.</p>
      <details className="proof-data-table"><summary>Read the plotted values</summary><table><thead><tr><th scope="col">Attempt</th><th scope="col">Consumed</th></tr></thead><tbody>{data.points.map((point) => <tr key={point.attempt}><td>{point.attempt}</td><td>{usd(point.consumedMicrousd)}</td></tr>)}</tbody></table></details>
    </>;
  } else if (data.kind === "costs") {
    const amounts = [["Authorized maximum", data.authorizedMicrousd], ["Recorded charge", data.recordedMicrousd], ["Visible overrun", data.overrunMicrousd]] as const;
    const max = Math.max(1, ...amounts.map(([, amount]) => amount));
    graphic = <><div className="proof-cost-bars">{amounts.map(([label, amount], index) => <div className="proof-bar-row" key={label}><div><span>{label}</span><strong>{usd(amount)}</strong></div><div className="proof-bar-track"><span className={`proof-bar-fill proof-series-${index}`} style={{ width: `${Math.max(0, amount) / max * 100}%` }} /></div></div>)}</div><p className="proof-chart-note">{data.reconciled ? "The public claim records explicit owner reconciliation. Normal settlement and reconciliation are separate steps." : "The public receipt provides the authorized maximum, recorded charge, and overrun. See verification for the observed resolution path."}</p></>;
  } else if (data.kind === "timeline") {
    graphic = <ol className="proof-timeline">{data.events.map((event, index) => <li key={`${event.label}-${index}`}><span className="proof-timeline-time">+{event.elapsedSeconds.toFixed(1)}s</span><div><strong>{event.label}</strong><p>{event.detail}</p></div></li>)}</ol>;
  } else {
    const counts = [["Allowed", data.allowed], ["Denied", data.denied], ["Unresolved/errors", data.errors]] as const;
    const total = data.allowed + data.denied + data.errors;
    graphic = <><div className="proof-outcome-strip" aria-hidden="true">{counts.map(([label, count], index) => count > 0 ? <span key={label} className={`proof-series-${index}`} style={{ flex: count }}>{count}</span> : null)}</div><dl className="proof-counts">{counts.map(([label, count], index) => <div key={label}><dt><span className={`proof-key proof-series-${index}`} />{label}</dt><dd>{count}<small> / {total}</small></dd></div>)}</dl><dl className="proof-money-states"><div><dt>Reserved after burst</dt><dd>{usd(data.reservedMicrousd)}</dd></div><div><dt>Consumed after settlement</dt><dd>{usd(data.consumedMicrousd)}</dd></div><div><dt>Remaining after settlement</dt><dd>{usd(data.remainingMicrousd)}</dd></div></dl></>;
  }
  return <figure className="proof-figure">{graphic}<figcaption id={captionId}>{report.release} · Observed public evidence · <a href={data.source.href}>{data.source.label} ↗</a></figcaption></figure>;
}

function Episode({ episode, result, report }: { episode: Kit; result: EpisodeResult; report: ReleaseReport }) {
  const sourceRef = report.kitsCommit ?? "main";
  return <section className="proof-episode" id={episode.id} aria-labelledby={`${episode.id}-title`}>
    <header className="proof-episode-header"><span className="proof-episode-number">{episode.number}</span><div><p className="proof-kicker">{episode.question}</p><h2 id={`${episode.id}-title`}>{episode.title}</h2></div><VerdictLabel verdict={result.verdict} /></header>
    {episode.challenge && <p className="proof-kicker proof-claim-tag">Challenge {episode.challenge.id} · {claimTitle(episode.challenge.claim)}</p>}
    <p className="proof-claim">{episode.claim}</p>
    <div className="proof-observation"><p className="proof-kicker">{result.execution === "complete" ? "Observed outcome" : result.execution === "incomplete" ? "Incomplete experiment" : "Awaiting evidence"}</p><p>{result.summary}</p></div>
    {result.metrics.length > 0 && <dl className="proof-metrics">{result.metrics.map((metric) => <div key={metric.label}><dt>{metric.label}</dt><dd>{metric.value}</dd></div>)}</dl>}
    {result.visual && <ObservedVisual data={result.visual} id={episode.id} report={report} />}
    <p className="proof-caveat"><strong>Boundary.</strong> {result.caveat}</p>
    <details className="proof-method"><summary><span>Setup, script & verification</span><span className="proof-expand" aria-hidden="true">+</span></summary><div className="proof-method-body"><h3>Setup</h3><p>{episode.setup}</p><h3>Script</h3><ol>{episode.steps.map((step) => <li key={step}>{step}</li>)}</ol><pre><code>{`./break-hubu/${episode.id}/run.sh --release ${report.release}`}</code></pre><a href={`${repository}/tree/${sourceRef}/break-hubu/${episode.id}`}>Read the complete kit ↗</a><h3>Verification</h3><p>{episode.verify}</p><p>The verifier reads saved public HTTP and CLI evidence using exact integer amounts. It does not read SQLite or import Hubu’s implementation.</p></div></details>
    {result.terminal && <details className="proof-terminal"><summary>Captured terminal excerpt</summary><pre><code>{result.terminal}</code></pre><p>The full transcript and raw exports remain the evidence of record.</p></details>}
    <div className="proof-evidence-links" aria-label={`${episode.title} evidence`}>{result.evidence.length ? result.evidence.map((link) => <a href={link.href} key={link.href}>{link.label} <span aria-hidden="true">↗</span></a>) : <span>Raw evidence has not been attached.</span>}</div>
  </section>;
}

export function ReleaseAnalysis({ report }: { report: ReleaseReport }) {
  const measured = kits.filter((kit) => report.episodes[kit.id]);
  // Releases measured before the identity and authority kits keep their documentary row.
  const documentary = !report.episodes["ep07-impersonate-agent"];
  const identityNumber = String(measured.length + 1).padStart(2, "0");
  return <div className="engineering-shell">
    <EngineeringHeader section="releases" />
    <main className="engineering-article proof-report" id="main-content">
      <article className="engineering-article-inner">
        <header className="proof-heading">
          <a className="engineering-back" href={`/releases/${report.release}`}>← {report.release} release notes</a>
          <p className="eyebrow"><span /> Release analysis · {report.release}</p>
          <h1>{`${report.release} system analysis`}</h1>
          <p className="engineering-deck">{documentary ? "Four reproducible experiments against Hubu’s USD budget accounting, with their setup, scripts, evidence, and limits." : `${measured.length} reproducible experiments against Hubu’s claims, with their setup, scripts, evidence, and limits.`}</p>
          <p className="proof-scope-statement">All claims are about Hubu’s own ledger and records, not provider invoices.</p>
          <div className="proof-top-actions"><a className="button primary" href="#results">Read the results <span aria-hidden="true">↓</span></a><a className="button secondary" href="#reproduce">Run it yourself <span aria-hidden="true">↗</span></a></div>
        </header>
        <dl className="proof-provenance"><div><dt>Released source</dt><dd><a href={`${repository}/tree/${report.sourceCommit}`}>{report.release} · {report.sourceCommit.slice(0, 8)} ↗</a></dd></div><div><dt>Kits revision</dt><dd>{report.kitsCommit ? <a href={`${repository}/tree/${report.kitsCommit}/break-hubu`}>{report.kitsCommit.slice(0, 8)} ↗</a> : "Not recorded"}{(report.supplements ?? []).map((supplement) => <span key={supplement.kitsCommit}><br /><a href={`${repository}/tree/${supplement.kitsCommit}/break-hubu`}>{supplement.kitsCommit.slice(0, 8)} ↗</a> · {supplement.episodes.length} added {supplement.recordedAt}</span>)}</dd></div><div><dt>Recorded</dt><dd>{report.recordedAt ?? "No run attached"}</dd></div><div><dt>Host</dt><dd>{report.host ?? "Not recorded"}</dd></div></dl>
        <section className="proof-overview" id="results" aria-labelledby="results-title">
          <div className="proof-section-label"><span className="proof-kicker">The 30-second read</span><h2 id="results-title">{documentary ? `${measured.length === 4 ? "Four" : measured.length} tests. One known gap.` : `${measured.length} tests across Hubu’s claims.`}</h2><p>Each claim stands on its own evidence. Caveats and failures are part of the card.</p></div>
          <div className="proof-summary-table"><table><thead><tr><th scope="col">Claim</th><th scope="col">Outcome</th><th scope="col">What the evidence says</th></tr></thead><tbody>{measured.map((episode) => <tr key={episode.id}><th scope="row"><a href={`#${episode.id}`}><span>{episode.number}</span> {episode.title} <span aria-hidden="true">↘</span></a></th><td><VerdictLabel verdict={report.episodes[episode.id].verdict} /></td><td>{report.episodes[episode.id].summary}</td></tr>)}{documentary && <tr><th scope="row"><a href="#identity"><span>{identityNumber}</span> Agent identity (C3, C5) <span aria-hidden="true">↘</span></a></th><td><VerdictLabel verdict="does-not-hold" /></td><td>Runtime/thread-bound authority is a documented gap in {report.release}. This row is documentary evidence, not a new experiment.</td></tr>}</tbody></table></div>
        </section>
        <aside className="proof-scope"><div><span className="proof-kicker">What runs</span><p>Real released Hubu CLI + HTTP server.<br />Fresh SQLite. Synthetic external executor.</p></div><div><span className="proof-kicker">Outside this card</span><p>Gongbu, Temporal, MCP transport, provider adapters, and actual provider billing.</p></div></aside>
        {measured.map((episode) => <Episode key={episode.id} episode={episode} result={report.episodes[episode.id]} report={report} />)}
        {documentary && <section className="proof-identity" id="identity" aria-labelledby="identity-title"><div className="proof-episode-header"><span className="proof-episode-number">{identityNumber}</span><div><p className="proof-kicker">Documented limitation · C3 and C5 · no experiment in this card</p><h2 id="identity-title">The identity gap stays visible.</h2></div><VerdictLabel verdict="does-not-hold" /></div><p>{report.release} does not bind authority to use a registered agent account to an authenticated runtime or thread. These budget experiments do not establish that boundary. This is a known gap that will be addressed in future releases.</p><p><strong>The bearer boundary matters too.</strong> The normal bearer can access protected administration routes, including budget and policy changes. Each kit fixes those settings; it does not prove a bearer holder cannot raise a budget. Driver and mock-executor separation is not server-enforced role isolation. More narrowly scoped executor authority is planned for future releases. Read the <a href={`${repository}/blob/${report.sourceCommit}/crates/hubu-api/src/lib.rs`}>released routing source ↗</a>.</p></section>}
        <section className="proof-reproduce" id="reproduce" aria-labelledby="reproduce-title"><div className="proof-section-label"><p className="proof-kicker">Make it your experiment</p><h2 id="reproduce-title">Read it. Run it. Compare it.</h2><p>One deterministic driver is enough. You do not need an LLM subscription or real provider credentials.</p></div><div className="proof-reproduce-grid"><div><h3>Start from the recorded kit</h3>{report.kitsCommit ? <pre><code>{`git clone ${repository}.git\ncd hubu\ngit checkout ${report.kitsCommit}\n./break-hubu/run-all.sh --release ${report.release}`}</code></pre> : <p className="proof-caveat">A kits revision will be pinned with the recorded run. No reproducibility claim is made before that revision and its evidence are attached.</p>}<p>The entry point builds the exact released source in a separate checkout and starts an isolated instance. It does not replace your installation or select another profile.</p><a href={`${repository}/blob/${report.kitsCommit ?? "main"}/break-hubu/README.md`}>Prerequisites and complete instructions ↗</a></div><div><h3>Inspect the evidence</h3><p>Every run retains the real terminal transcript, request trace, public ledger pages, authorization records, budget snapshots, and verification output—even when a claim fails.</p><p>Initial qualified host: macOS. Git, Python 3, the pinned Rust toolchain, and platform build prerequisites are required. The first build downloads source and dependencies; the experiment uses loopback.</p>{report.evidenceBundle ? <a className="button secondary" href={report.evidenceBundle.href}>{report.evidenceBundle.label} ↓</a> : <p className="proof-pending-note">A downloadable evidence bundle has not been attached.</p>}</div></div></section>
        <details className="proof-method proof-reading"><summary><span>How to read this card</span><span className="proof-expand" aria-hidden="true">+</span></summary><div className="proof-method-body"><dl><div><dt>Holds</dt><dd>Complete evidence supports every assertion in the stated claim.</dd></div><div><dt>Holds with caveat</dt><dd>The claim depends on an explicit condition or intervention shown beside the result.</dd></div><div><dt>Does not hold</dt><dd>Valid evidence contradicts a required assertion. A later harness failure does not erase it.</dd></div><div><dt>Not tested</dt><dd>The run is incomplete or evidence is insufficient. It is not a pass.</dd></div></dl><p>Fixture receipts let us control cost and timing. They do not establish real invoices, truthful executors, provider-side exactly-once execution, or task quality. Unknown outcomes remain unresolved until evidence supports a resolution.</p><p>{report.release === "v0.2.4" ? "This is the first published system analysis. No earlier release has been retroactively verified here." : "Each analysis applies only to its own released source. No release is retroactively verified, and earlier results are never carried forward."}</p></div></details>
        <footer className="engineering-article-footer proof-footer"><a href={`/releases/${report.release}`}>← {report.release} release notes</a><a href={`${repository}/blob/main/break-hubu/cards/${report.release}.md`}>Plain-text system analysis ↗</a></footer>
      </article>
    </main>
  </div>;
}
