import { EngineeringHeader } from "../../components/EngineeringHeader";
import { documentMetadata } from "../../lib/metadata";
import { report, type EpisodeId, type Verdict } from "./report";

export const metadata = documentMetadata({
  title: "v0.2.4 release notes", href: "/releases/v0.2.4",
  description: "Hubu v0.2.4: exact amounts, governance evidence, USDC budgets, upgrade guidance, and a compact system card with links to the full analysis.",
});

const repository = "https://github.com/hacker-no-ice/hubu";
const verdicts: Record<Verdict, string> = {
  holds: "Holds", "holds-with-caveat": "Holds with caveat", "does-not-hold": "Does not hold", "not-tested": "Not tested",
};
const claims: { id: EpisodeId; title: string; condition?: string }[] = [
  { id: "ep01-runaway", title: "Budget exhaustion" },
  { id: "ep02-last-cent", title: "Overrun accounting", condition: "Requires explicit owner reconciliation after lease expiry." },
  { id: "ep03-crash-recovery", title: "Crash recovery" },
  { id: "ep04-many-clients", title: "Shared-budget burst", condition: "The released HTTP server handles requests serially. This does not establish behavior under parallel server execution." },
];

export default function ReleaseNotes() {
  return <div className="engineering-shell">
    <EngineeringHeader section="releases" />
    <main className="engineering-article release-overview" id="main-content">
      <article className="engineering-article-inner">
        <header className="release-heading">
          <a className="engineering-back" href="/releases">← Release notes</a>
          <p className="eyebrow"><span /> Stable release · October 19, 2026</p>
          <h1>Hubu v0.2.4</h1>
          <p className="engineering-deck">Exact amounts and clearer governance evidence.</p>
          <div className="release-links"><a href={`${repository}/releases/tag/v0.2.4`}>Release on GitHub ↗</a><a href={`${repository}/blob/main/CHANGELOG.md#v024--2026-10-19`}>Complete changelog ↗</a></div>
        </header>

        <section className="release-section" aria-labelledby="changes-title">
          <h2 id="changes-title">What changed</h2>
          <ul className="release-highlights">
            <li><strong>Governance history.</strong> Append-only records and read-only HTTP/MCP queries expose policy decisions, human resolutions, authorizations, and financial transitions, with explicit coverage limits.</li>
            <li><strong>Exact amounts.</strong> Budgets, policies, API calls, and the CLI support up to six decimal places. New budget charges use micro-units; historical ledger entries retain their original charges.</li>
            <li><strong>USDC budgets.</strong> USD and USDC are accounted separately, without conversion. Wallet rails remain USD-only and reject USDC payments.</li>
            <li><strong>Executor contract v4.5.</strong> Hubu and Gongbu use exact amounts for quotes, authorization, claims, and settlement. Legacy whole-cent requests remain supported for this release.</li>
          </ul>
          <p className="proof-caveat"><strong>Before upgrading:</strong> back up Hubu and Gongbu state and read the <a href="/docs/operations/upgrading-to-v0.2.4">upgrade guide</a>. Storage migrations are one-way; restore backups instead of downgrading in place. Hand-written Gongbu configurations must use executor contract v4.5.</p>
        </section>

        <section className="release-section" aria-labelledby="system-card-title">
          <div className="release-section-heading"><h2 id="system-card-title">System card</h2><span className="proof-kicker">Recorded {report.recordedAt}</span></div>
          <dl className="release-system-card">
            <div><dt>Tested system</dt><dd>Hubu remains experimental and local-first. This run uses the released CLI and HTTP API, fresh SQLite, a synthetic external executor, and fixed USD budgets and policies.</dd></div>
            <div><dt>Outside this analysis</dt><dd>USDC, governance-query coverage, Gongbu, Temporal, MCP transport, provider adapters, and actual provider billing. The four experiments do not qualify every new release feature.</dd></div>
            <div><dt>Known authority limits</dt><dd>Registered agent accounts are not bound to an authenticated runtime or thread. The normal bearer can change budgets and policies. These known gaps are planned for future releases.</dd></div>
            <div><dt>Reproduction</dt><dd>Recorded on macOS with the pinned released source and public test scripts. No model subscription or provider credentials required. <a href="/releases/v0.2.4/report#reproduce">Setup and commands ↗</a></dd></div>
          </dl>
          <p className="release-scope-note">These findings concern Hubu’s own ledger and records, not provider invoices. Each result applies only to its stated setup.</p>
          <div className="release-claims">
            {claims.map((claim) => {
              const result = report.episodes[claim.id];
              return <div className="release-claim" key={claim.id}>
                <div><h3><a href={`/releases/v0.2.4/report#${claim.id}`}>{claim.title} <span aria-hidden="true">↗</span></a></h3><span className={`proof-verdict proof-verdict-${result.verdict}`}><span aria-hidden="true" />{verdicts[result.verdict]}</span></div>
                <p>{result.summary}</p>
                {claim.condition && <p className="release-condition">{claim.condition}</p>}
              </div>;
            })}
          </div>
          <div className="release-analysis-link"><div><h3>Full system analysis</h3><p>Inspect the setup, scripts, measured charts, captured output, raw evidence, and known identity gap.</p></div><a className="button primary" href="/releases/v0.2.4/report">Read the analysis <span aria-hidden="true">↗</span></a></div>
        </section>
        <footer className="engineering-article-footer proof-footer"><a href="/releases">← All release notes</a><a href={`${repository}/tree/${report.sourceCommit}`}>Released source · {report.sourceCommit.slice(0, 8)} ↗</a></footer>
      </article>
    </main>
  </div>;
}
