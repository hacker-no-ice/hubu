"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { PortalTour } from "./PortalTour";

type PolicyExcerpt = { path: string; header: readonly string[]; rules: readonly { id: string; lines: readonly string[] }[]; ruleCount: number };

const MASCOT = "/brand/hubu-mascot-evolved.jpg";

export function say(text: string, ms = 1500) {
  window.dispatchEvent(new CustomEvent("marty:say", { detail: { text, ms } }));
}

function Command({ children }: { children: string }) {
  return <div className="code-block"><code>{children}</code><button className="copy-code" type="button" data-copy-code hidden>Copy</button></div>;
}

function YamlLine({ line }: { line: string }) {
  const match = line.match(/^(\s*)(- )?([a-z_]+)(:)(.*)$/);
  if (!match) return <>{line}</>;
  const [, indent, dash, key, , value] = match;
  const literal = value.trim() && (key === "string" || key === "money");
  return <>{indent}{dash}<span className="k">{key}</span>:{literal ? <> <span className="s">{value.trim()}</span></> : value}</>;
}

// ---- simulated state for the playback (illustrative values) ----
type Event = { t: string; glyph: string; agent: string; provider: string; status: string; rule: string };
type Hud = { image: { consumed: number; frozen: number }; copy: { consumed: number; frozen: number }; events: Event[]; clock: number; fresh: boolean };
type Row = { who: string; kind: "you" | "tool" | "ok" | "hold" | "image"; text: string };

const startHud = (): Hud => ({
  image: { consumed: 0.62, frozen: 0 },
  copy: { consumed: 0.18, frozen: 0 },
  events: [
    { t: "14:01:12", glyph: "✓", agent: "image-agent", provider: "flux", status: "$0.05 → $0.03", rule: "allow_draft_renders" },
    { t: "13:58:40", glyph: "✓", agent: "copy-agent", provider: "gemini", status: "$0.04 → $0.02", rule: "allow_small_spend" },
  ],
  clock: 14 * 3600 + 2 * 60 + 5,
  fresh: false,
});
const settledHud = (): Hud => ({
  ...startHud(),
  image: { consumed: 0.97, frozen: 0 },
  events: [
    { t: "14:02:58", glyph: "✓", agent: "image-agent", provider: "flux", status: "$0.40 → $0.32", rule: "escalate_final_renders" },
    { t: "14:02:09", glyph: "✓", agent: "image-agent", provider: "flux", status: "$0.05 → $0.03", rule: "allow_draft_renders" },
    ...startHud().events,
  ],
  clock: 14 * 3600 + 2 * 60 + 58,
});

const MSG = {
  ask1: { who: "you", kind: "you", text: "Draft Marty holding a lantern, 1k." },
  call1: { who: "tool", kind: "tool", text: "hubu_submit_governed_execution · flux-2-pro · image_size 1k" },
  allow: { who: "hubu", kind: "ok", text: "allow · allow_draft_renders · reserved $0.05" },
  done: { who: "gongbu", kind: "image", text: "artifact ready · settled $0.03 · released $0.02" },
  ask2: { who: "you", kind: "you", text: "Now render the final at 4k." },
  call2: { who: "tool", kind: "tool", text: "hubu_submit_governed_execution · flux-2-pro · image_size 4k" },
  hold: { who: "hubu", kind: "hold", text: "‖ needs approval · escalate_final_renders · sent to the owner" },
  approved: { who: "hubu", kind: "ok", text: "approved by the owner in the admin portal · reserved $0.40" },
  final: { who: "gongbu", kind: "image", text: "artifact ready · settled $0.32 · released $0.08" },
} satisfies Record<string, Row>;
const settledChat: Row[] = [MSG.ask1, MSG.done, MSG.ask2, MSG.hold, MSG.approved, MSG.final];

const clockText = (seconds: number) => [Math.floor(seconds / 3600), Math.floor(seconds / 60) % 60, seconds % 60].map((part) => String(part).padStart(2, "0")).join(":");
const pad = (text: string, width: number) => (text + " ".repeat(width)).slice(0, width);
const money = (value: number) => `$${value.toFixed(2)}`;
function bar(consumed: number, frozen: number, limit: number) {
  const width = 24, used = Math.round((consumed / limit) * width), held = frozen > 0 ? Math.max(1, Math.round((frozen / limit) * width)) : 0;
  return "█".repeat(used) + "▓".repeat(held) + "░".repeat(Math.max(0, width - used - held));
}

