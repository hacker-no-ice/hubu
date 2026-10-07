import type { Metadata } from "next";
import { Search } from "./components/Search";
import { HubuWordmark } from "./components/HubuWordmark";
import { searchDocuments } from "./lib/docs";
import { homeMetadata } from "./lib/metadata";

const stackSteps = [
  ["01", "Initialize a profile", "hubu stack init --mode sandbox --profile \"$HOME/hubu-sandbox\"", "Create and register operator-owned starter files without starting services."],
  ["02", "Select and configure", "hubu stack select --profile \"$HOME/hubu-sandbox\"", "Select the profile for later commands, then edit stack.toml, credentials.toml, and providers.toml."],
  ["03", "Start in one shot", "hubu stack start", "Validate, render, and start missing managed components in dependency order."],
  ["04", "Check readiness", "hubu stack status", "Inspect the whole stack through one stable, redacted readiness view."],
  ["05", "Connect your favorite agent harness", "hubu init codex --stack-profile \"$HOME/hubu-sandbox\"", "Use the Codex helper shown here, or connect another MCP-capable harness to the unified MCP process."],
  ["06", "Run governed work", "authorize → execute → settle, release, or reconcile", "Keep policy and money state in Hubu; provider execution and artifacts in Gongbu."],
] as const;

const exampleSteps = [
  ["Agent asks for an image", "The request arrives through unified MCP. The agent never holds a provider key."],
  ["Hubu checks policy and reserves $0.05", "Policy allows it, so Hubu holds an illustrative $0.05 maximum against the budget."],
  ["Gongbu calls the provider", "Gongbu runs only the Hubu-authorized work and reports a $0.03 receipt."],
  ["Hubu settles $0.03 and releases $0.02", "The ledger records the actual cost and returns the rest to the budget."],
] as const;

const worksToday = [
  ["macOS", "source install from a release tag"],
  ["Codex", "setup helper; other MCP clients by manual config"],
  ["Gemini and FLUX.2 Pro", "live providers"],
  ["Sandbox", "with no provider credentials"],
] as const;

const primaryLinks = [
  ["Documentation", "/docs/overview"],
  ["Demos", "/demos"],
  ["Architecture", "/architecture/"],
  ["Send feedback", "/docs/feedback"],
  ["GitHub", "https://github.com/hacker-no-ice/hubu"],
] as const;

export const metadata: Metadata = homeMetadata;

