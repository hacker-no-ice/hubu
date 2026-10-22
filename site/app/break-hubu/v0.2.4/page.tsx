import { EngineeringHeader } from "../../components/EngineeringHeader";
import { documentMetadata } from "../../lib/metadata";
import { report, type EpisodeId, type EpisodeResult, type EvidenceVisual, type Verdict } from "./report";

export const metadata = documentMetadata({
  title: "Break Hubu · v0.2.4 release card", href: "/break-hubu/v0.2.4",
  description: "Four public, reproducible Hubu experiments: budget exhaustion, overrun accounting, crash recovery, and shared-budget requests. Evidence and limits included.",
});

const repository = "https://github.com/hacker-no-ice/hubu";
const verdicts: Record<Verdict, string> = {
  holds: "Holds", "holds-with-caveat": "Holds with caveat", "does-not-hold": "Does not hold", "not-tested": "Not tested",
};
const episodes: { id: EpisodeId; number: string; title: string; question: string; claim: string; setup: string; steps: string[]; verify: string }[] = [
  {
    id: "ep01-runaway", number: "01", title: "The runaway agent",
    question: "What happens after the budget is gone?",
    claim: "With a fixed $0.10 budget and $0.01 per operation, Hubu stops authorizing new work once the budget is consumed—even after 100 more attempts.",
    setup: "One synthetic agent. A fresh $0.10 budget. A fixed allow policy. Each authorized operation reports a $0.01 synthetic receipt.",
    steps: ["Complete ten authorize → claim → settle operations.", "Issue 100 new logical authorization attempts after exhaustion.", "Export the final budget, authorization history, and every ledger page."],
    verify: "Compare public settlements and exact budget charges with the request trace. Denials need request evidence; an empty ledger alone cannot establish them.",
  },
  {
    id: "ep02-last-cent", number: "02", title: "The last cent",
    question: "Can the ledger tell an uncomfortable truth?",
    claim: "A confirmed $0.03 charge against a $0.01 authorization is accounted for in full, the $0.02 overrun is visible, and further positive authorizations stop.",
    setup: "A $0.01 budget, a $0.01 claimed authorization, and a synthetic receipt for $0.03. The public short lease is 30 seconds.",
    steps: ["Try normal settlement with the overrun receipt and inspect the retained hold.", "Wait for the returned lease deadline, then use the explicit owner reconciliation path.", "Repeat that reconciliation and probe fresh authorizations; inspect the public ledger."],
    verify: "Inspect the authorized maximum, actual charge, visible overrun, remaining budget, and idempotent posting. The owner action is part of the result.",
  },
  {
    id: "ep03-crash-recovery", number: "03", title: "The unresolved crash",
    question: "Does a restart make an obligation disappear?",
    claim: "After Hubu is killed with a claimed operation unresolved, restart preserves its financial obligation; lease expiry does not release it, and replaying a confirmed resolution produces one settlement.",
    setup: "A $0.01 claimed operation and a durable synthetic billed marker outside Hubu’s database. Kill only the kit’s server process before settlement.",
    steps: ["Confirm the claim publicly, write the fixture receipt, and kill the server.", "Restart the same released binary and database; inspect the claim and hold.", "After lease expiry, reconcile the confirmed charge, repeat the resolution, and restart again."],
    verify: "Check the same public authorization and claim IDs, the retained hold, rejected unsafe resolution paths, and one final ledger posting.",
  },
  {
    id: "ep04-many-clients", number: "04", title: "Many clients, one budget",
    question: "Can a burst reserve more than the budget?",
    claim: "Ten clients sharing a fixed $1 budget cannot obtain more than $0.99 of $0.03 reservations in this 100-request burst; normal settlement records those costs once.",
    setup: "Ten ordinary client processes, each issuing ten real HTTP requests. One agent account and one $1 budget. No paid model or fleet of AI agents.",
    steps: ["Release the clients at a start barrier and claim allowed operations promptly.", "Hold settlement until every burst request has completed; export the reserved budget.", "Settle the accepted receipts and export the final public accounting evidence."],
    verify: "Reconcile allows, denials, and transport errors separately. Compare the held total with final charges; a timeout never counts as a denial.",
  },
];

function VerdictLabel({ verdict }: { verdict: Verdict }) {
  return <span className={`proof-verdict proof-verdict-${verdict}`}><span aria-hidden="true" />{verdicts[verdict]}</span>;
}

function usd(microusd: number) {
  return `${microusd < 0 ? "−" : ""}$${(Math.abs(microusd) / 1_000_000).toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 6 })}`;
}

