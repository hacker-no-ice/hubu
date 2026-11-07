import catalog from "../claims.json";
import type { ReleaseReport, Verdict } from "../report-types";

// The release-notes system card: Hubu's six claims, each judged by its
// challenges. A challenge's status comes from the release's measured report;
// an unmeasured challenge can only be marked a documented known gap.
//
// Claim tiers: any failed challenge → Does not hold; otherwise every in-scope
// challenge holds → Verified, some → Partially verified, none → Unverified.
// Out-of-scope challenges never count; untested, planned and blocked ones count
// as not holding. break-hubu/render-report.py applies the same rule.
export type ClaimId = "C1" | "C2" | "C3" | "C4" | "C5" | "C6";
export type ChallengeStatus = Verdict | "known-gap" | "out-of-scope";
export type ClaimTier = "verified" | "partially-verified" | "unverified" | "does-not-hold";
type Challenge = { id: string; claim: string; title: string; status: string; kit?: string; reason?: string };

const challenges = catalog.challenges as Challenge[];
const kitText = catalog.kits as Record<string, { short: string; condition?: string }>;
const tierLabels: Record<ClaimTier, string> = {
  verified: "Verified", "partially-verified": "Partially verified", unverified: "Unverified", "does-not-hold": "Does not hold",
};
const tierStyles: Record<ClaimTier, string> = {
  verified: "holds", "partially-verified": "holds-with-caveat", unverified: "not-tested", "does-not-hold": "does-not-hold",
};
const statusLabels: Record<ChallengeStatus, string> = {
  holds: "Holds", "holds-with-caveat": "Holds with caveat", "does-not-hold": "Does not hold", "not-tested": "Not tested",
  "known-gap": "Known gap", "out-of-scope": "Out of scope",
};
const statusStyles: Record<ChallengeStatus, string> = {
  holds: "holds", "holds-with-caveat": "holds-with-caveat", "does-not-hold": "does-not-hold", "not-tested": "not-tested",
  "known-gap": "does-not-hold", "out-of-scope": "not-tested",
};

export function challengeStatus(challenge: Challenge, report: ReleaseReport | null, documented: Record<string, "known-gap">): ChallengeStatus {
  if (challenge.status === "out-of-scope") return "out-of-scope";
  const measured = challenge.kit ? report?.episodes[challenge.kit] : undefined;
  return measured?.verdict ?? documented[challenge.id] ?? "not-tested";
}

export function claimTier(statuses: ChallengeStatus[]) {
  const inScope = statuses.filter((status) => status !== "out-of-scope");
  const held = inScope.filter((status) => status === "holds" || status === "holds-with-caveat").length;
  const failed = inScope.filter((status) => status === "does-not-hold" || status === "known-gap").length;
  const tier: ClaimTier = failed ? "does-not-hold" : inScope.length && held === inScope.length ? "verified" : held ? "partially-verified" : "unverified";
  return { tier, held, total: inScope.length, failed, outOfScope: statuses.length - inScope.length };
}

export function systemCardKicker(report: ReleaseReport | null) {
  return report?.recordedAt ? `Recorded ${report.recordedAt}` : "Measured run pending";
}

export function MeasuredClaims({ release, report, previous, notes = {}, documented = {} }: {
  release: string; report: ReleaseReport | null; previous?: string;
  notes?: Partial<Record<ClaimId, string>>; documented?: Record<string, "known-gap">;
}) {
  return <>
    <p className="release-scope-note">{report
      ? "Hubu makes six claims, each judged by its challenges: Verified when every in-scope challenge holds, Partially verified when some do, Unverified when none have been shown, and Does not hold when any challenge fails. Results concern Hubu’s own ledger and records, not provider invoices, and apply only to their stated setup."
      : `Hubu makes six claims, each judged by its challenges. The experiments run only against a published release tag; ${release} results will be recorded here after publication. Until then no challenge is counted as holding, and earlier results are not carried forward.`}</p>
    <div className="release-claims release-claims-catalog">
      {catalog.claims.map((claim) => {
        const id = claim.id as ClaimId;
        const own = challenges.filter((challenge) => challenge.claim === id);
        const statuses = own.map((challenge) => challengeStatus(challenge, report, documented));
        const summary = claimTier(statuses);
        return <div className="release-claim" key={id} id={`claim-${id.toLowerCase()}`}>
          <div>
            <h3><span className="release-claim-id">{id}</span> {claim.title}</h3>
            <span className={`proof-verdict proof-verdict-${tierStyles[summary.tier]}`} data-tier={summary.tier}><span aria-hidden="true" />{tierLabels[summary.tier]}</span>
          </div>
          <p className="release-claim-count">{`${summary.held} of ${summary.total} challenges hold${summary.failed ? ` · ${summary.failed} ${summary.failed === 1 ? "fails" : "fail"}` : ""}${summary.outOfScope ? ` · ${summary.outOfScope} out of scope` : ""}`}</p>
          <p>{claim.invariant}</p>
          {notes[id] && <p className="release-claim-note">{notes[id]}</p>}
          <ul className="release-claim-experiments">
            {own.map((challenge, index) => {
              const status = statuses[index];
              const result = challenge.kit ? report?.episodes[challenge.kit] : undefined;
              const name = <><span className="release-challenge-id">{challenge.id}</span> {challenge.title}</>;
              return <li key={challenge.id} id={`challenge-${challenge.id.toLowerCase().replace(".", "-")}`}>
                <div>
                  <h4>{result ? <a href={`/releases/${release}/report#${challenge.kit}`}>{name} <span aria-hidden="true">↗</span></a> : name}</h4>
                  <span className={`proof-verdict proof-verdict-${statusStyles[status]}`} data-status={status}><span aria-hidden="true" />{statusLabels[status]}</span>
                </div>
                {result?.summary && <p>{result.summary}</p>}
                {result && challenge.kit && kitText[challenge.kit]?.condition && <p className="release-condition">{kitText[challenge.kit].condition}</p>}
                {!result && challenge.reason && <p className="release-challenge-reason">{challenge.status === "out-of-scope" ? "Out of scope: " : challenge.status === "blocked" ? "Blocked: " : challenge.status === "planned" ? "Planned: " : ""}{challenge.reason}</p>}
                {!result && !challenge.reason && challenge.kit && status !== "known-gap" && <p className="release-challenge-reason">Not measured for {release}.</p>}
              </li>;
            })}
          </ul>
        </div>;
      })}
    </div>
    {report
      ? <div className="release-analysis-link"><div><h3>Full system analysis</h3><p>Inspect the setup, scripts, captured output and raw evidence behind every measured challenge.</p></div><a className="button primary" href={`/releases/${release}/report`}>Read the analysis <span aria-hidden="true">↗</span></a></div>
      : previous && <div className="release-analysis-link"><div><h3>Previous measured analysis</h3><p>The {previous} analysis applies only to {previous}’s released source. It shows the setup, scripts, and evidence the {release} run will repeat.</p></div><a className="button secondary" href={`/releases/${previous}/report`}>Read the {previous} analysis <span aria-hidden="true">↗</span></a></div>}
  </>;
}
