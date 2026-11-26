import { EngineeringHeader } from "../../components/EngineeringHeader";
import { documentMetadata } from "../../lib/metadata";
import { catalogFor } from "../assessments";
import { ClaimDetails } from "./MeasuredClaims";
import type { EpisodeId, EpisodeResult, ReleaseReport, Verdict } from "../report-types";

type Kit = ReturnType<typeof catalogFor>["kits"][string];

// One analysis page for every measured release. Each release directory holds
// generated report data and a thin report/page.tsx wrapper around this.
export function analysisMetadata(release: string) {
  return documentMetadata({
    title: `${release} system analysis`, href: `/releases/${release}/report`,
    description: "Public, reproducible Hubu experiments against its claims: budget limits, overruns, crash recovery, approvals, identity and governance. Includes methods and limits.",
  });
}

const repository = "https://github.com/hacker-no-ice/hubu";
const verdicts: Record<Verdict, string> = {
  holds: "Holds", "holds-with-caveat": "Holds with caveat", "does-not-hold": "Does not hold", "not-tested": "Not tested",
};

function VerdictLabel({ verdict }: { verdict: Verdict }) {
  return <span className={`proof-verdict proof-verdict-${verdict}`}><span aria-hidden="true" />{verdicts[verdict]}</span>;
}

function Episode({ id, kit, challengeLabel, result, report }: { id: EpisodeId; kit: Kit; challengeLabel?: string; result: EpisodeResult; report: ReleaseReport }) {
  // A supplemented kit links to the kits revision that measured it.
  const sourceRef = report.supplements?.find((supplement) => supplement.episodes.includes(id))?.kitsCommit ?? report.kitsCommit ?? "main";
  return <section className="proof-episode" id={id} aria-labelledby={`${id}-title`}>
    <header className="proof-episode-header"><span className="proof-episode-number">{kit.number}</span><div><p className="proof-kicker">{kit.question}</p><h2 id={`${id}-title`}>{kit.title}</h2></div><VerdictLabel verdict={result.verdict} /></header>
    {challengeLabel && <p className="proof-kicker proof-claim-tag">{challengeLabel}</p>}
    <p className="proof-claim">{kit.statement}</p>
    <div className="proof-observation"><p className="proof-kicker">{result.execution === "complete" ? "Observed outcome" : result.execution === "incomplete" ? "Incomplete experiment" : "Awaiting a run"}</p><p>{result.summary}</p></div>
    {result.metrics.length > 0 && <dl className="proof-metrics">{result.metrics.map((metric) => <div key={metric.label}><dt>{metric.label}</dt><dd>{metric.value}</dd></div>)}</dl>}
    <p className="proof-caveat"><strong>Boundary.</strong> {result.caveat}</p>
    <details className="proof-method"><summary><span>Setup, script & verification</span><span className="proof-expand" aria-hidden="true">+</span></summary><div className="proof-method-body"><h3>Setup</h3><p>{kit.setup}</p><h3>Script</h3><ol>{kit.steps.map((step) => <li key={step}>{step}</li>)}</ol><pre><code>{`./break-hubu/${id}/run.sh --release ${report.release}`}</code></pre><a href={`${repository}/tree/${sourceRef}/break-hubu/${id}`}>Read the complete kit ↗</a><h3>Verification</h3><p>{kit.verify}</p><p>The verifier reads HTTP and CLI output from your run using exact integer amounts. It does not read SQLite or import Hubu’s implementation.</p></div></details>
  </section>;
}

