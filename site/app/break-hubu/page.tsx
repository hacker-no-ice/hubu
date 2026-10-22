import { EngineeringHeader } from "../components/EngineeringHeader";
import { documentMetadata } from "../lib/metadata";
import { report } from "./v0.2.4/report";

export const metadata = documentMetadata({
  title: "Break Hubu", href: "/break-hubu",
  description: "Reproducible experiments against released Hubu: the setup, scripts, public evidence, outcomes, and limits of each claim.",
});

export default function ProofIndex() {
  return <div className="engineering-shell">
    <EngineeringHeader section="break-hubu" />
    <main id="main-content" className="engineering-index proof-index">
      <header className="engineering-index-heading">
        <p className="eyebrow"><span /> Public proof kits</p>
        <h1>Break Hubu.</h1>
        <p>Put the claims under pressure. Inspect the setup, read the evidence, and run the same experiment yourself.</p>
      </header>
      <a className="proof-release-link" href="/break-hubu/v0.2.4">
        <div><span className="proof-kicker">Release card · {report.release}</span><h2>Four experiments.<br />One known identity gap.</h2></div>
        <div><p>Runaway requests, an overrun on the last cent, a process crash, and ten clients sharing a budget.</p><span className="proof-text-link">{report.recordedAt ? "Read the evidence" : "View the experiment design"} <span aria-hidden="true">↗</span></span></div>
      </a>
      <p className="proof-index-note">All claims are about Hubu’s own ledger and records, not provider invoices.</p>
    </main>
  </div>;
}
