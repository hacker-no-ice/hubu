import { EngineeringHeader } from "../components/EngineeringHeader";
import { documentMetadata } from "../lib/metadata";

export const metadata = documentMetadata({
  title: "Release notes", href: "/releases",
  description: "Stable Hubu releases: what changed, upgrade notes, and the evidence and limits behind each system analysis.",
});

export default function Releases() {
  return <div className="engineering-shell">
    <EngineeringHeader section="releases" />
    <main id="main-content" className="engineering-index proof-index">
      <header className="engineering-index-heading">
        <p className="eyebrow"><span /> Hubu releases</p>
        <h1>Release notes.</h1>
        <p>What changed, what to know before upgrading, and what the recorded evidence supports.</p>
      </header>
      <a className="proof-release-link" href="/releases/v0.2.4">
        <div><span className="proof-kicker">Stable · October 19, 2026</span><h2>v0.2.4</h2></div>
        <div><p>Exact amounts, governance evidence, USDC budgets, and an updated executor contract.</p><span className="proof-text-link">Read release notes and system card <span aria-hidden="true">↗</span></span></div>
      </a>
      <p className="proof-index-note"><a className="proof-text-link" href="https://github.com/hacker-no-ice/hubu/releases">View the complete release history on GitHub ↗</a></p>
    </main>
  </div>;
}
