import type { EpisodeId, ReleaseReport, Verdict } from "../report-types";

// The release-notes half of a system card: the four ledger claims, measured
// when the release's generated report exists and "Not tested" until then.
const verdicts: Record<Verdict, string> = {
  holds: "Holds", "holds-with-caveat": "Holds with caveat", "does-not-hold": "Does not hold", "not-tested": "Not tested",
};
const claims: { id: EpisodeId; title: string; question: string; condition?: string }[] = [
  { id: "ep01-runaway", title: "Budget exhaustion", question: "Does Hubu stop authorizing new work once a fixed budget is consumed?" },
  { id: "ep02-last-cent", title: "Overrun accounting", question: "Is a confirmed charge above its authorization recorded in full, with the overrun visible?", condition: "Requires explicit owner reconciliation after lease expiry." },
  { id: "ep03-crash-recovery", title: "Crash recovery", question: "Does a restart preserve an unresolved claim’s financial obligation?" },
  { id: "ep04-many-clients", title: "Shared-budget burst", question: "Can many clients sharing one budget reserve more than that budget?", condition: "The released HTTP server handles requests serially. This does not establish behavior under parallel server execution." },
];

export function systemCardKicker(report: ReleaseReport | null) {
  return report?.recordedAt ? `Recorded ${report.recordedAt}` : "Measured run pending";
}

export function MeasuredClaims({ release, report, previous }: { release: string; report: ReleaseReport | null; previous?: string }) {
  return <>
    <p className="release-scope-note">{report
      ? "These findings concern Hubu’s own ledger and records, not provider invoices. Each result applies only to its stated setup."
      : `The four ledger experiments run only against a published release tag. Their ${release} results will be recorded here after publication. Until then each claim is not tested, and earlier results are not carried forward.`}</p>
    <div className="release-claims">
      {claims.map((claim) => {
        const result = report?.episodes[claim.id];
        const verdict: Verdict = result?.verdict ?? "not-tested";
        return <div className="release-claim" key={claim.id}>
          <div>
            <h3>{report ? <a href={`/releases/${release}/report#${claim.id}`}>{claim.title} <span aria-hidden="true">↗</span></a> : claim.title}</h3>
            <span className={`proof-verdict proof-verdict-${verdict}`}><span aria-hidden="true" />{verdicts[verdict]}</span>
          </div>
          <p>{result?.summary || claim.question}</p>
          {result && claim.condition && <p className="release-condition">{claim.condition}</p>}
        </div>;
      })}
    </div>
    {report
      ? <div className="release-analysis-link"><div><h3>Full system analysis</h3><p>Inspect the setup, scripts, measured charts, captured output, raw evidence, and known identity gap.</p></div><a className="button primary" href={`/releases/${release}/report`}>Read the analysis <span aria-hidden="true">↗</span></a></div>
      : previous && <div className="release-analysis-link"><div><h3>Previous measured analysis</h3><p>The {previous} analysis applies only to {previous}’s released source. It shows the setup, scripts, and evidence the {release} run will repeat.</p></div><a className="button secondary" href={`/releases/${previous}/report`}>Read the {previous} analysis <span aria-hidden="true">↗</span></a></div>}
  </>;
}
