import { assessmentFor, catalogFor, type Challenge } from "../assessments";
import type { ReleaseReport, Verdict } from "../report-types";

// Hubu's claims are judged by their challenges. A challenge's status comes
// from the release's measured report; an unmeasured challenge can only be a
// documented known gap (site/app/releases/assessments.ts).
//
// Claim tiers: any failed challenge → Does not hold; otherwise every in-scope
// challenge holds → Verified, some → Partially verified, none → Unverified.
// Out-of-scope and not-applicable challenges are excluded. Untested, planned
// and blocked ones count as not holding. break-hubu/render-report.py applies
// the same rule.
//
// The release notes show only the claims (MeasuredClaims); every challenge,
// result and reason lives in the analysis (ClaimDetails).
export type ChallengeStatus = Verdict | "known-gap" | "out-of-scope" | "not-applicable";
export type ClaimTier = "verified" | "partially-verified" | "unverified" | "does-not-hold";

const tierLabels: Record<ClaimTier, string> = {
  verified: "Verified", "partially-verified": "Partially verified", unverified: "Unverified", "does-not-hold": "Does not hold",
};
const tierStyles: Record<ClaimTier, string> = {
  verified: "holds", "partially-verified": "holds-with-caveat", unverified: "not-tested", "does-not-hold": "does-not-hold",
};
const statusLabels: Record<ChallengeStatus, string> = {
  holds: "Holds", "holds-with-caveat": "Holds with caveat", "does-not-hold": "Does not hold", "not-tested": "Not tested",
  "known-gap": "Known gap", "out-of-scope": "Out of scope", "not-applicable": "Not applicable",
};
const statusStyles: Record<ChallengeStatus, string> = {
  holds: "holds", "holds-with-caveat": "holds-with-caveat", "does-not-hold": "does-not-hold", "not-tested": "not-tested",
  "known-gap": "does-not-hold", "out-of-scope": "not-tested", "not-applicable": "not-tested",
};
const reasonPrefix: Record<string, string> = { "out-of-scope": "Out of scope: ", blocked: "Blocked: ", planned: "Planned: " };

export function challengeStatus(challenge: Challenge, report: ReleaseReport | null, documented: Record<string, "known-gap">): ChallengeStatus {
  if (challenge.status === "out-of-scope" || challenge.status === "not-applicable") return challenge.status;
  const measured = challenge.kit ? report?.episodes[challenge.kit] : undefined;
  return measured?.verdict ?? documented[challenge.id] ?? "not-tested";
}

export function claimTier(statuses: ChallengeStatus[]) {
  const inScope = statuses.filter((status) => status !== "out-of-scope" && status !== "not-applicable");
  const held = inScope.filter((status) => status === "holds" || status === "holds-with-caveat").length;
  const failed = inScope.filter((status) => status === "does-not-hold" || status === "known-gap").length;
  const tier: ClaimTier = failed ? "does-not-hold" : inScope.length && held === inScope.length ? "verified" : held ? "partially-verified" : "unverified";
  const outOfScope = statuses.filter((status) => status === "out-of-scope").length;
  const notApplicable = statuses.length - inScope.length - outOfScope;
  return { tier, held, total: inScope.length, failed, outOfScope, notApplicable };
}

function evaluate(release: string, report: ReleaseReport | null) {
  const { documented, notes } = assessmentFor(release);
  const { claims, challenges, kits } = catalogFor(release);
  const results = claims.map((claim) => {
    const own = challenges.filter((challenge) => challenge.claim === claim.id);
    const statuses = own.map((challenge) => challengeStatus(challenge, report, documented));
    return { claim, id: claim.id, own, statuses, note: notes[claim.id], ...claimTier(statuses) };
  });
  return { results, kits };
}

function countText(held: number, total: number, failed: number) {
  return `${held} of ${total} challenges hold${failed ? ` · ${failed} ${failed === 1 ? "fails" : "fail"}` : ""}`;
}

function TierLabel({ tier }: { tier: ClaimTier }) {
  return <span className={`proof-verdict proof-verdict-${tierStyles[tier]}`} data-tier={tier}><span aria-hidden="true" />{tierLabels[tier]}</span>;
}

