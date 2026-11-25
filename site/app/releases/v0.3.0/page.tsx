import { EngineeringHeader } from "../../components/EngineeringHeader";
import { documentMetadata } from "../../lib/metadata";
import { MeasuredClaims, systemCardKicker } from "../components/MeasuredClaims";
import { report } from "./report";

const release = "v0.3.0";
const repository = "https://github.com/hacker-no-ice/hubu";

export const metadata = documentMetadata({
  title: `${release} release notes`,
  href: `/releases/${release}`,
  description: "Separate agent and owner credentials, confirm owner actions on macOS, and upgrade existing Hubu and Gongbu profiles.",
});

export default function ReleaseNotes() {
  return <div className="engineering-shell">
    <EngineeringHeader section="releases" />
    <main className="engineering-article release-overview" id="main-content">
      <article className="engineering-article-inner">
        <header className="release-heading">
          <a className="engineering-back" href="/releases">← Release notes</a>
          <p className="eyebrow"><time dateTime="2026-11-25">November 25, 2026</time></p>
          <h1>{`Hubu ${release}`}</h1>
          <p className="engineering-deck">Separate agent access from owner approval.</p>
          <div className="release-links">
            {report && <a href={`${repository}/releases/tag/${release}`}>Release on GitHub ↗</a>}
            <a href={`${repository}/blob/main/CHANGELOG.md`}>Complete changelog ↗</a>
          </div>
        </header>

        <section className="release-section" aria-labelledby="changes-title">
          <h2 id="changes-title">What changed</h2>
          <ul className="release-highlights">
            <li><strong>Agent access.</strong> The unified MCP uses a separate agent credential with an allowlist for spend requests, status, and reads.</li>
            <li><strong>Owner confirmation.</strong> Hubu asks the owner to confirm actions such as raising budgets, loosening policies, approving spend, and reconciling claims. The macOS dialog names the action and covers that action only.</li>
            <li><strong>Approval review.</strong> The Review request link opens the exact pending request in the signed-in portal. The Touch ID dialog asks for the owner’s approval and identifies the agent, spending limit, and execution target.</li>
            <li><strong>Policy review.</strong> Owners can see a policy diff before applying a change. Strict permissions are the default. The owner can explicitly delegate lower limits and new deny rules.</li>
            <li><strong>Governance history.</strong> Review confirmation outcomes and delegated actions alongside existing spend records.</li>
            <li><strong>Reconciliation recovery.</strong> Gongbu can close an execution that is awaiting reconciliation when recovery finds its matching finalized Hubu owner decision.</li>
          </ul>
          <p className="proof-caveat">Before upgrading, back up the v0.2.5 profile and upgrade Hubu and Gongbu together. Read the <a href="/docs/operations/upgrading-to-v0.3.0">upgrade guide</a>, the <a href="/docs/owner-presence">owner presence guide</a>, and the <a href="/docs/threat-model">threat model</a>.</p>
        </section>

        <section className="release-section" aria-labelledby="live-verification-title">
          <h2 id="live-verification-title">Live verification</h2>
          <dl className="release-system-card">
            <div><dt>Owner approval</dt><dd>Live macOS checks covered approval, denial, retry after timeout, and separate prompts for a policy change and a budget change. The owner also verified the revised confirmation text with a multiword agent name and denied the request.</dd></div>
            <div><dt>BFL usage cost</dt><dd>Black Forest Labs reported $0.0140 for one 2K FLUX 3 Image Klein request. Hubu settled $0.0140 from its frozen quote, a difference of $0.0000. The comparison used the provider’s usage export. Prepaid balance movement was unmeasured.</dd></div>
            <div><dt>Gemini</dt><dd>Live cost verification was deferred because the current image configurations lack a verified provider-enforced charge ceiling.</dd></div>
          </dl>
        </section>

        <section className="release-section" aria-labelledby="system-card-title">
          <div className="release-section-heading"><h2 id="system-card-title">System card</h2><span className="proof-kicker">{systemCardKicker(report)}</span></div>
          <dl className="release-system-card">
            <div><dt>Scope</dt><dd>Local CLI and HTTP authorization on macOS, with fresh Hubu state and a synthetic executor.</dd></div>
            <div><dt>Known gaps</dt><dd>Agent identities can be reused within the same macOS user. Executor and provider credential isolation and direct storage tampering remain follow-up work.</dd></div>
          </dl>
          <MeasuredClaims release={release} report={report} previous="v0.2.5" />
        </section>
        <footer className="engineering-article-footer proof-footer"><a href="/releases">← All release notes</a></footer>
      </article>
    </main>
  </div>;
}