export function ReleaseAnalysis({ report }: { report: ReleaseReport }) {
  const { claims, challenges, kits } = catalogFor(report.release);
  const measured = Object.entries(kits).filter(([id]) => report.episodes[id]);
  // The first analysis documents identity and bearer authority without a kit.
  const documentary = report.release === "v0.2.4" && !report.episodes["ep07-impersonate-agent"];
  const supplements = report.supplements ?? [];
  const identityNumber = String(measured.length + 1).padStart(2, "0");
  return <div className="engineering-shell">
    <EngineeringHeader section="releases" />
    <main className="engineering-article proof-report" id="main-content">
      <article className="engineering-article-inner">
        <header className="proof-heading">
          <a className="engineering-back" href={`/releases/${report.release}`}>← {report.release} release notes</a>
          <p className="eyebrow"><span /> Release analysis · {report.release}</p>
          <h1>{`${report.release} system analysis`}</h1>
          <p className="engineering-deck">{documentary ? "Four reproducible experiments against Hubu’s USD budget accounting, with their setup, scripts, results, and limits." : `${measured.length} reproducible experiments against Hubu’s claims, with their setup, scripts, results, and limits.`}</p>
          <p className="proof-scope-statement">All claims are about Hubu’s own ledger and records, not provider invoices.</p>
          <div className="proof-top-actions"><a className="button primary" href="#results">Read the results <span aria-hidden="true">↓</span></a><a className="button secondary" href="#reproduce">Run it yourself <span aria-hidden="true">↗</span></a></div>
        </header>
        <dl className="proof-provenance"><div><dt>Released source</dt><dd><a href={`${repository}/tree/${report.sourceCommit}`}>{report.release} · {report.sourceCommit.slice(0, 8)} ↗</a></dd></div><div><dt>Kits revision</dt><dd>{report.kitsCommit ? <a href={`${repository}/tree/${report.kitsCommit}/break-hubu`}>{report.kitsCommit.slice(0, 8)} ↗</a> : "Not recorded"}{(report.supplements ?? []).map((supplement) => <span key={supplement.kitsCommit}><br /><a href={`${repository}/tree/${supplement.kitsCommit}/break-hubu`}>{supplement.kitsCommit.slice(0, 8)} ↗</a> · {supplement.episodes.length} added {supplement.recordedAt}</span>)}</dd></div><div><dt>Recorded</dt><dd>{report.recordedAt ?? "No run attached"}</dd></div></dl>
        <section className="proof-overview" id="results" aria-labelledby="results-title">
          <div className="proof-section-label"><span className="proof-kicker">The 30-second read</span><h2 id="results-title">Claims and results</h2><p>Each claim is Verified when all its in-scope challenges hold, Partially verified when some do, Unverified when none have been shown, and Does not hold when any fails. Each measured challenge has its own method and reported result below.</p></div>
          <ClaimDetails release={report.release} report={report} />
          {report.release === "v0.3.0" && <p className="proof-index-note"><a className="proof-text-link" href="/releases/v0.3.0#live-verification-title">Read the live macOS approval checks ↗</a></p>}
          {documentary && <p className="proof-index-note"><a className="proof-text-link" href="#identity">Agent identity and bearer authority (C3, C5) ↘</a> remain documented limitations in this release.</p>}
        </section>
        <aside className="proof-scope"><div><span className="proof-kicker">What runs</span><p>Real released Hubu CLI + HTTP server.<br />Fresh SQLite. Synthetic external executor.</p></div><div><span className="proof-kicker">Outside this card</span><p>Gongbu, Temporal, MCP transport, provider adapters, and actual provider billing.</p></div></aside>
        {measured.map(([id, kit]) => {
          const challenge = challenges.find((item) => item.kit === id);
          const title = claims.find((claim) => claim.id === challenge?.claim)?.title;
          const challengeLabel = challenge ? `Challenge ${challenge.id} · ${title ?? challenge.claim}` : undefined;
          return <Episode key={id} id={id} kit={kit} challengeLabel={challengeLabel} result={report.episodes[id]} report={report} />;
        })}
        {documentary && <section className="proof-identity" id="identity" aria-labelledby="identity-title"><div className="proof-episode-header"><span className="proof-episode-number">{identityNumber}</span><div><p className="proof-kicker">Documented limitation · C3 and C5 · no experiment in this card</p><h2 id="identity-title">The identity gap stays visible.</h2></div><VerdictLabel verdict="does-not-hold" /></div><p>{report.release} does not bind authority to use a registered agent account to an authenticated runtime or thread. These budget experiments do not establish that boundary. This is a known gap that will be addressed in future releases.</p><p><strong>The bearer boundary matters too.</strong> The normal bearer can access protected administration routes, including budget and policy changes. Each kit fixes those settings; it does not prove a bearer holder cannot raise a budget. Driver and mock-executor separation is not server-enforced role isolation. More narrowly scoped executor authority is planned for future releases. Read the <a href={`${repository}/blob/${report.sourceCommit}/crates/hubu-api/src/lib.rs`}>released routing source ↗</a>.</p></section>}
        <section className="proof-reproduce" id="reproduce" aria-labelledby="reproduce-title">
          <div className="proof-section-label"><h2 id="reproduce-title">Reproduce these experiments</h2><p>The kits use synthetic fixtures and run over loopback. They require no LLM subscription or provider credentials.</p></div>
          <div className="proof-reproduce-grid">
            <div>
              <h3>Run the recorded kits</h3>
              <pre><code>{`git clone ${repository}.git\ncd hubu`}</code></pre>
              {report.kitsCommit && <pre><code>{`git checkout ${report.kitsCommit}\n./break-hubu/run-all.sh --release ${report.release}`}</code></pre>}
              {supplements.map(({ kitsCommit, episodes }) => (
                <pre key={kitsCommit}><code>{`git checkout ${kitsCommit}\n./break-hubu/run-all.sh --release ${report.release} --kits ${episodes.join(",")}`}</code></pre>
              ))}
              <a href={`${repository}/blob/${report.kitsCommit ?? "main"}/break-hubu/README.md`}>Prerequisites and complete instructions ↗</a>
            </div>
            <div>
              <h3>Verify your run</h3>
              <p>The kit builds the exact release in a separate checkout and starts an isolated instance. The build downloads source and dependencies. Use macOS with Git, Python 3, and the release’s pinned Rust toolchain.</p>
              <p>Each kit runs its verifier and prints where it saved the output. Its README shows how to repeat verification using your local files.</p>
            </div>
          </div>
        </section>
        <details className="proof-method proof-reading"><summary><span>How to read this card</span><span className="proof-expand" aria-hidden="true">+</span></summary><div className="proof-method-body"><dl><div><dt>Holds</dt><dd>Complete evidence supports every assertion in the stated claim.</dd></div><div><dt>Holds with caveat</dt><dd>The claim depends on an explicit condition or intervention shown beside the result.</dd></div><div><dt>Does not hold</dt><dd>Valid evidence contradicts a required assertion. A later harness failure does not erase it.</dd></div><div><dt>Not tested</dt><dd>The run is incomplete or evidence is insufficient. It is not a pass.</dd></div></dl><p>Fixture receipts let us control cost and timing. They do not establish real invoices, truthful executors, provider-side exactly-once execution, or task quality. Unknown outcomes remain unresolved until evidence supports a resolution.</p><p>{report.release === "v0.2.4" ? "This is the first published system analysis. No earlier release has been retroactively verified here." : "Each analysis applies only to its own released source. No release is retroactively verified, and earlier results are never carried forward."}</p></div></details>
        <footer className="engineering-article-footer proof-footer"><a href={`/releases/${report.release}`}>← {report.release} release notes</a><a href={`${repository}/blob/main/break-hubu/cards/${report.release}.md`}>Plain-text system analysis ↗</a></footer>
      </article>
    </main>
  </div>;
}