function HudView({ hud }: { hud: Hud }) {
  const row = (name: string, state: { consumed: number; frozen: number }, limit: number) => {
    const used = state.consumed + state.frozen;
    return ` ${pad(name, 12)}${bar(state.consumed, state.frozen, limit)}  ${money(used)} of ${money(limit)}${`${Math.floor((used / limit) * 100)}%`.padStart(5)}`;
  };
  return (
    <pre className="term hud" aria-label="Example hubu watch output">
      <span className="hd">{pad(" HUBU WATCH · 2 agents", 56)}USD · {clockText(hud.clock)}</span>{"\n"}
      <span className="dim"> {"─".repeat(68)}</span>{"\n"}
      {row("image-agent", hud.image, 2)}{"\n"}
      {row("copy-agent", hud.copy, 1)}{"\n"}
      <span className="dim">{" ".repeat(13)}█ consumed  ▓ frozen  ░ free</span>{"\n\n"}
      <span className="hd"> RECENT DECISIONS</span>
      {hud.events.slice(0, 3).map((event, index) => {
        const line = ` 2026-11-24 ${event.t}  ${event.glyph} ${pad(event.agent, 12)}${pad(event.provider, 7)}${pad(event.status, 18)}${event.rule}`;
        return <span key={index}>{"\n"}{index === 0 && hud.fresh ? <span className="new">{line}</span> : line}</span>;
      })}
    </pre>
  );
}

function StatusView({ ready }: { ready: boolean[] }) {
  const flag = (on: boolean, yes = "yes") => (on ? <span className="ok">{yes}</span> : <span className="wait">…</span>);
  return (
    <pre className="term">
      {"Hubu stack status\n\nSummary\n  Classification        "}{ready[0] ? <span className="ok">running_ready</span> : <span className="wait">starting</span>}
      {"\n  Restart impact        none\nComponents\n  hubu     Ready        "}{flag(ready[1])}
      {"\n  gongbu   Ready        "}{flag(ready[2])}
      {"\nTemporal\n  Worker ready          "}{flag(ready[3])}
    </pre>
  );
}

