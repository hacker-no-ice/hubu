import { EngineeringHeader } from "../../components/EngineeringHeader";
import { documentMetadata } from "../../lib/metadata";
import { MeasuredClaims, systemCardKicker } from "../components/MeasuredClaims";
import { report } from "./report";

export const metadata = documentMetadata({
  title: "v0.2.5 release notes", href: "/releases/v0.2.5",
  description: "Hubu v0.2.5: provider packs, a larger image catalog, image editing, model and size policy tiers, saved artifacts, and a local admin web UI, with upgrade guidance and a compact system card.",
});

const repository = "https://github.com/hacker-no-ice/hubu";
export default function ReleaseNotes() {
  return <div className="engineering-shell">
    <EngineeringHeader section="releases" />
    <main className="engineering-article release-overview" id="main-content">
      <article className="engineering-article-inner">
        <header className="release-heading">
          <a className="engineering-back" href="/releases">← Release notes</a>
          <p className="eyebrow"><span /> Stable release · November 6, 2026</p>
          <h1>Hubu v0.2.5</h1>
          <p className="engineering-deck">Provider packs and a governed creative workflow.</p>
          <div className="release-links"><a href={`${repository}/releases/tag/v0.2.5`}>Release on GitHub ↗</a><a href={`${repository}/blob/main/CHANGELOG.md#v025--2026-11-06`}>Complete changelog ↗</a></div>
        </header>

        <section className="release-section" aria-labelledby="changes-title">
          <h2 id="changes-title">What changed</h2>
          <ul className="release-highlights">
            <li><strong>Provider packs.</strong> Every shipped integration runs from a verified TOML <a href="/docs/provider-packs">provider pack</a> executed by one of two reviewed engines. Engines, not packs, own retries, settlement, reconciliation, and credential hosts. The Gemini and FLUX.2 Rust adapters are gone.</li>
            <li><strong>A larger image catalog.</strong> FLUX.2 [pro] v2 with aspect ratios and per-megapixel pricing, FLUX.2 [max], [flex], and [klein], FLUX 3 Image and FLUX 3 Image Klein, Gemini aspect ratios, Gemini 3 Pro Image, and Gemini Nano Banana 2.1. Earlier contract versions stay available unchanged.</li>
            <li><strong>Image editing.</strong> Every shipped image model can edit 1–4 input images, given as earlier governed outputs or as the operator’s own reference images pinned by digest.</li>
            <li><strong>Model and size tiers.</strong> Policies can allow draft models and sizes while escalating final renders. Gongbu binds the authorized model and size to the execution it runs.</li>
            <li><strong>Saved artifacts.</strong> Agents save a chosen output into a human-allowlisted folder, with a receipt recording its digest and exact settlement.</li>
            <li><strong>Admin web UI.</strong> <code>hubu ui</code> serves a loopback-only browser UI for agents, policies, budgets, approvals, spend, and decisions, with a spend summary by platform, agent, and day.</li>
          </ul>
          <p className="proof-caveat"><strong>Before upgrading:</strong> drain unfinished Gongbu work, including executions awaiting reconciliation, then re-render the stack. Read the <a href="/docs/operations/upgrading-to-v0.2.5">upgrade guide</a>. Gongbu refuses to start while unfinished work references a replaced provider target revision. Upgrade Hubu and Gongbu together.</p>
        </section>

        <section className="release-section" aria-labelledby="system-card-title">
          <div className="release-section-heading"><h2 id="system-card-title">System card</h2><span className="proof-kicker">{systemCardKicker(report)}</span></div>
          <dl className="release-system-card">
            <div><dt>Tested system</dt><dd>Hubu remains experimental and local-first. Release validation runs formatting, lints, locked workspace tests, integration and packaging checks, and published-asset smoke tests. It never supplies provider credentials or makes live provider calls.</dd></div>
            <div><dt>Provider coverage</dt><dd>New packs and targets are verified in CI with recorded fixtures and mock transports. Prices come from each provider’s published rates on the review dates stated in the contract docs; provider invoices may differ. Redaction attestation covers FLUX.2 [pro] v1 only.</dd></div>
            <div><dt>Known authority limits</dt><dd>Registered agent accounts are not bound to an authenticated runtime or thread. The normal bearer can change budgets and policies. <code>hubu ui</code> holds the bearer and approval credentials in a local process protected by loopback binding, a one-time login link, and same-origin checks, not per-user accounts.</dd></div>
            <div><dt>Reproduction</dt><dd>The release experiment kits build the exact published release and need no model subscription or provider credentials. <a href={`${repository}/blob/main/break-hubu/README.md`}>Prerequisites and commands ↗</a></dd></div>
          </dl>
          <MeasuredClaims release="v0.2.5" report={report} previous="v0.2.4" assessments={{
            C2: { status: "not-tested", note: "Policy denial and human approval run through Hubu and Gongbu, but no system-card experiment measures them yet." },
            C3: { status: "known-gap", note: "Registered agent accounts are not bound to an authenticated runtime or thread. Strong binding is planned for a future release." },
            C4: { status: "not-tested", note: "The experiments use a synthetic executor, so Gongbu’s credential custody is outside this analysis." },
            C5: { status: "known-gap", note: "The normal bearer can change budgets and policies. hubu ui holds the bearer and approval credentials in a local process, not per-user accounts." },
            C6: { status: "not-tested", note: "Only verified packs from the repository ship in v0.2.5, and pack validation rejects packs that weaken engine-owned settlement or credential hosts. Self-serve packs are not available, and no system-card experiment covers this yet." },
          }} />
        </section>
        <footer className="engineering-article-footer proof-footer"><a href="/releases">← All release notes</a><a href={`${repository}/releases/tag/v0.2.5`}>Release on GitHub ↗</a></footer>
      </article>
    </main>
  </div>;
}