export default function Home() {
  return (
    <div className="home-shell">
      <header className="topbar">
        <a className="brand" href="/" aria-label="Hubu documentation home">
          <HubuWordmark className="brand-wordmark" decorative />
          <i>/ docs</i>
        </a>
        <nav className="primary-nav" aria-label="Primary navigation">
          {primaryLinks.map(([label, href]) => <a href={href} key={href}>{label}</a>)}
        </nav>
        <details className="site-menu">
          <summary>Menu</summary>
          <nav aria-label="Primary navigation">
            {primaryLinks.map(([label, href]) => <a href={href} key={href}>{label}</a>)}
          </nav>
        </details>
      </header>

      <main id="main-content">
        <section className="hero">
          <div className="hero-copy">
            <HubuWordmark className="hero-wordmark" />
            <p className="eyebrow"><span /> Agent spend control plane</p>
            <h1>Governed spend<br />for AI agents.</h1>
            <p className="hero-lede">
              Hubu governs AI-agent spend. Gongbu executes only Hubu-authorized provider work.
              Agents get a budget, never your keys.
            </p>
            <div className="hero-actions">
              <a className="button primary" href="/docs/local-stack">Start with the local stack <span>→</span></a>
              <a className="button secondary" href="/architecture/">Explore the architecture</a>
              <a className="button secondary" href="/demos">Watch a demo</a>
            </div>
          </div>
          <div className="boundary-card" aria-label="Hubu and Gongbu responsibility boundary">
            <div className="boundary-heading"><span>One governed flow</span></div>
            <div className="boundary-plane hubu-plane">
              <p>CONTROL PLANE</p><h2>Hubu</h2>
              <ul><li>Policy + budgets</li><li>Authorizations</li><li>Ledger + reconciliation</li></ul>
            </div>
            <div className="contract-line"><span>versioned executor contract</span></div>
            <div className="boundary-plane gongbu-plane">
              <p>EXECUTION PLANE</p><h2>Gongbu</h2>
              <ul><li>Provider credentials</li><li>Temporal workflows</li><li>Artifacts + retries</li></ul>
            </div>
            <p className="boundary-note">
              Gongbu is the first-party executor, not a requirement. Run governance only with <code>hubu-only</code>, or <a href="/docs/external-executor">bring your own executor →</a>
            </p>
          </div>
        </section>

        <section className="worked-example section-wrap" aria-labelledby="worked-example-title">
          <div className="worked-example-head">
            <p className="eyebrow"><span /> Example</p>
            <h2 id="worked-example-title">One image request, end to end</h2>
            <p>Illustrative amounts.</p>
          </div>
          <ol className="example-flow">
            {exampleSteps.map(([title, copy], index) => (
              <li key={title}>
                <span>{`0${index + 1}`}</span>
                <strong>{title}</strong>
                <p>{copy}</p>
              </li>
            ))}
          </ol>
        </section>

        <section className="intro-video section-wrap" aria-labelledby="intro-video-title">
          <div className="section-intro compact">
            <p className="eyebrow"><span /> Watch the introduction</p>
            <h2 id="intro-video-title">Meet Hubu in 3 min</h2>
            <p>Give agents room to work, with spending boundaries you control.</p>
          </div>
          <div className="intro-video-player">
            <iframe
              src="https://www.youtube-nocookie.com/embed/ufEgYjmxKWM"
              title="Introducing Hubu: bounded spending power for AI agents"
              width="960"
              height="540"
              loading="lazy"
              referrerPolicy="strict-origin-when-cross-origin"
              allow="encrypted-media; picture-in-picture; fullscreen"
              allowFullScreen
            />
          </div>
          <p className="intro-video-link"><a href="https://youtu.be/ufEgYjmxKWM">Watch on YouTube ↗</a></p>
          <p className="intro-video-link"><a href="/demos/sandbox">See Hubu in use: watch the sandbox demo and follow the walkthrough →</a></p>
        </section>

        <section className="warning-band" aria-label="Project status warning">
          <span className="warning-mark">!</span>
          <div><strong>Experimental and local-first.</strong><p>Suitable for development, evaluation, and controlled live-provider experiments—not yet for money-grade production workloads.</p></div>
          <a href="/docs/overview#project-status">Read the production warning →</a>
        </section>

        <section className="works-today" aria-labelledby="works-today-title">
          <h2 id="works-today-title">Works today</h2>
          <ul>
            {worksToday.map(([label, detail]) => <li key={label}><strong>{label}</strong>{` ${detail}`}</li>)}
          </ul>
          <a href="/docs/overview#what-works-today">See what works today →</a>
        </section>

        <section className="quickstart section-wrap">
          <div className="section-intro">
            <p className="eyebrow"><span /> Managed stack experience</p>
            <h2>From install to a running stack</h2>
            <p>Follow the managed lifecycle from operator-owned configuration to a ready stack while preserving separate Hubu and Gongbu ownership.</p>
          </div>
          <div className="steps">
            <article className="step">
              <span>00</span>
              <div>
                <h3>Install from a release tag</h3>
                <a className="step-link" href="/docs/local-stack#install">Install guide →</a>
                <p>Build the four binaries from an exact release tag on macOS.</p>
              </div>
            </article>
            {stackSteps.map(([number, title, command, copy]) => (
              <article className="step" key={number}>
                <span>{number}</span>
                <div>
                  <h3>{title}</h3>
                  {command.startsWith("hubu ") ? (
                    <div className="code-block"><code>{command}</code><button className="copy-code" type="button" data-copy-code hidden>Copy</button></div>
                  ) : <code>{command}</code>}
                  <p>{copy}</p>
                </div>
              </article>
            ))}
          </div>
        </section>

        <section className="docs-entry section-wrap">
          <div className="section-intro compact">
            <p className="eyebrow"><span /> Find your path</p>
            <h2>Documentation that follows the system</h2>
          </div>
          <Search documents={searchDocuments} large />
          <div className="topic-grid">
            <a href="/docs/local-stack"><small>QUICK START</small><h3>Quick start</h3><p>From install to your first governed request.</p><span>Start here →</span></a>
            <a href="/docs/policy-engine"><small>GOVERNANCE</small><h3>Write a policy</h3><p>Human-authored rules every spend request is evaluated against.</p><span>Read the policy guide →</span></a>
            <a href="/docs/unified-mcp#setup"><small>AGENT SURFACE</small><h3>Connect your agent</h3><p>Codex setup helper, or configure another MCP client by hand.</p><span>Set up MCP →</span></a>
            <a href="/docs/external-executor"><small>EXECUTION</small><h3>Use your own executor</h3><p>Run Hubu-authorized work on any service that implements the executor contract.</p><span>Read the executor guide →</span></a>
          </div>
        </section>
      </main>

      <footer><span>Hubu / 户部</span><p>Governance before execution.</p><a href="https://github.com/hacker-no-ice/hubu">Source on GitHub ↗</a></footer>
    </div>
  );
}
