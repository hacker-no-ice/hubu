import { EngineeringHeader } from "../../components/EngineeringHeader";
import { documentMetadata } from "../../lib/metadata";
import { MeasuredClaims, systemCardKicker } from "../components/MeasuredClaims";
import { report } from "./report";

export const metadata = documentMetadata({
  title: "v0.2.4 release notes", href: "/releases/v0.2.4",
  description: "Hubu v0.2.4: exact amounts, governance evidence, USDC budgets, upgrade guidance, and a compact system card with links to the full analysis.",
});

const repository = "https://github.com/hacker-no-ice/hubu";
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
          <div className="release-section-heading"><h2 id="system-card-title">System card</h2><span className="proof-kicker">{systemCardKicker(report)}</span></div>
          <dl className="release-system-card">
            <div><dt>Tested</dt><dd>Released CLI and HTTP API, fresh SQLite, a synthetic executor and fixed USD budgets.</dd></div>
            <div><dt>Not covered</dt><dd>USDC, governance queries, Gongbu, MCP and real provider billing.</dd></div>
            <div><dt>Known gaps</dt><dd>Agent accounts aren’t bound to a runtime (C3); the normal bearer can change budgets and policies (C5).</dd></div>
            <div><dt>Reproduce</dt><dd>macOS, the released source, no provider credentials. <a href="/releases/v0.2.4/report#reproduce">Commands ↗</a></dd></div>
          </dl>
          <MeasuredClaims release="v0.2.4" report={report} />
        </section>
        <footer className="engineering-article-footer proof-footer"><a href="/releases">← All release notes</a><a href={`${repository}/tree/${report.sourceCommit}`}>Released source · {report.sourceCommit.slice(0, 8)} ↗</a></footer>
      </article>
    </main>
  </div>;
}
