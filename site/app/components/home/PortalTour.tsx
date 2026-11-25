"use client";

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";

// An illustrative tour of `hubu ui`. Labels follow the portal's sections in
// crates/hubu-cli/src/ui/assets/app.js; all amounts and IDs are examples.

type View = "overview" | "approvals" | "decisions" | "agents" | "policies" | "budgets" | "providers";
type Step = { view: View; feature: number; ms: number; render: () => ReactNode };

const NAV: { view: View | null; label: string }[] = [
  { view: "overview", label: "Overview" },
  { view: "approvals", label: "Approvals" },
  { view: null, label: "Spend history" },
  { view: "decisions", label: "Decisions" },
  { view: "agents", label: "Agents" },
  { view: null, label: "Users & targets" },
  { view: "policies", label: "Policies" },
  { view: "budgets", label: "Budgets" },
  { view: "providers", label: "Providers & secrets" },
];

const FEATURES = [
  "Overview at a glance",
  "Approve pending requests",
  "Inspect decisions",
  "Register and manage agents",
  "Edit policies with validate and diff",
  "Change budget limits",
  "See enabled providers",
];

const Badge = ({ tone, children }: { tone: "ok" | "warn" | "bad"; children: ReactNode }) => <span className={`badge ${tone}`}>{children}</span>;
const Btn = ({ tone, children }: { tone?: string; children: ReactNode }) => <span className={`pbtn ${tone ?? ""}`}>{children}</span>;
const Head = ({ title, right }: { title: string; right?: ReactNode }) => <div className="phead"><strong>{title}</strong><span className="pright">{right ?? <Btn tone="ghost">Refresh</Btn>}</span></div>;
const Chips = ({ items }: { items: string[] }) => <span className="pchips">{items.map((item) => <span key={item}>{item}</span>)}</span>;
const Meter = ({ used, limit }: { used: number; limit: number }) => <span className="pmeter"><i style={{ width: `${Math.min(100, (used / limit) * 100)}%` }} /></span>;
const Card = ({ label, value, tone }: { label: string; value: string; tone?: string }) => <div className={`pcard ${tone ?? ""}`}><small>{label}</small><b>{value}</b></div>;

function Table({ cols, template, rows }: { cols: string[]; template: string; rows: { cells: ReactNode[]; tone?: string }[] }) {
  return (
    <div className="ptable" style={{ "--cols": template } as React.CSSProperties}>
      <div className="pth">{cols.map((col, index) => <span key={index}>{col}</span>)}</div>
      {rows.map((row, index) => <div className={`ptr ${row.tone ?? ""}`} key={index}>{row.cells.map((cell, cellIndex) => <span key={cellIndex}>{cell}</span>)}</div>)}
    </div>
  );
}
const mono = (text: string) => <span className="mono">{text}</span>;
const num = (text: string) => <span className="num">{text}</span>;
const week = [0.12, 0.31, 0.08, 0.22, 0.17, 0.25, 0.35];

function Overview({ focus }: { focus: boolean }) {
  return (
    <>
      <Head title="Overview" right={<span className="pauto"><i />Auto-refresh</span>} />
      <div className="pgroups">
        <div className="pgroup"><h5>People and agents</h5><div className="pcards"><Card label="Users" value="1" /><Card label="Agents" value="3" /></div></div>
        <div className="pgroup"><h5>Spend lifecycle</h5><div className="pcards"><Card label="Pending approvals" value="1" tone={`hot${focus ? " focus" : ""}`} /><Card label="Automatic denials" value="1" tone="hotbad" /><Card label="Unresolved reconciliations" value="0" /></div></div>
        <div className="pgroup"><h5>Policies and budgets</h5><div className="prows">
          <div><span>image-agent</span><Meter used={0.97} limit={2} />{num("$0.97 of $2.00")}</div>
          <div><span>copy-agent</span><Meter used={0.18} limit={1} />{num("$0.18 of $1.00")}</div>
          <div><span>Policies</span><span className="muted">creative_tiers · starter_spending_policy</span><span /></div>
        </div></div>
        <div className="pgroup"><h5>Spend per day · 7 days</h5>
          <div className="pbars">{week.map((value, index) => <i key={index} className={index === 6 ? "today" : undefined} style={{ height: `${Math.max(6, (value / 0.4) * 100)}%` }} title={`$${value.toFixed(2)}`} />)}</div>
          <div className="ptotal"><span>Total</span><b>$1.50</b></div>
        </div>
      </div>
    </>
  );
}

