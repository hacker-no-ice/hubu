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
            <div><dt>Tested</dt><dd>Released CLI and HTTP API, fresh SQLite and a synthetic executor. No live provider calls.</dd></div>
            <div><dt>Not covered</dt><dd>Gongbu, MCP and real provider billing. New provider packs are fixture-tested in CI.</dd></div>
            <div><dt>Known gaps</dt><dd>Agent accounts aren’t bound to a runtime (C3); the normal bearer can change budgets and policies (C5).</dd></div>
            <div><dt>Reproduce</dt><dd>No model subscription or provider credentials needed. <a href={`${repository}/blob/main/break-hubu/README.md`}>Commands ↗</a></dd></div>
          </dl>
          <MeasuredClaims release="v0.2.5" report={report} previous="v0.2.4" />
        </section>
        <footer className="engineering-article-footer proof-footer"><a href="/releases">← All release notes</a><a href={`${repository}/releases/tag/v0.2.5`}>Release on GitHub ↗</a></footer>
      </article>
    </main>
  </div>;
}
