import catalog from "../claims.json";
import type { EpisodeId, ReleaseReport, Verdict } from "../report-types";

// The release-notes system card: Hubu's six claims (C1–C6). C1 is measured by
// the four proof-kit experiments and derives its status from the release's
// report; every other claim carries an explicit assessment from the release page.
export type ClaimId = "C1" | "C2" | "C3" | "C4" | "C5" | "C6";
export type ClaimStatus = Verdict | "known-gap" | "not-applicable";
export type Assessment = { status: ClaimStatus; note: string };

const labels: Record<ClaimStatus, string> = {
  holds: "Holds", "holds-with-caveat": "Holds with caveat", "does-not-hold": "Does not hold", "not-tested": "Not tested",
  "known-gap": "Known gap", "not-applicable": "Not applicable",
};
const styles: Record<ClaimStatus, string> = {
  holds: "holds", "holds-with-caveat": "holds-with-caveat", "does-not-hold": "does-not-hold", "not-tested": "not-tested",
  "known-gap": "does-not-hold", "not-applicable": "not-tested",
};
const conditions: Partial<Record<EpisodeId, string>> = {
  "ep02-last-cent": "Requires explicit owner reconciliation after lease expiry.",
  "ep04-many-clients": "The released HTTP server handles requests serially. This does not establish behavior under parallel server execution.",
};
const experimentNames = catalog.experiments as Record<EpisodeId, string>;

function Status({ status }: { status: ClaimStatus }) {
  return <span className={`proof-verdict proof-verdict-${styles[status]}`} data-status={status}><span aria-hidden="true" />{labels[status]}</span>;
}

// A claim measured by several experiments is only as strong as its weakest one.
export function measuredStatus(report: ReleaseReport | null, experiments: EpisodeId[]): Verdict {
  if (!report || experiments.length === 0) return "not-tested";
  const verdicts = experiments.map((id) => report.episodes[id]?.verdict ?? "not-tested");
  for (const verdict of ["does-not-hold", "not-tested", "holds-with-caveat"] as const) {
    if (verdicts.includes(verdict)) return verdict;
  }
  return "holds";
}

export function systemCardKicker(report: ReleaseReport | null) {
  return report?.recordedAt ? `Recorded ${report.recordedAt}` : "Measured run pending";
}

export function MeasuredClaims({ release, report, previous, assessments }: {
  release: string; report: ReleaseReport | null; previous?: string; assessments: Record<Exclude<ClaimId, "C1">, Assessment>;
}) {
  return <>
    <p className="release-scope-note">{report
      ? "Hubu makes six claims. The four experiments measure C1 against Hubu’s own ledger and records, not provider invoices; each result applies only to its stated setup. The other claims show their current status."
      : `Hubu makes six claims. The four experiments that measure C1 run only against a published release tag; their ${release} results will be recorded here after publication. Until then C1 is not tested, and earlier results are not carried forward.`}</p>
    <div className="release-claims release-claims-catalog">
      {catalog.claims.map((claim) => {
        const id = claim.id as ClaimId;
        const experiments = claim.experiments as EpisodeId[];
        const status: ClaimStatus = id === "C1" ? measuredStatus(report, experiments) : assessments[id].status;
        return <div className="release-claim" key={id} id={`claim-${id.toLowerCase()}`}>
          <div><h3><span className="release-claim-id">{id}</span> {claim.title}</h3><Status status={status} /></div>
          <p>{claim.invariant}</p>
          {id !== "C1" && <p className="release-claim-note">{assessments[id].note}</p>}
          {experiments.length > 0 && <ul className="release-claim-experiments">
            {experiments.map((experiment) => {
              const result = report?.episodes[experiment];
              return <li key={experiment}>
                <div>
                  <h4>{report ? <a href={`/releases/${release}/report#${experiment}`}>{experimentNames[experiment]} <span aria-hidden="true">↗</span></a> : experimentNames[experiment]}</h4>
                  <Status status={result?.verdict ?? "not-tested"} />
                </div>
                {result?.summary && <p>{result.summary}</p>}
                {result && conditions[experiment] && <p className="release-condition">{conditions[experiment]}</p>}
              </li>;
            })}
          </ul>}
        </div>;
      })}
    </div>
    {report
      ? <div className="release-analysis-link"><div><h3>Full system analysis</h3><p>Inspect the setup, scripts, measured charts, captured output, raw evidence, and known identity gap.</p></div><a className="button primary" href={`/releases/${release}/report`}>Read the analysis <span aria-hidden="true">↗</span></a></div>
      : previous && <div className="release-analysis-link"><div><h3>Previous measured analysis</h3><p>The {previous} analysis applies only to {previous}’s released source. It shows the setup, scripts, and evidence the {release} run will repeat.</p></div><a className="button secondary" href={`/releases/${previous}/report`}>Read the {previous} analysis <span aria-hidden="true">↗</span></a></div>}
  </>;
}