function ObservedVisual({ data, id }: { data: EvidenceVisual; id: string }) {
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

function Episode({ episode, result }: { episode: typeof episodes[number]; result: EpisodeResult }) {
  const sourceRef = report.kitsCommit ?? "main";
  return <section className="proof-episode" id={episode.id} aria-labelledby={`${episode.id}-title`}>
    <header className="proof-episode-header"><span className="proof-episode-number">{episode.number}</span><div><p className="proof-kicker">{episode.question}</p><h2 id={`${episode.id}-title`}>{episode.title}</h2></div><VerdictLabel verdict={result.verdict} /></header>
    <p className="proof-claim">{episode.claim}</p>
    <div className="proof-observation"><p className="proof-kicker">{result.execution === "complete" ? "Observed outcome" : result.execution === "incomplete" ? "Incomplete experiment" : "Awaiting evidence"}</p><p>{result.summary}</p></div>
    {result.metrics.length > 0 && <dl className="proof-metrics">{result.metrics.map((metric) => <div key={metric.label}><dt>{metric.label}</dt><dd>{metric.value}</dd></div>)}</dl>}
    {result.visual ? <ObservedVisual data={result.visual} id={episode.id} /> : <div className="proof-no-data"><span className="proof-kicker">No measured plot</span><p>A chart appears here only when captured observations are available.</p></div>}
    <p className="proof-caveat"><strong>Boundary.</strong> {result.caveat}</p>
    <details className="proof-method"><summary><span>Setup, script & verification</span><span className="proof-expand" aria-hidden="true">+</span></summary><div className="proof-method-body"><h3>Setup</h3><p>{episode.setup}</p><h3>Script</h3><ol>{episode.steps.map((step) => <li key={step}>{step}</li>)}</ol><pre><code>{`./break-hubu/${episode.id}/run.sh --release ${report.release}`}</code></pre><a href={`${repository}/tree/${sourceRef}/break-hubu/${episode.id}`}>Read the complete kit ↗</a><h3>Verification</h3><p>{episode.verify}</p><p>The verifier reads saved public HTTP and CLI evidence using exact integer amounts. It does not read SQLite or import Hubu’s implementation.</p></div></details>
    {result.terminal && <details className="proof-terminal"><summary>Captured terminal excerpt</summary><pre><code>{result.terminal}</code></pre><p>The full transcript and raw exports remain the evidence of record.</p></details>}
    <div className="proof-evidence-links" aria-label={`${episode.title} evidence`}>{result.evidence.length ? result.evidence.map((link) => <a href={link.href} key={link.href}>{link.label} <span aria-hidden="true">↗</span></a>) : <span>Raw evidence has not been attached.</span>}</div>
  </section>;
}

export default function ReleaseCard() {
  return <div className="engineering-shell">
    <EngineeringHeader section="break-hubu" />
    <main className="engineering-article proof-report" id="main-content">
      <article className="engineering-article-inner">
        <header className="proof-heading">
          <a className="engineering-back" href="/break-hubu">← Release cards</a>
          <p className="eyebrow"><span /> Break Hubu · {report.release}</p>
          <h1>Claims meet<br />the ledger.</h1>
          <p className="engineering-deck">Four small experiments against a released Hubu. The setup, the scripts, the outcomes—and the limits.</p>
          <p className="proof-scope-statement">All claims are about Hubu’s own ledger and records, not provider invoices.</p>
          <div className="proof-top-actions"><a className="button primary" href="#results">Read the results <span aria-hidden="true">↓</span></a><a className="button secondary" href="#reproduce">Run it yourself <span aria-hidden="true">↗</span></a></div>
        </header>
        <dl className="proof-provenance"><div><dt>Released source</dt><dd><a href={`${repository}/tree/${report.sourceCommit}`}>{report.release} · {report.sourceCommit.slice(0, 8)} ↗</a></dd></div><div><dt>Kits revision</dt><dd>{report.kitsCommit ? <a href={`${repository}/tree/${report.kitsCommit}/break-hubu`}>{report.kitsCommit.slice(0, 8)} ↗</a> : "Not recorded"}</dd></div><div><dt>Recorded</dt><dd>{report.recordedAt ?? "No run attached"}</dd></div><div><dt>Host</dt><dd>{report.host ?? "Not recorded"}</dd></div></dl>
        <section className="proof-overview" id="results" aria-labelledby="results-title">
          <div className="proof-section-label"><span className="proof-kicker">The 30-second read</span><h2 id="results-title">Four tests. One known gap.</h2><p>Each claim stands on its own evidence. Caveats and failures are part of the card.</p></div>
          <div className="proof-summary-table"><table><thead><tr><th scope="col">Claim</th><th scope="col">Outcome</th><th scope="col">What the evidence says</th></tr></thead><tbody>{episodes.map((episode) => <tr key={episode.id}><th scope="row"><a href={`#${episode.id}`}><span>{episode.number}</span> {episode.title} <span aria-hidden="true">↘</span></a></th><td><VerdictLabel verdict={report.episodes[episode.id].verdict} /></td><td>{report.episodes[episode.id].summary}</td></tr>)}<tr><th scope="row"><a href="#identity"><span>05</span> Agent identity <span aria-hidden="true">↘</span></a></th><td><VerdictLabel verdict="does-not-hold" /></td><td>Runtime/thread-bound authority is a documented gap in v0.2.4. This row is documentary evidence, not a new experiment.</td></tr></tbody></table></div>
        </section>
        <aside className="proof-scope"><div><span className="proof-kicker">What runs</span><p>Real released Hubu CLI + HTTP server.<br />Fresh SQLite. Synthetic external executor.</p></div><div><span className="proof-kicker">Outside this card</span><p>Gongbu, Temporal, MCP transport, provider adapters, and actual provider billing.</p></div></aside>
        {episodes.map((episode) => <Episode key={episode.id} episode={episode} result={report.episodes[episode.id]} />)}
        <section className="proof-identity" id="identity" aria-labelledby="identity-title"><div className="proof-episode-header"><span className="proof-episode-number">05</span><div><p className="proof-kicker">Documented limitation · no experiment in this card</p><h2 id="identity-title">The identity gap stays visible.</h2></div><VerdictLabel verdict="does-not-hold" /></div><p>v0.2.4 does not bind authority to use a registered agent account to an authenticated runtime or thread. These budget experiments do not establish that boundary. The work is tracked in <a href="https://linear.app/hubu/issue/HUB-162/bind-agent-account-use-to-an-authenticated-agent-session">HUB-162 ↗</a> for v0.3.</p><p><strong>The bearer boundary matters too.</strong> The normal bearer can access protected administration routes, including budget and policy changes. Each kit fixes those settings; it does not prove a bearer holder cannot raise a budget. Driver and mock-executor separation is not server-enforced role isolation. See <a href="https://linear.app/hubu/issue/HUB-32/issue-executor-scoped-hubu-capabilities-for-gongbu">HUB-32 ↗</a> and the <a href={`${repository}/blob/${report.sourceCommit}/crates/hubu-api/src/lib.rs`}>released routing source ↗</a>.</p></section>
        <section className="proof-reproduce" id="reproduce" aria-labelledby="reproduce-title"><div className="proof-section-label"><p className="proof-kicker">Make it your experiment</p><h2 id="reproduce-title">Read it. Run it. Compare it.</h2><p>One deterministic driver is enough. You do not need an LLM subscription or real provider credentials.</p></div><div className="proof-reproduce-grid"><div><h3>Start from the recorded kit</h3>{report.kitsCommit ? <pre><code>{`git clone ${repository}.git\ncd hubu\ngit checkout ${report.kitsCommit}\n./break-hubu/run-all.sh --release ${report.release}`}</code></pre> : <p className="proof-caveat">A kits revision will be pinned with the recorded run. No reproducibility claim is made before that revision and its evidence are attached.</p>}<p>The entry point builds the exact released source in a separate checkout and starts an isolated instance. It does not replace your installation or select another profile.</p><a href={`${repository}/blob/${report.kitsCommit ?? "main"}/break-hubu/README.md`}>Prerequisites and complete instructions ↗</a></div><div><h3>Inspect the evidence</h3><p>Every run retains the real terminal transcript, request trace, public ledger pages, authorization records, budget snapshots, and verification output—even when a claim fails.</p><p>Initial qualified host: macOS. Git, Python 3, the pinned Rust toolchain, and platform build prerequisites are required. The first build downloads source and dependencies; the experiment uses loopback.</p>{report.evidenceBundle ? <a className="button secondary" href={report.evidenceBundle.href}>{report.evidenceBundle.label} ↓</a> : <p className="proof-pending-note">A downloadable evidence bundle has not been attached.</p>}</div></div></section>
        <details className="proof-method proof-reading"><summary><span>How to read this card</span><span className="proof-expand" aria-hidden="true">+</span></summary><div className="proof-method-body"><dl><div><dt>Holds</dt><dd>Complete evidence supports every assertion in the stated claim.</dd></div><div><dt>Holds with caveat</dt><dd>The claim depends on an explicit condition or intervention shown beside the result.</dd></div><div><dt>Does not hold</dt><dd>Valid evidence contradicts a required assertion. A later harness failure does not erase it.</dd></div><div><dt>Not tested</dt><dd>The run is incomplete or evidence is insufficient. It is not a pass.</dd></div></dl><p>Fixture receipts let us control cost and timing. They do not establish real invoices, truthful executors, provider-side exactly-once execution, or task quality. Unknown outcomes remain unresolved until evidence supports a resolution.</p><p>This is the first release card. There is no previous card to compare against.</p></div></details>
        <footer className="engineering-article-footer proof-footer"><a href="/break-hubu">← All release cards</a><a href={`${repository}/blob/main/break-hubu/cards/v0.2.4.md`}>Plain-text release card ↗</a></footer>
      </article>
    </main>
  </div>;
}
