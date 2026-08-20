import Link from "next/link";
import { Search } from "./components/Search";
import { searchDocuments } from "./lib/docs";

const verifiedSteps = [
  ["01", "Initialize", "hubu stack init", "Create an operator-owned profile without starting services."],
  ["02", "Configure", "stack.toml · credentials.toml · providers.toml", "Choose ownership, binaries, identities, credential references, providers, and spend gates."],
  ["03", "Validate + render", "hubu stack doctor → hubu stack render", "Fail closed on incomplete inputs, then produce an immutable active generation."],
  ["04", "Start explicitly", "Hubu + Gongbu runbooks", "Start each service within its own state, credential, and failure boundary."],
  ["05", "Connect Codex", "hubu init codex --stack-profile …", "Install the unified MCP handoff, restart Codex, and inspect capabilities."],
  ["06", "Run governed work", "authorize → execute → submit → reconcile", "Keep policy and money state in Hubu; provider execution and artifacts in Gongbu."],
] as const;

export default function Home() {
  return (
    <div className="home-shell">
      <header className="topbar">
        <Link className="brand" href="/" aria-label="Hubu documentation home">
          <span className="brand-seal" aria-hidden="true">户</span>
          <span>Hubu <i>/ docs</i></span>
        </Link>
        <nav aria-label="Primary navigation">
          <Link href="/docs/overview">Documentation</Link>
          <a href="/architecture/">Architecture</a>
          <a href="https://github.com/hacker-no-ice/hubu">GitHub</a>
        </nav>
      </header>

      <main id="main-content">
        <section className="hero">
          <div className="hero-copy">
            <p className="eyebrow"><span /> Agent spend control plane</p>
            <h1>Governed spend<br />for AI agents.</h1>
            <p className="hero-lede">
              Hubu gives humans policy, budget, authorization, and ledger control.
              Gongbu executes provider work behind a versioned contract. The boundary is the product.
            </p>
            <div className="hero-actions">
              <Link className="button primary" href="/docs/local-stack">Start with the local stack <span>→</span></Link>
              <a className="button secondary" href="/architecture/">Explore the architecture</a>
            </div>
          </div>
          <div className="boundary-card" aria-label="Hubu and Gongbu responsibility boundary">
            <div className="boundary-heading"><span>One governed flow</span><code>v4.2</code></div>
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
          <Link href="/docs/overview#before-real-money-deployment">Read the production warning →</Link>
        </section>

        <section className="quickstart section-wrap">
          <div className="section-intro">
            <p className="eyebrow"><span /> Verified quick start</p>
            <h2>From profile to governed workload</h2>
            <p>This sequence reflects commands available on the current main branch. Every step preserves separate Hubu and Gongbu ownership.</p>
          </div>
          <div className="steps">
            {verifiedSteps.map(([number, title, command, copy]) => (
              <article className="step" key={number}>
                <span>{number}</span><div><h3>{title}</h3><code>{command}</code><p>{copy}</p></div>
              </article>
            ))}
          </div>
          <aside className="roadmap-note">
            <div><span>INTENDED MANAGED LIFECYCLE</span><code>stack init → configure → stack start → stack status → init codex</code></div>
            <p><strong>Not available on main yet.</strong> <code>stack start</code> and <code>stack status</code> are planned lifecycle work. Until they land, follow the service runbooks and use <code>stack doctor</code> for readiness.</p>
          </aside>
        </section>

        <section className="docs-entry section-wrap">
          <div className="section-intro compact">
            <p className="eyebrow"><span /> Find your path</p>
            <h2>Documentation that follows the system</h2>
          </div>
          <Search documents={searchDocuments} large />
          <div className="topic-grid">
            <Link href="/docs/agent-registration"><small>IDENTITY</small><h3>Register an agent</h3><p>Guidance-first, canonical, fingerprinted.</p><span>Read guide →</span></Link>
            <Link href="/docs/spend-lifecycle"><small>GOVERNANCE</small><h3>Trace a spend</h3><p>From intent and policy to ledger finality.</p><span>Follow lifecycle →</span></Link>
            <Link href="/docs/gongbu-execution"><small>EXECUTION</small><h3>Understand Gongbu</h3><p>Admission, workflows, providers, artifacts.</p><span>Open execution plane →</span></Link>
            <Link href="/docs/unified-mcp"><small>AGENT SURFACE</small><h3>Use unified MCP</h3><p>One catalog, two isolated backends.</p><span>Inspect the tools →</span></Link>
          </div>
        </section>
      </main>

      <footer><span>Hubu / 户部</span><p>Governance before execution.</p><a href="https://github.com/hacker-no-ice/hubu">Source on GitHub ↗</a></footer>
    </div>
  );
}