function Approvals({ state }: { state: "list" | "review" | "confirm" | "approved" }) {
  const actions = state === "list" ? <><Btn>Review</Btn><Btn tone="pri">Approve</Btn><Btn>Deny</Btn></> : state === "confirm" ? <Btn tone="busy">Confirm on your Mac…</Btn> : state === "approved" ? null : <><Btn tone="pri">Approve</Btn><Btn>Deny</Btn></>;
  return (
    <>
      <Head title="Approvals" />
      <Table cols={["Requested", "Agent", "Amount", "Billing merchant", "Policy", ""]} template="64px 88px 60px 96px 118px minmax(150px,1fr)"
        rows={[{ tone: state === "list" ? "sel" : undefined, cells: ["14:02:31", "image-agent", num("$0.40"), "bfl", state === "approved" ? <Badge tone="ok" key="b">approved</Badge> : <Badge tone="warn" key="b">needs_approval</Badge>, <span className="pact" key="a">{actions}</span>] }]} />
      {state !== "list" && (
        <div className="preview">
          <h6>Review request</h6>
          <dl>
            <div><dt>Agent context</dt><dd>Final 4k render of Marty holding a lantern, picked from three 1k drafts.</dd></div>
            <div><dt>Execution scope</dt><dd className="mono">flux-2-pro · image_size 4k · capability:image:generate</dd></div>
            <div><dt>Amount</dt><dd className="mono">$0.40 maximum</dd></div>
            <div><dt>Policy decision</dt><dd><Badge tone="warn">needs_approval</Badge> {mono("escalate_final_renders")}: high-resolution finals need human approval</dd></div>
          </dl>
          {state === "approved" && <p className="pok">Approved. $0.40 reserved for image-agent.</p>}
          {state === "confirm" && <p className="pwarn">Waiting for Touch ID on your Mac.</p>}
        </div>
      )}
    </>
  );
}

function Decisions({ highlight }: { highlight: boolean }) {
  return (
    <>
      <Head title="Decisions" right={<Chips items={["Agent: all", "Category: all", "Last 24 hours"]} />} />
      <div className="pcards four"><Card label="Allowed" value="6" /><Card label="Approved" value="1" /><Card label="Denied" value="1" tone="hotbad" /><Card label="Spent" value="$1.50" /></div>
      <Table cols={["Time", "Agent", "Decision", "Rule", "Requested → cost"]} template="64px 92px 86px minmax(150px,1fr) 118px" rows={[
        { cells: ["14:02:58", "image-agent", <Badge tone="ok" key="b">approved</Badge>, mono("escalate_final_renders"), num("$0.40 → $0.32")] },
        { cells: ["14:02:09", "image-agent", <Badge tone="ok" key="b">allowed</Badge>, mono("allow_draft_renders"), num("$0.05 → $0.03")] },
        { tone: highlight ? "sel" : undefined, cells: ["13:47:22", "image-agent", <Badge tone="bad" key="b">denied</Badge>, mono("deny_unapproved_image_model"), num("$0.08 blocked")] },
        { cells: ["13:58:40", "copy-agent", <Badge tone="ok" key="b">allowed</Badge>, mono("allow_small_spend"), num("$0.04 → $0.02")] },
      ]} />
      {highlight && <p className="pnote">Denied: image work must use an approved model. The request named a model outside the allowed list.</p>}
    </>
  );
}