export function systemCardKicker(report: ReleaseReport | null) {
  return report?.recordedAt ? `Recorded ${report.recordedAt}` : "Measured run pending";
}

// Release notes: the claims and their results, linking into the analysis.
export function MeasuredClaims({ release, report, previous }: { release: string; report: ReleaseReport | null; previous?: string }) {
  const { results } = evaluate(release, report);
  return <>
    <p className="release-scope-note">{report
      ? "Each claim is judged by its challenges: Verified when all hold, Partially verified when some do, Unverified when none have been shown, and Does not hold when any fails."
      : `Each claim is judged by its challenges, measured only against a published release tag. Until the ${release} run is recorded, no challenge counts as holding.`}</p>
    <ul className="release-claim-summary">
      {results.map(({ id, claim, tier, held, total, failed }) => <li key={id} id={`claim-${id.toLowerCase()}`}>
        {report
          ? <a href={`/releases/${release}/report#claim-${id.toLowerCase()}`}><span className="release-claim-id">{id}</span>{claim.title}</a>
          : <span><span className="release-claim-id">{id}</span>{claim.title}</span>}
        <span className="release-claim-count">{countText(held, total, failed)}</span>
        <TierLabel tier={tier} />
      </li>)}
    </ul>
    {report
      ? <div className="release-analysis-link"><div><h3>Full system analysis</h3><p>Every challenge behind these claims, with its setup, scripts, result and limits.</p></div><a className="button primary" href={`/releases/${release}/report`}>Read the analysis <span aria-hidden="true">↗</span></a></div>
      : previous && <div className="release-analysis-link"><div><h3>Previous measured analysis</h3><p>The {previous} analysis applies only to {previous}’s released source. It shows the challenges the {release} run will repeat.</p></div><a className="button secondary" href={`/releases/${previous}/report`}>Read the {previous} analysis <span aria-hidden="true">↗</span></a></div>}
  </>;
}

// Analysis: every claim with its challenges, results and reasons.
export function ClaimDetails({ release, report }: { release: string; report: ReleaseReport }) {
  const { results, kits } = evaluate(release, report);
  return <div className="claim-details">
    {results.map(({ id, claim, own, statuses, note, tier, held, total, failed, outOfScope, notApplicable }) => (
      <section className="claim-detail" key={id} id={`claim-${id.toLowerCase()}`} aria-labelledby={`claim-${id.toLowerCase()}-title`}>
        <header>
          <h3 id={`claim-${id.toLowerCase()}-title`}><span className="release-claim-id">{id}</span>{claim.title}</h3>
          <TierLabel tier={tier} />
        </header>
        <p className="release-claim-count">{countText(held, total, failed)}{outOfScope ? ` · ${outOfScope} out of scope` : ""}{notApplicable ? ` · ${notApplicable} not applicable` : ""}</p>
        <p>{claim.invariant}</p>
        {note && <p className="claim-detail-note">{note}</p>}
        <table className="claim-challenges"><tbody>
          {own.map((challenge, index) => {
            const status = statuses[index];
            const result = challenge.kit ? report.episodes[challenge.kit] : undefined;
            const kit = result && challenge.kit ? kits[challenge.kit] : undefined;
            return <tr key={challenge.id}>
              <th scope="row"><span className="release-challenge-id">{challenge.id}</span>{kit ? `${challenge.title} (EP ${kit.number})` : challenge.title}</th>
              <td><span className={`proof-verdict proof-verdict-${statusStyles[status]}`} data-status={status}><span aria-hidden="true" />{statusLabels[status]}</span></td>
              <td>
                {result ? result.summary : challenge.reason ? `${reasonPrefix[challenge.status] ?? ""}${challenge.reason}` : status === "known-gap" ? "Documented known gap; not measured for this release." : `Not measured for ${release}.`}
                {kit?.condition && <span className="release-condition">{kit.condition}</span>}
              </td>
            </tr>;
          })}
        </tbody></table>
      </section>
    ))}
  </div>;
}