export function StackBento({ policy }: { policy: PolicyExcerpt }) {
  const sectionRef = useRef<HTMLElement>(null);
  const [ready, setReady] = useState([true, true, true, true]);
  const [registered, setRegistered] = useState(true);
  const [hud, setHud] = useState<Hud>(settledHud);
  const [chat, setChat] = useState<Row[]>(settledChat);
  const [hit, setHit] = useState<string | null>(null);
  const [reveal, setReveal] = useState<"idle" | "pending" | "in">("idle");
  const timers = useRef<number[]>([]);
  const replay = useRef<() => void>(() => {});

  const stop = useCallback(() => { timers.current.forEach(window.clearTimeout); timers.current = []; }, []);
  const settle = useCallback(() => { stop(); setReady([true, true, true, true]); setRegistered(true); setHud(settledHud()); setChat(settledChat); setHit(null); }, [stop]);

  const play = useCallback(() => {
    stop();
    const at = (ms: number, fn: () => void) => timers.current.push(window.setTimeout(fn, ms));
    const add = (row: Row) => setChat((rows) => [...rows, row].slice(-6));
    const event = (next: Partial<Hud> & { push?: Event; replace?: Event; tick?: number }) => setHud((current) => {
      const events = next.push ? [next.push, ...current.events] : next.replace ? [next.replace, ...current.events.slice(1)] : current.events;
      return { ...current, ...next, clock: current.clock + (next.tick ?? 0), events, fresh: Boolean(next.push || next.replace) };
    });
    setReady([false, false, false, false]); setRegistered(false); setHud(startHud()); setChat([]); setHit(null);
    at(500, () => setReady([false, true, false, false]));
    at(850, () => setReady([false, true, true, false]));
    at(1200, () => setReady([false, true, true, true]));
    at(1550, () => setReady([true, true, true, true]));
    at(1900, () => setRegistered(true));
    at(2500, () => add(MSG.ask1));
    at(3300, () => add(MSG.call1));
    at(3900, () => { setHit("allow_draft_renders"); event({ image: { consumed: 0.62, frozen: 0.05 }, tick: 4, push: { t: "14:02:09", glyph: "◐", agent: "image-agent", provider: "flux", status: "reserved $0.05", rule: "allow_draft_renders" } }); add(MSG.allow); });
    at(5200, () => { event({ image: { consumed: 0.65, frozen: 0 }, tick: 2, replace: { t: "14:02:11", glyph: "✓", agent: "image-agent", provider: "flux", status: "$0.05 → $0.03", rule: "allow_draft_renders" } }); add(MSG.done); say("saved $0.02!"); });
    at(6400, () => { setHit(null); setHud((current) => ({ ...current, fresh: false })); });
    at(7200, () => add(MSG.ask2));
    at(8000, () => add(MSG.call2));
    at(8600, () => { setHit("escalate_final_renders"); event({ tick: 20, push: { t: "14:02:31", glyph: "‖", agent: "image-agent", provider: "flux", status: "$0.40 approval", rule: "escalate_final_renders" } }); add(MSG.hold); say("a 4k final needs your OK", 1800); });
    at(10600, () => { setHit(null); event({ image: { consumed: 0.65, frozen: 0.4 }, tick: 20, replace: { t: "14:02:51", glyph: "◐", agent: "image-agent", provider: "flux", status: "reserved $0.40", rule: "escalate_final_renders" } }); add(MSG.approved); say("approved, stamp!"); });
    at(12400, () => { event({ image: { consumed: 0.97, frozen: 0 }, tick: 7, replace: { t: "14:02:58", glyph: "✓", agent: "image-agent", provider: "flux", status: "$0.40 → $0.32", rule: "escalate_final_renders" } }); add(MSG.final); });
    at(13600, () => setHud((current) => ({ ...current, fresh: false })));
    at(17000, () => replay.current());
  }, [stop]);
  useEffect(() => { replay.current = play; }, [play]);

  useEffect(() => {
    const section = sectionRef.current;
    if (!section) return;
    const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    if (reduce) return;
    // Only fade tiles in if the section starts below the fold, so it is never hidden on arrival.
    if (section.getBoundingClientRect().top > window.innerHeight) setReveal("pending");
    let running = false;
    const observer = new IntersectionObserver((entries) => entries.forEach((entry) => {
      if (entry.isIntersecting) {
        setReveal("in");
        if (!running) { running = true; timers.current.push(window.setTimeout(play, 600)); }
      } else if (running && entry.intersectionRatio === 0) { running = false; settle(); }
    }), { threshold: [0, 0.12] });
    observer.observe(section);
    return () => { observer.disconnect(); stop(); };
  }, [play, settle, stop]);

  function spotlight(event: React.PointerEvent<HTMLElement>) {
    const rect = event.currentTarget.getBoundingClientRect();
    event.currentTarget.style.setProperty("--tx", `${event.clientX - rect.left}px`);
    event.currentTarget.style.setProperty("--ty", `${event.clientY - rect.top}px`);
  }

  return (
    <section className={`stackx reveal-${reveal}`} id="stack" aria-labelledby="stack-title" ref={sectionRef}>
      <div className="stackx-grid" aria-hidden="true" />
      <i className="beam" style={{ top: 192 }} aria-hidden="true" />
      <i className="beam" style={{ top: 640, animationDelay: "-3s", animationDuration: "9s" }} aria-hidden="true" />
      <i className="beam v" style={{ left: "40%" }} aria-hidden="true" />
      <i className="beam v" style={{ left: "78%", animationDelay: "-3.5s", animationDuration: "8s" }} aria-hidden="true" />
      <div className="home-wrap">
        <div className="sx-head reveal">
          <div>
            <p className="eyebrow"><span /> Managed stack experience</p>
            <h2 id="stack-title">From install to a<br />governed agent call</h2>
          </div>
          <p>Four setup steps, all on your machine. Install the four binaries from an exact release tag first (<a href="/docs/local-stack#install">install guide</a>), then follow the <a href="/docs/local-stack">local stack guide</a> for every flag.</p>
        </div>
        <div className="bx">
          <article className="tile t-stack reveal" style={{ "--i": 1 } as React.CSSProperties} onPointerMove={spotlight}>
            <div className="tt"><small>Start the stack</small><span className="step">Step 1 of 4</span></div>
            <Command>{'hubu stack init --mode sandbox --profile "$HOME/hubu-sandbox"'}</Command>
            <Command>{'hubu stack select --profile "$HOME/hubu-sandbox"'}</Command>
            <Command>hubu stack start</Command>
            <StatusView ready={ready} />
          </article>
          <article className="tile t-register reveal" style={{ "--i": 2 } as React.CSSProperties} onPointerMove={spotlight}>
            <div className="tt"><small>Register the owner and agent</small><span className="step">Step 2 of 4</span></div>
            <Command>{'hubu register human --username alice-example --display-name "Alice Example"'}</Command>
            <Command>hubu register agent --name image-agent --version local-dev</Command>
            <pre className="term">
              {"agent      image-agent\nagent_id   "}{registered ? <span className="ok">agt_…7KQ</span> : <span className="wait">…</span>}
              {"\naccount_id "}{registered ? <span className="ok">aga_…</span> : <span className="wait">…</span>}
            </pre>
            <p>The <code>agt_</code> ID is what policies and budgets attach to.</p>
          </article>
          <article className="tile t-policy reveal" style={{ "--i": 3 } as React.CSSProperties} onPointerMove={spotlight}>
            <div className="tt"><small>Apply a policy</small><span className="step">Step 3 of 4</span></div>
            <h3>Rules in plain YAML</h3>
            <Command>{`hubu policy apply --path ${policy.path} --name "Creative tiers"`}</Command>
            <pre className="term yaml">
              {policy.header.map((line) => <span key={line}><YamlLine line={line} />{"\n"}</span>)}
              {policy.rules.map((rule, index) => (
                <span key={rule.id}>
                  <span className={`rule${hit === rule.id ? (rule.id.startsWith("allow") ? " hit" : " hold") : ""}`}>
                    {rule.lines.map((line, lineIndex) => <span key={lineIndex}><YamlLine line={line} />{lineIndex < rule.lines.length - 1 ? "\n" : ""}</span>)}
                  </span>
                  {index < policy.rules.length - 1 ? "\n\n" : "\n"}
                </span>
              ))}
              <span className="c">{`  # excerpt: the full file has ${policy.ruleCount} rules`}</span>
            </pre>
            <p>This policy is for live FLUX and Gemini models, and it denies the sandbox&apos;s fixture model. In sandbox, apply the starter policy from <code>hubu policy new-template</code> instead (<a href="/docs/local-stack#make-your-first-governed-request">first governed request</a>).</p>
          </article>
          <article className="tile t-budget reveal" style={{ "--i": 4 } as React.CSSProperties} onPointerMove={spotlight}>
            <div className="tt"><small>Give the agent a budget</small><span className="step">Step 4 of 4</span></div>
            <Command>hubu budget create --agent-id agt_IMAGE_AGENT_ID --amount 2</Command>
            <div className="amt">$2.00<span>USD · cumulative</span></div>
            <p className="bid">image-agent · bgt_…</p>
          </article>
          <article className="tile t-hud reveal" style={{ "--i": 5 } as React.CSSProperties} onPointerMove={spotlight}>
            <div className="tt"><small>Watch spend live</small><span className="live">live</span></div>
            <Command>hubu watch</Command>
            <HudView hud={hud} />
          </article>
          <article className="tile t-harness reveal" style={{ "--i": 6 } as React.CSSProperties} onPointerMove={spotlight}>
            <div className="left">
              <div className="tt"><small>Plug into your harness</small></div>
              <h3>Hubu as MCP tools for your agent</h3>
              <p>Hubu ships a stdio MCP server, <code>hubu-unified-mcp</code>. The agent sees <code>hubu_*</code> and <code>gongbu_*</code> tools and never a provider key.</p>
              <div className="paths">
                <div className="path">
                  <div className="ph"><b>Remote, from ChatGPT</b><span className="badge warn">experimental</span></div>
                  <p>Secure MCP Tunnel: an outbound connection from your Mac, no inbound port. Works from ChatGPT on the web or your phone while the client runs.</p>
                  <div className="hop"><span>ChatGPT</span><i /><span>OpenAI tunnel</span><i /><span>tunnel-client</span><i /><span>hubu-unified-mcp</span></div>
                  <Command>caffeinate -i tunnel-client run --profile hubu-stdio</Command>
                  <a className="path-link" href="/docs/operations/chatgpt-mcp-tunnel">Tunnel runbook →</a>
                </div>
                <div className="path">
                  <div className="ph"><b>Local MCP client</b><span className="badge ok">Codex verified</span></div>
                  <p>Your harness starts <code>hubu-unified-mcp</code> from its MCP config. Codex has a setup helper. Other clients take manual config and aren&apos;t verified yet.</p>
                  <Command>{'hubu init codex --stack-profile "$HOME/hubu-sandbox"'}</Command>
                  <a className="path-link" href="/docs/unified-mcp#setup">MCP setup →</a>
                </div>
              </div>
            </div>
            <div className="chat" aria-label="Example agent session">
              <div className="bar"><span>Agent harness · image-agent</span><span>MCP · hubu-unified-mcp</span></div>
              {chat.map((row, index) => (
                <div className="row" key={`${index}-${row.text}`}>
                  <span className="who">{row.who}</span>
                  {row.kind === "image" ? (
                    <div className="img">
                      {/* eslint-disable-next-line @next/next/no-img-element -- Static artwork is served directly by the site worker. */}
                      <img src={MASCOT} alt="Draft of Marty holding a lantern" width="56" height="56" loading="lazy" />
                      <span className="ok">{row.text}</span>
                    </div>
                  ) : <div className={row.kind === "you" ? undefined : row.kind}>{row.text}</div>}
                </div>
              ))}
            </div>
          </article>
          <PortalTour />
        </div>
        <p className="sx-foot"><span>Illustrative amounts, IDs, and times.</span><a href="/docs/local-stack">Local stack guide →</a><a href="/docs/policy-engine">Policy guide →</a><a href="/docs/cli">CLI reference →</a></p>
      </div>
    </section>
  );
}