function Agents({ step }: { step: number }) {
  const rows = [
    { cells: ["image-agent", mono("agt_…7KQ"), "v3", num("$2.00")] },
    { cells: ["copy-agent", mono("agt_…2MD"), "v1", num("$1.00")] },
    ...(step >= 3 ? [{ tone: "new", cells: ["storyboard-agent", mono("agt_…9XA"), "v1", <span className="muted" key="m">no current budget</span>] }] : []),
  ];
  return (
    <>
      <Head title="Agents" right={<Btn tone="pri">Register agent</Btn>} />
      <Table cols={["Name", "Agent ID", "Version", "Budget"]} template="minmax(120px,1fr) 110px 70px 130px" rows={rows} />
      {step >= 1 && step < 3 && (
        <div className="pform">
          <h6>Register agent</h6>
          <div className="pfields">
            <div>Agent name<span className="pin">storyboard-agent{step === 1 && <i className="caret" />}</span></div>
            <div>Version label<span className="pin">v1</span></div>
          </div>
          {step === 2 && <><p className="pnote">Review: owner and runtime fields come from the active Hubu session. Fingerprints are computed here and recomputed by the server.</p><span className="pact"><Btn tone="pri">Register</Btn></span></>}
        </div>
      )}
      {step >= 3 && <p className="pok">Registered storyboard-agent. Human users and spending targets live under Users &amp; targets.</p>}
    </>
  );
}

function Line({ n, text, tone }: { n: number; text: string; tone?: string }) {
  return <div className={`pl ${tone ?? ""}`}><span>{n}</span><code>{text}</code></div>;
}

function Policies({ step }: { step: number }) {
  return (
    <>
      <Head title="Policies" right={<Chips items={["creative_tiers", "revision 2026-10-19.1"]} />} />
      <div className="ptabs"><span>Read</span><span className="on">Edit</span><span>Compare</span></div>
      <div className="peditor">
        <Line n={52} text="  - id: allow_draft_renders" /><Line n={53} text="    effect: allow" /><Line n={54} text="    reason: 1k image drafts are pre-approved" />
        <Line n={66} text="        - op: lte" /><Line n={67} text="          field: amount" /><Line n={68} text="          value:" />
        {step >= 1 ? <><Line n={69} text='            money: "0.10"' tone="del" /><Line n={69} text='            money: "0.15"' tone="add" /></> : <Line n={69} text='            money: "0.10"' />}
      </div>
      <div className="pfoot">
        {step >= 2 ? <span className="pok">Validate passed · diff shown against the revision in effect</span> : <span className="muted">Paste an agent draft or edit the YAML, then validate.</span>}
        <span className="pact"><Btn tone={step === 1 ? "pri" : undefined}>Validate</Btn>{step === 3 ? <Btn tone="busy">Confirm on your Mac…</Btn> : <Btn tone={step === 2 ? "pri" : "off"}>Save</Btn>}</span>
      </div>
      {step >= 4 && <p className="pok">Saved a new revision. Save sends exactly the validated text.</p>}
    </>
  );
}

