import { Search } from "./components/Search";
import { HubuWordmark } from "./components/HubuWordmark";
import { searchDocuments } from "./lib/docs";

const stackSteps = [
  ["01", "Initialize a profile", "hubu stack init --profile …", "Create and register operator-owned starter files without starting services."],
  ["02", "Select and configure", "hubu stack select --profile …", "Select the profile for later commands, then edit stack.toml, credentials.toml, and providers.toml."],
  ["03", "Start in one shot", "hubu stack start", "Validate, render, and start missing managed components in dependency order."],
  ["04", "Check readiness", "hubu stack status", "Inspect the whole stack through one stable, redacted readiness view."],
  ["05", "Connect your favorite agent harness", "hubu init codex --stack-profile …", "Use the Codex helper shown here, or connect another MCP-capable harness to the unified MCP process."],
  ["06", "Run governed work", "authorize → execute → settle, release, or reconcile", "Keep policy and money state in Hubu; provider execution and artifacts in Gongbu."],
] as const;

export default function Home() {
  return (
    <div className="home-shell">
      <header className="topbar">
        <a className="brand" href="/" aria-label="Hubu documentation home">
          <HubuWordmark className="brand-wordmark" decorative />
          <i>/ docs</i>
        </a>
        <nav aria-label="Primary navigation">
          <a href="/docs/overview">Documentation</a>
          <a href="/architecture/">Architecture</a>
          <a href="https://github.com/hacker-no-ice/hubu">GitHub</a>
        </nav>
      </header>

      <main id="main-content">
        <section className="hero">
          <div className="hero-copy">
            <HubuWordmark className="hero-wordmark" />
            <p className="eyebrow"><span /> Agent spend control plane</p>
            <h1>Governed spend<br />for AI agents.</h1>
            <p className="hero-lede">
              Humans use Hubu to govern AI-agent spending through policy, budgets, authorization, and an auditable ledger.
              Gongbu executes provider work behind a versioned contract. The boundary is the product.
            </p>
            <div className="hero-actions">
              <a className="button primary" href="/docs/local-stack">Start with the local stack <span>→</span></a>
              <a className="button secondary" href="/architecture/">Explore the architecture</a>
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
          </div>
        </section>

        <section className="warning-band" aria-label="Project status warning">
          <span className="warning-mark">!</span>
          <div><strong>Experimental and local-first.</strong><p>Suitable for development, evaluation, and controlled live-provider experiments—not yet for money-grade production workloads.</p></div>
          <a href="/docs/overview">Read the production warning →</a>
        </section>

        <section className="quickstart section-wrap">
          <div className="section-intro">
            <p className="eyebrow"><span /> Managed stack experience</p>
            <h2>From profile to a running stack</h2>
            <p>Follow the managed lifecycle from operator-owned configuration to a ready stack while preserving separate Hubu and Gongbu ownership.</p>
          </div>
          <div className="steps">
            {stackSteps.map(([number, title, command, copy]) => (
              <article className="step" key={number}>
                <span>{number}</span><div><h3>{title}</h3><code>{command}</code><p>{copy}</p></div>
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
            <a href="/docs/agent-registration"><small>IDENTITY</small><h3>Register an agent</h3><p>Guidance-first, canonical, fingerprinted.</p><span>Read guide →</span></a>
            <a href="/docs/spend-lifecycle"><small>GOVERNANCE</small><h3>Trace a spend</h3><p>From intent and policy to ledger finality.</p><span>Follow lifecycle →</span></a>
            <a href="/docs/gongbu-execution"><small>EXECUTION</small><h3>Understand Gongbu</h3><p>Admission, workflows, providers, artifacts.</p><span>Open execution plane →</span></a>
            <a href="/docs/unified-mcp"><small>AGENT SURFACE</small><h3>Use unified MCP</h3><p>One catalog, two isolated backends.</p><span>Inspect the tools →</span></a>
          </div>
        </section>
      </main>

      <footer><span>Hubu / 户部</span><p>Governance before execution.</p><a href="https://github.com/hacker-no-ice/hubu">Source on GitHub ↗</a></footer>
    </div>
  );
}
