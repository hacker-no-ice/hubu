import { EngineeringHeader } from "../../components/EngineeringHeader";
import { documentMetadata } from "../../lib/metadata";

export const metadata = documentMetadata({
  title: "v0.2.5 release notes", href: "/releases/v0.2.5",
  description: "Hubu v0.2.5: provider packs, a larger image catalog, image editing, model and size policy tiers, saved artifacts, and a local admin web UI, with upgrade guidance and a compact system card.",
});

const repository = "https://github.com/hacker-no-ice/hubu";
// The Break Hubu kits run only against a published release tag. Until the
// v0.2.5 run is recorded, every claim stays "Not tested"; v0.2.4 verdicts are
// never carried forward.
const claims: { title: string; question: string }[] = [
  { title: "Budget exhaustion", question: "Does Hubu stop authorizing new work once a fixed budget is consumed?" },
  { title: "Overrun accounting", question: "Is a confirmed charge above its authorization recorded in full, with the overrun visible?" },
  { title: "Crash recovery", question: "Does a restart preserve an unresolved claim’s financial obligation?" },
  { title: "Shared-budget burst", question: "Can many clients sharing one budget reserve more than that budget?" },
];

export default function ReleaseNotes() {
  return <div className="engineering-shell">
    <EngineeringHeader section="releases" />
    <main className="engineering-article release-overview" id="main-content">
      <article className="engineering-article-inner">
        <header className="release-heading">
          <a className="engineering-back" href="/releases">← Release notes</a>
          <p className="eyebrow"><span /> Stable release · November 5, 2026</p>
          <h1>Hubu v0.2.5</h1>
          <p className="engineering-deck">Provider packs and a governed creative workflow.</p>
          <div className="release-links"><a href={`${repository}/releases/tag/v0.2.5`}>Release on GitHub ↗</a><a href={`${repository}/blob/main/CHANGELOG.md#v025--2026-11-05`}>Complete changelog ↗</a></div>
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
          <div className="release-section-heading"><h2 id="system-card-title">System card</h2><span className="proof-kicker">Measured run pending</span></div>
          <dl className="release-system-card">
            <div><dt>Tested system</dt><dd>Hubu remains experimental and local-first. Release validation runs formatting, lints, locked workspace tests, integration and packaging checks, and published-asset smoke tests. It never supplies provider credentials or makes live provider calls.</dd></div>
            <div><dt>Provider coverage</dt><dd>New packs and targets are verified in CI with recorded fixtures and mock transports. Prices come from each provider’s published rates on the review dates stated in the contract docs; provider invoices may differ. Redaction attestation covers FLUX.2 [pro] v1 only.</dd></div>
            <div><dt>Known authority limits</dt><dd>Registered agent accounts are not bound to an authenticated runtime or thread. The normal bearer can change budgets and policies. <code>hubu ui</code> holds the bearer and approval credentials in a local process protected by loopback binding, a one-time login link, and same-origin checks, not per-user accounts.</dd></div>
            <div><dt>Reproduction</dt><dd>The release experiment kits build the exact published release and need no model subscription or provider credentials. <a href={`${repository}/blob/main/break-hubu/README.md`}>Prerequisites and commands ↗</a></dd></div>
          </dl>
          <p className="release-scope-note">The four ledger experiments run only against a published release tag. Their v0.2.5 results will be recorded here after publication. Until then each claim is not tested, and earlier results are not carried forward.</p>
          <div className="release-claims">
            {claims.map((claim) => <div className="release-claim" key={claim.title}>
              <div><h3>{claim.title}</h3><span className="proof-verdict proof-verdict-not-tested"><span aria-hidden="true" />Not tested</span></div>
              <p>{claim.question}</p>
            </div>)}
          </div>
          <div className="release-analysis-link"><div><h3>Previous measured analysis</h3><p>The v0.2.4 analysis applies only to v0.2.4’s released source. It shows the setup, scripts, and evidence the v0.2.5 run will repeat.</p></div><a className="button secondary" href="/releases/v0.2.4/report">Read the v0.2.4 analysis <span aria-hidden="true">↗</span></a></div>
        </section>
        <footer className="engineering-article-footer proof-footer"><a href="/releases">← All release notes</a><a href={`${repository}/releases/tag/v0.2.5`}>Release on GitHub ↗</a></footer>
      </article>
    </main>
  </div>;
}