function Budgets({ step }: { step: number }) {
  const limit = step >= 2 ? 5 : 2;
  return (
    <>
      <Head title="Budgets" right={<Btn>Create budget</Btn>} />
      <Table cols={["Agent", "Budget", "Limit", "Used", "Version"]} template="100px 96px 80px minmax(130px,1fr) 64px" rows={[
        { tone: step >= 1 ? "sel" : undefined, cells: ["image-agent", mono("bgt_…4HF"), num(`$${limit.toFixed(2)}`), <span key="u"><Meter used={0.97} limit={limit} /> {num("$0.97")}</span>, step >= 2 ? "v2" : "v1"] },
        { cells: ["copy-agent", mono("bgt_…8PL"), num("$1.00"), <span key="u"><Meter used={0.18} limit={1} /> {num("$0.18")}</span>, "v1"] },
      ]} />
      {step === 1 && (
        <div className="pform">
          <h6>Change limit · image-agent</h6>
          <div className="pfields"><div>New limit (USD)<span className="pin">5.00<i className="caret" /></span></div><div>Pinned to<span className="pin muted">v1</span></div></div>
          <span className="pact"><Btn tone="busy">Confirm on your Mac…</Btn></span>
        </div>
      )}
      {step >= 2 && <div className="phist"><h6>Version history</h6><div>{mono("v2")}<span>limit $5.00</span><span className="muted">today 14:06</span></div><div>{mono("v1")}<span>limit $2.00</span><span className="muted">created 13:20</span></div></div>}
    </>
  );
}

const FLUX = ["flux-2-pro", "flux-2-max", "flux-2-flex", "flux-2-klein-4b", "flux-2-klein-9b", "flux-3-image", "flux-3-image-klein"];
const GEMINI = ["gemini-3.1-flash-image", "gemini-3.1-flash-lite-image", "gemini-3-pro-image"];
const VERSION: Record<string, string> = { "flux-2-pro": "v2", "flux-3-image": "v2" };

function Providers() {
  const table = (models: string[], secret: string, fallback: string) => (
    <Table cols={["Model", "Workloads", "Config", "Secret reference", "Offered by Gongbu"]} template="minmax(170px,1fr) 118px 54px 120px 110px"
      rows={models.map((model) => ({ cells: [mono(model), "image · edit", VERSION[model] ?? fallback, mono(secret), <Badge tone="ok" key="b">yes</Badge>] }))} />
  );
  return (
    <>
      <Head title="Providers & secrets" right={<Chips items={["read-only", `${FLUX.length + GEMINI.length} models`]} />} />
      <div className="pplat"><h6>Black Forest Labs · FLUX</h6>{table(FLUX, "bfl_api_key", "v1")}</div>
      <div className="pplat"><h6>Google · Gemini Developer API</h6>{table(GEMINI, "gemini_api_key", "v2")}</div>
      <p className="pnote">Secret values are never shown. To add a provider, edit the stack profile and run {mono("hubu stack render")}.</p>
    </>
  );
}

const TOUR: Step[] = [
  { view: "overview", feature: 0, ms: 2600, render: () => <Overview focus={false} /> },
  { view: "overview", feature: 0, ms: 1200, render: () => <Overview focus /> },
  { view: "approvals", feature: 1, ms: 1500, render: () => <Approvals state="list" /> },
  { view: "approvals", feature: 1, ms: 2200, render: () => <Approvals state="review" /> },
  { view: "approvals", feature: 1, ms: 1700, render: () => <Approvals state="confirm" /> },
  { view: "approvals", feature: 1, ms: 1800, render: () => <Approvals state="approved" /> },
  { view: "decisions", feature: 2, ms: 1800, render: () => <Decisions highlight={false} /> },
  { view: "decisions", feature: 2, ms: 2400, render: () => <Decisions highlight /> },
  ...[0, 1, 2, 3].map((step, index) => ({ view: "agents" as View, feature: 3, ms: [1200, 1500, 2000, 2000][index], render: () => <Agents step={step} /> })),
  ...[0, 1, 2, 3, 4].map((step, index) => ({ view: "policies" as View, feature: 4, ms: [1400, 1400, 1600, 1500, 1800][index], render: () => <Policies step={step} /> })),
  ...[0, 1, 2].map((step, index) => ({ view: "budgets" as View, feature: 5, ms: [1300, 1700, 2200][index], render: () => <Budgets step={step} /> })),
  { view: "providers", feature: 6, ms: 4800, render: () => <Providers /> },
];
const APPROVED_AT = 5;

