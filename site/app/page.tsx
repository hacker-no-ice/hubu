import type { Metadata } from "next";
import { HubuWordmark } from "./components/HubuWordmark";
import { HomeHeader } from "./components/home/HomeHeader";
import { Marty } from "./components/home/Marty";
import { StackBento } from "./components/home/StackBento";
import { StageSpotlight } from "./components/home/StageSpotlight";
import { policyExcerpt } from "./generated-home";
import { headerSearchIndex, navPanels, popularSearches } from "./lib/home";
import { homeMetadata } from "./lib/metadata";
import "./home.css";

const worksToday = [
  ["macOS", "source install from a release tag"],
  ["Codex", "setup helper; other MCP clients by manual config"],
  ["Gemini and the FLUX suite", "live providers: FLUX.2 [pro], [max], [flex], [klein] and FLUX 3 Image, with edit"],
  ["Live and sandbox modes", "live with approved real providers; sandbox with a non-billable fixture and no provider credentials"],
] as const;

export const metadata: Metadata = homeMetadata;

export default function Home() {
  return (
    <div className="home-shell">
      <HomeHeader panels={navPanels} searchIndex={headerSearchIndex} popular={popularSearches} />

      <main id="main-content">
        <div className="home-stage">
          <div className="stage-aurora" aria-hidden="true"><i /><i /><i /></div>
          <div className="stage-dots" aria-hidden="true" />
          <StageSpotlight />
          <section className="hero home-wrap">
            <div className="hero-copy">
              <div className="hero-branding rise">
                <HubuWordmark className="hero-wordmark" />
                <p className="eyebrow"><span /> Agent spend control plane</p>
              </div>
              <h1 className="rise" style={{ "--d": 2 } as React.CSSProperties}>Governed spend<br />for AI agents.</h1>
              <p className="hero-lede rise" style={{ "--d": 4 } as React.CSSProperties}>
                Hubu governs AI-agent spend. Gongbu executes only Hubu-authorized provider work.
                Agents get a budget, never your keys.
              </p>
              <div className="hero-actions rise" style={{ "--d": 5 } as React.CSSProperties}>
                <a className="button primary" href="/docs/local-stack">Start with the local stack <span>→</span></a>
                <a className="button secondary" href="/architecture/">Explore the architecture</a>
                <a className="button secondary" href="/demos">Watch a demo</a>
              </div>
            </div>
            <div className="boundary-wrap rise" style={{ "--d": 3 } as React.CSSProperties}>
              <Marty />
              <div className="boundary-card" aria-label="Hubu and Gongbu responsibility boundary">
                <div className="boundary-heading"><span>Control plane + executors</span></div>
                <div className="boundary-plane hubu-plane">
                  <p>CONTROL PLANE</p><h2>Hubu</h2>
                  <ul><li>Policy + budgets</li><li>Authorizations</li><li>Ledger + reconciliation</li></ul>
                </div>
                <div className="contract-line"><span>versioned executor contract</span></div>
                <div className="boundary-plane gongbu-plane">
                  <p>DEFAULT EXECUTOR</p><h2>Gongbu</h2>
                  <ul><li>Provider credentials</li><li>Temporal workflows</li><li>Artifacts + retries</li></ul>
                </div>
                <p className="boundary-note">
                  Two supported modes: <code>sandbox</code> and <code>live</code>. Both start Gongbu by default. Decisions-only is a future advanced concept and is not supported. <a href="/docs/external-executor">Advanced: custom executors →</a>
                </p>
              </div>
            </div>
          </section>
        </div>

        <StackBento policy={policyExcerpt} />

        <section className="works-today" aria-labelledby="works-today-title">
          <div className="home-wrap">
            <h2 id="works-today-title">Works today</h2>
            <ul>
              {worksToday.map(([label, detail]) => <li key={label}><strong>{label}</strong>{` ${detail}`}</li>)}
            </ul>
            <a href="/docs/overview#what-works-today">See what works today →</a>
          </div>
        </section>
      </main>

      <footer className="home-footer">
        <div className="home-wrap">
          <span>Hubu / 户部</span><p>Governance before execution.</p>
          <a href="/docs/feedback">Send feedback</a>
          <a href="/blogs">Engineering Blogs</a>
          <a href="/releases">Release notes ↗</a>
          <a href="https://github.com/hacker-no-ice/hubu">Source on GitHub ↗</a>
        </div>
      </footer>
    </div>
  );
}