export function PortalTour() {
  const [index, setIndex] = useState(0);
  const [running, setRunning] = useState(false);
  const tileRef = useRef<HTMLElement>(null);
  const navRef = useRef<HTMLElement>(null);
  const step = TOUR[index];

  const schedule = useCallback((from: number) => {
    setIndex(from);
    setRunning(!window.matchMedia("(prefers-reduced-motion: reduce)").matches);
  }, []);

  useEffect(() => {
    if (!running) return;
    const timer = window.setTimeout(() => setIndex((current) => (current + 1) % TOUR.length), TOUR[index].ms);
    return () => window.clearTimeout(timer);
  }, [index, running]);

  useEffect(() => {
    const tile = tileRef.current;
    if (!tile) return;
    let visible = false;
    const observer = new IntersectionObserver((entries) => entries.forEach((entry) => {
      if (entry.isIntersecting && !visible) { visible = true; schedule(0); }
      if (!entry.isIntersecting && visible) { visible = false; setRunning(false); }
    }), { threshold: 0.3 });
    observer.observe(tile);
    return () => observer.disconnect();
  }, [schedule]);

  // When the section bar scrolls sideways (phones), keep the shown section's
  // tab in view. Only the bar scrolls, never the page.
  useEffect(() => {
    const nav = navRef.current;
    const active = nav?.querySelector<HTMLElement>("button.on");
    if (!nav || !active || nav.scrollWidth <= nav.clientWidth) return;
    const navBox = nav.getBoundingClientRect(), tabBox = active.getBoundingClientRect();
    const left = nav.scrollLeft + (tabBox.left - navBox.left) - (nav.clientWidth - tabBox.width) / 2;
    const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    nav.scrollTo({ left: Math.max(0, left), behavior: reduce ? "auto" : "smooth" });
  }, [step.view]);

  useEffect(() => {
    if (step.view === "providers") window.dispatchEvent(new CustomEvent("marty:say", { detail: { text: "so many FLUX models", ms: 1600 } }));
  }, [step.view]);

  const jump = (view: View) => schedule(TOUR.findIndex((candidate) => candidate.view === view));
  const pending = index < APPROVED_AT ? 1 : 0;

  return (
    <article className="tile t-portal reveal" style={{ "--i": 7 } as React.CSSProperties} ref={tileRef}>
      <div className="left">
        <div className="tt"><small>Run it from the admin portal</small></div>
        <h3>A local portal for the owner</h3>
        <div className="code-block"><code>hubu ui</code><button className="copy-code" type="button" data-copy-code hidden>Copy</button></div>
        <p>Opens from a one-time login link. Everything the admin CLI does, in a browser. Changes that matter ask for Touch ID on your Mac. <a href="/docs/admin-ui">Admin UI guide →</a></p>
        <ul className="pfeat">
          {FEATURES.map((feature, featureIndex) => (
            <li key={feature} className={featureIndex === step.feature ? "on" : undefined}>
              <button type="button" onClick={() => schedule(TOUR.findIndex((candidate) => candidate.feature === featureIndex))}>{feature}</button>
            </li>
          ))}
        </ul>
      </div>
      <div className="portal" aria-label="Example admin portal tour">
        <div className="pbar"><span className="dotz"><i /><i /><i /></span><span>127.0.0.1:8790/#/{step.view}</span><span /></div>
        <div className="pbody">
          <nav className="pnav" aria-label="Portal sections (example)" ref={navRef}>
            <b>Hubu <span>/ admin</span></b>
            {NAV.map((item) => (
              <button type="button" key={item.label} disabled={!item.view} className={item.view === step.view ? "on" : undefined} aria-current={item.view === step.view ? "page" : undefined} onClick={() => item.view && jump(item.view)}>
                {item.label}{item.view === "approvals" && <em className={pending ? "hot" : undefined}>{pending}</em>}
              </button>
            ))}
          </nav>
          <div className="pmain" key={step.view}>{step.render()}</div>
        </div>
      </div>
    </article>
  );
}
