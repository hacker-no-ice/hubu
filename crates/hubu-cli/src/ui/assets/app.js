// Hubu admin UI. Served only by `hubu ui`, which relays allow-listed calls to
// the Hubu server. Every DOM node is built with textContent; server data is
// never interpreted as HTML.
"use strict";

const VIEWS = [
  { group: "Monitor" },
  { id: "overview", label: "Overview", render: renderOverview },
  { id: "approvals", label: "Approvals", render: renderApprovals, badge: "approvals" },
  { id: "spend", label: "Spend history", render: renderSpend },
  { id: "decisions", label: "Decisions", render: renderDecisions },
  { group: "Manage" },
  { id: "agents", label: "Agents", render: renderAgents },
  { id: "users", label: "Users & targets", render: renderUsers },
  { id: "policies", label: "Policies", render: renderPolicies },
  { id: "budgets", label: "Budgets", render: renderBudgets },
  { id: "providers", label: "Providers & secrets", render: renderProviders },
];

const state = { session: null, agents: [], counts: {}, routeParams: new URLSearchParams() };

// ---------- DOM helpers ----------

function h(tag, props, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(props || {})) {
    if (value === undefined || value === null || value === false) continue;
    if (key === "class") node.className = value;
    else if (key === "style") Object.assign(node.style, value); // CSSOM: allowed by the CSP
    else if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else if (key === "value") node.value = value;
    else if (value === true) node.setAttribute(key, "");
    else node.setAttribute(key, String(value));
  }
  append(node, children);
  return node;
}

function append(node, children) {
  for (const child of children.flat(Infinity)) {
    if (child === undefined || child === null || child === false) continue;
    node.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return node;
}

function clear(node) {
  while (node.firstChild) node.removeChild(node.firstChild);
  return node;
}

function card(title, body, actions, flush) {
  return h("div", { class: "card" },
    h("header", {}, h("h2", {}, title), actions ? h("div", { class: "row-actions" }, actions) : null),
    h("div", { class: flush ? "body flush" : "body" }, body));
}

function table(columns, rows, emptyText) {
  if (!rows || rows.length === 0) return h("div", { class: "empty" }, emptyText || "Nothing to show.");
  return h("table", {},
    h("thead", {}, h("tr", {}, columns.map((c) => h("th", { class: c.num ? "num" : null }, c.label)))),
    h("tbody", {}, rows.map((row) => h("tr", {}, columns.map((c) => {
      const value = c.render ? c.render(row) : row[c.key];
      return h("td", { class: [c.num ? "num" : "", c.mono ? "mono" : ""].join(" ").trim() || null },
        value === undefined || value === null || value === "" ? "—" : value);
    })))));
}

function kv(pairs) {
  return h("dl", { class: "kv" }, pairs.filter(Boolean).map(([k, v]) => [h("dt", {}, k),
    h("dd", {}, v === undefined || v === null || v === "" ? "—" : v)]));
}

function jsonBlock(value) {
  return h("pre", { class: "json" }, JSON.stringify(value, null, 2));
}

function details(summary, value) {
  return h("details", {}, h("summary", { class: "hint" }, summary), jsonBlock(value));
}

const STATUS_TONE = {
  active: "ok", allow: "ok", allowed: "ok", authorized: "ok", settled: "ok", approved: "ok",
  policy_allow: "ok", granted: "ok", admitted: "ok", true: "ok",
  live: "ok", current: "info", exceeded: "warn", exhausted: "warn", scheduled: "info", enabled: "ok", disabled: "", inactive: "",
  needs_approval: "warn", pending: "warn", escalated: "warn", claimed: "info",
  policy_needs_approval: "warn", reconciliation_required: "warn", unknown: "warn",
  deny: "bad", denied: "bad", revoked: "bad", policy_deny: "bad", released: "", expired: "",
  declined: "bad", rejected: "bad", false: "",
};

function badge(text) {
  if (text === undefined || text === null) return null;
  const tone = STATUS_TONE[String(text)] ?? "";
  return h("span", { class: `badge ${tone}` }, String(text).replaceAll("_", " "));
}

function money(m) {
  if (!m) return null;
  if (m.display !== undefined) return `${m.display} ${String(m.currency || "").toUpperCase()}`.trim();
  if (m.amount !== undefined && m.scale !== undefined) {
    const n = Number(m.amount) / 10 ** Number(m.scale);
    return `${n.toFixed(Math.max(2, Number(m.scale)))} ${String(m.currency || "").toUpperCase()}`.trim();
  }
  return JSON.stringify(m);
}

function when(value) {
  if (!value) return null;
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return String(value);
  return h("span", { class: "nowrap", title: String(value) }, date.toLocaleString());
}

function short(id, length = 14) {
  if (!id) return null;
  const text = String(id);
  return h("span", { class: "mono", title: text }, text.length > length ? `${text.slice(0, length)}…` : text);
}

function agentName(agentId) {
  const agent = state.agents.find((a) => a.agent_id === agentId);
  return agent ? h("span", { title: agentId }, agent.display_name) : short(agentId);
}

function toast(message, tone = "ok") {
  const node = h("div", { class: `toast ${tone}` }, message);
  document.getElementById("toasts").append(node);
  setTimeout(() => node.remove(), tone === "bad" ? 9000 : 4000);
}

function isoFromLocal(value) {
  return value ? new Date(value).toISOString() : null;
}

function formValues(form) {
  const values = {};
  for (const element of form.elements) {
    if (!element.name) continue;
    if (element.type === "checkbox") values[element.name] = element.checked;
    else values[element.name] = element.value.trim();
  }
  return values;
}

// ---------- API ----------

class ApiError extends Error {
  constructor(status, body) {
    const code = body && body.error_code ? ` (${body.error_code})` : "";
    super(`${(body && body.error) || `HTTP ${status}`}${code}`);
    this.status = status;
    this.body = body;
  }
}

async function api(method, path, body, options = {}) {
  const headers = { "X-Hubu-UI": "1" };
  if (body !== undefined) headers["Content-Type"] = "application/json";
  if (options.confirmed) headers["X-Hubu-UI-Confirmed"] = "1";
  const response = await fetch(path, {
    method,
    headers,
    credentials: "same-origin",
    cache: "no-store",
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  let data = null;
  try { data = await response.json(); } catch (_) { data = null; }
  if (!response.ok) throw new ApiError(response.status, data);
  return data;
}

function hubu(path, query) {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query || {})) {
    if (value !== undefined && value !== null && value !== "") params.set(key, value);
  }
  const suffix = params.toString() ? `?${params}` : "";
  return api("GET", `/api/hubu${path}${suffix}`);
}

function hubuPost(path, body, options) {
  return api("POST", `/api/hubu${path}`, body, options);
}

async function loadAgents() {
  // Owner-scoped: these feed forms and filters whose APIs only accept the
  // signed-in owner's agents (`all=true` would include other users' agents).
  const data = await hubu("/agents");
  state.agents = data.agents || [];
  return state.agents;
}

// ---------- dialogs ----------

/** `footer` is an optional (close) => buttons callback. */
function openDialog(title, body, footer) {
  const dialog = h("dialog", {});
  const close = () => { dialog.close(); dialog.remove(); };
  append(dialog, [
    h("header", {}, h("h3", {}, title), h("button", { class: "ghost small", type: "button", onclick: close }, "Close")),
    h("div", { class: "dialog-body" }, body),
    footer ? h("footer", {}, footer(close)) : null,
  ]);
  dialog.addEventListener("close", () => dialog.remove());
  document.body.append(dialog);
  dialog.showModal();
  return { dialog, close };
}

async function showJsonDialog(title, loader) {
  const body = h("div", {}, h("p", { class: "hint" }, "Loading…"));
  openDialog(title, body);
  try {
    const value = await loader();
    clear(body).append(value instanceof Node ? value : jsonBlock(value));
  } catch (error) {
    clear(body).append(h("div", { class: "notice bad" }, error.message));
  }
}

/** Two-step confirmation for actions that use the human approval capability. */
function confirmCapability({ title, summary, confirmLabel, danger, onConfirm }) {
  const status = h("div", {});
  let armed = false;
  const button = h("button", { type: "button", class: danger ? "danger" : "" }, confirmLabel);
  const { close } = openDialog(title, [
    summary,
    h("div", { class: "notice warn" },
      "This action uses the human approval capability held by `hubu ui`. Review the details, then confirm."),
    status,
  ], (closeFn) => [
    h("button", { type: "button", class: "ghost", onclick: closeFn }, "Cancel"),
    button,
  ]);
  button.addEventListener("click", async () => {
    if (!armed) {
      armed = true;
      button.textContent = `Click again to ${confirmLabel.toLowerCase()}`;
      return;
    }
    button.disabled = true;
    try {
      await onConfirm();
      close();
    } catch (error) {
      clear(status).append(h("div", { class: "notice bad" }, error.message));
      button.disabled = false;
      armed = false;
      button.textContent = confirmLabel;
    }
  });
}

// ---------- views ----------

async function renderOverview(view) {
  const [summary, budgets, records, agents, users, policies, targets] = await Promise.all([
    hubu("/governance/summary"),
    hubu("/budgets"),
    hubu("/governance/records", { limit: "8" }),
    loadAgents(),
    hubu("/users"),
    hubu("/policies"),
    hubu("/user/spending-target"),
  ]);
  const s = summary.summary || {};
  const backlog = s.backlog || {};
  const budgetRows = budgets.budgets || [];
  const policyRows = policies.policies || [];
  const totals = (field) => {
    const sums = {};
    for (const b of budgetRows) {
      const cur = String(b.currency).toUpperCase();
      sums[cur] = (sums[cur] || 0) + Number(b[field].units) / 10 ** b[field].scale;
    }
    return sums;
  };
  const consumed = totals("consumed_amount");
  const limits = totals("amount_limit");
  const consumedText = Object.keys(consumed).length
    ? Object.entries(consumed).map(([c, v]) => `${v.toFixed(2)} ${c}`).join(" · ") : "0.00";
  const limitText = Object.entries(limits).map(([c, v]) => `of ${v.toFixed(2)} ${c} in limits`).join(" · ");
  const denials = (s.automatic_denials || []).reduce((n, d) => n + (d.distinct_attempts || 0), 0);
  const activeAgents = agents.filter((a) => a.status === "active").length;
  const activeUsers = (users.users || []).filter((u) => u.status === "active").length;
  const nearLimit = budgetRows.filter((b) => {
    const limit = Number(b.amount_limit.units);
    return limit > 0 && (Number(b.consumed_amount.units) + Number(b.frozen_amount.units)) / limit >= 0.8;
  }).length;
  const overrides = policyRows.filter((p) => p.agent_id).length;
  const hasDefault = policyRows.some((p) => !p.agent_id);
  const exceededTargets = (targets.targets || []).filter((t) => t.is_exceeded).length;

  append(view, [
    section("People & agents", "Who can spend, and on whose behalf.", [
      h("div", { class: "stats" }, [
        stat("Human users", activeUsers, `${(users.users || []).length} registered · manage →`, { href: "#/users" }),
        stat("Active agents", activeAgents, `${agents.length} registered · manage →`, { href: "#/agents" }),
        stat("Inactive agents", agents.length - activeAgents, "suspended or retired", { href: "#/agents" }),
      ]),
    ]),
    section("Spend lifecycle", "Requests, decisions and settlement, from policy check to provider receipt.", [
      h("div", { class: "stats" }, [
        stat("Pending approvals", backlog.pending_human_approvals ?? 0, "waiting on a human · review →",
          { tone: "warn", href: "#/approvals" }),
        stat("Automatic denials", denials, "policy and budget · view →",
          { tone: "bad", href: "#/decisions?outcomes=automatic_deny" }),
        stat("Unresolved reconciliations", backlog.unresolved_reconciliations ?? 0, "uncertain executor claims · view →",
          { tone: "warn", href: "#/decisions?outcomes=reconciliation_required" }),
        stat("Spend attempts", s.distinct_attempts ?? 0, `${s.distinct_operations ?? 0} operations · history →`, { href: "#/spend" }),
      ]),
      card("Recent decisions", decisionTable(records.records || []),
        [h("a", { href: "#/decisions", class: "hint" }, "All decisions →")], true),
    ]),
    section("Policies & budgets", "The limits that decide what agents may spend.", [
      h("div", { class: "stats" }, [
        stat("Spent in active budgets", consumedText, limitText || "no active budgets", { href: "#/budgets" }),
        stat("Budgets near their limit", nearLimit, `80%+ used or held · of ${budgetRows.length} · view →`,
          { tone: "warn", href: "#/budgets" }),
        stat("Policies", policyRows.length,
          `${hasDefault ? "owner default" : "no owner default"} · ${overrides} agent override${overrides === 1 ? "" : "s"} · manage →`,
          { href: "#/policies" }),
        stat("Spending targets exceeded", exceededTargets, `${(targets.targets || []).length} active advisory target${(targets.targets || []).length === 1 ? "" : "s"} · view →`,
          { tone: "warn", href: "#/users" }),
      ]),
      card("Active budgets", budgetTable(budgetRows, false), [h("a", { href: "#/budgets", class: "hint" }, "Manage budgets →")], true),
    ]),
  ]);
}

/** A titled group of related tiles and tables on the overview. */
function section(title, description, children) {
  return h("section", { class: "overview-section", "aria-label": title },
    h("div", { class: "section-head" }, h("h2", {}, title), h("p", { class: "hint" }, description)),
    children);
}

/** A summary tile. `tone` colors it only when the value is non-zero, so an
 * empty backlog stays calm; `href` makes the whole tile a link. */
function stat(label, value, sub, { tone, href } = {}) {
  const active = tone && Number(value) > 0;
  const classes = ["card", "stat", active ? `tone-${tone}` : "", href ? "stat-link" : ""].join(" ").trim();
  const content = [h("div", { class: "label" }, label),
    h("div", { class: "value" }, String(value)), h("div", { class: "sub" }, sub)];
  return href
    ? h("a", { class: classes, href, "aria-label": `${label}: ${value}. Open` }, content)
    : h("div", { class: classes }, content);
}

// Approvals

async function renderApprovals(view) {
  await loadAgents();
  const data = await hubu("/spend/authorizations", { status: "needs_approval", limit: "100" });
  const rows = data.authorization_records || [];
  state.counts.approvals = rows.length;
  updateNavCounts();
  if (!state.session.approval_capability) {
    view.append(h("div", { class: "notice warn" },
      "No human approval capability is configured for this Hubu connection, so approvals can be reviewed but not resolved here."));
  }
  view.append(card(`Waiting for a decision (${rows.length})`, table([
    { label: "Requested", render: (r) => when(r.created_at) },
    { label: "Agent", render: (r) => agentName(r.agent_id) },
    { label: "Amount", num: true, render: (r) => money(r.authorized_max) },
    { label: "Billing merchant", render: (r) => r.billing_merchant || r.provider },
    { label: "Policy", render: (r) => badge(r.policy_decision) },
    { label: "", render: (r) => h("button", { class: "small", type: "button", onclick: () => reviewApproval(r.authorization_id) }, "Review") },
  ], rows, "No spend is waiting for approval."), null, true));
}

async function reviewApproval(approvalRequestId) {
  let approval;
  try {
    approval = await hubu("/spend/approval", { approval_request_id: approvalRequestId });
  } catch (error) {
    toast(error.message, "bad");
    return;
  }
  const r = approval.review || {};
  const scope = r.execution_scope || {};
  const summary = h("div", {}, kv([
    ["Status", badge(approval.status)],
    ["Agent", agentName(r.agent_id)],
    ["Amount", money(r.amount)],
    ["Merchant", r.merchant],
    ["Reason", r.reason],
    ["Policy", r.policy_summary],
    (r.policy_reasons || []).length ? ["Policy reasons", (r.policy_reasons || []).join("; ")] : null,
    ["Provider", scope.provider ? scope.provider.display_name : null],
    ["Capability", scope.capability ? scope.capability.display_name : null],
    ["Lease profile", r.lease_profile],
    ["Request ID", short(approval.approval_request_id, 40)],
  ]));
  if (approval.status !== "pending" || !state.session.approval_capability) {
    openDialog("Spend approval", [summary,
      approval.status !== "pending" ? h("div", { class: "notice" }, "This request is no longer pending.") : null]);
    return;
  }
  const decide = (decision) => confirmCapability({
    title: decision === "approve" ? "Approve spend" : "Deny spend",
    summary,
    confirmLabel: decision === "approve" ? "Approve" : "Deny",
    danger: decision === "deny",
    onConfirm: async () => {
      await hubuPost("/spend/approval/resolve", { approval_request_id: approvalRequestId, decision }, { confirmed: true });
      toast(decision === "approve" ? "Spend approved; budget reserved." : "Spend denied.");
      route();
    },
  });
  openDialog("Spend approval", [summary,
    h("p", { class: "hint" }, "Approving reserves budget and returns an authorization to the agent. It never calls a provider. Denial is final for this request."),
  ], (close) => [
    h("button", { type: "button", class: "danger", onclick: () => { close(); decide("deny"); } }, "Deny…"),
    h("button", { type: "button", onclick: () => { close(); decide("approve"); } }, "Approve…"),
  ]);
}

// Spend history

async function renderSpend(view) {
  await loadAgents();
  const authBody = h("div", {});
  const ledgerBody = h("div", {});
  const agentSelect = () => h("select", { name: "agent_id" }, h("option", { value: "" }, "All agents"),
    state.agents.map((a) => h("option", { value: a.agent_id }, a.display_name)));

  const authFilters = h("form", { class: "filters" },
    h("label", {}, "Agent", agentSelect()),
    h("label", {}, "Status", h("select", { name: "status" }, h("option", { value: "" }, "Any"),
      ["authorized", "needs_approval", "claimed", "settled", "released", "expired", "reconciliation_required", "unknown"]
        .map((s) => h("option", { value: s }, s.replaceAll("_", " "))))),
    h("button", { type: "submit", class: "ghost" }, "Apply"));
  const ledgerFilters = h("form", { class: "filters" },
    h("label", {}, "Agent", agentSelect()),
    h("button", { type: "submit", class: "ghost" }, "Apply"));

  const loadAuth = pagedLoader(authBody, (cursor) => hubu("/spend/authorizations",
    { ...formValues(authFilters), limit: "50", cursor }), (data) => data.authorization_records || [], (rows) => table([
    { label: "Created", render: (r) => when(r.created_at) },
    { label: "Agent", render: (r) => agentName(r.agent_id) },
    { label: "Decision", render: (r) => badge(r.decision) },
    { label: "Status", render: (r) => badge(r.status) },
    { label: "Authorized max", num: true, render: (r) => money(r.authorized_max) },
    { label: "Charged", num: true, render: (r) => (r.receipt ? money(r.receipt.actual_vendor_cost) : null) },
    { label: "Merchant / provider", render: (r) => r.billing_merchant || r.provider },
    { label: "", render: (r) => h("button", { class: "ghost small", type: "button",
      onclick: () => showJsonDialog("Authorization record", () => hubu("/spend/authorizations/show", { authorization_id: r.authorization_id })) }, "Details") },
  ], rows, "No authorization records match."));

  const loadLedger = pagedLoader(ledgerBody, (cursor) => hubu("/ledger/transactions",
    { ...formValues(ledgerFilters), limit: "50", cursor }), (data) => data.transactions || [], (rows) => table([
    { label: "Posted", render: (t) => when(t.created_at) },
    { label: "Agent", render: (t) => agentName(t.agent_id) },
    { label: "Kind", render: (t) => badge(t.kind) },
    { label: "Cost", num: true, render: (t) => money(t.effective_cost) },
    { label: "Budget", render: (t) => short(t.budget_id) },
    { label: "Provider", render: (t) => t.provider || t.billing_merchant },
    { label: "", render: (t) => h("button", { class: "ghost small", type: "button", onclick: () => openDialog("Ledger transaction", jsonBlock(t)) }, "Entries") },
  ], rows, "No ledger transactions yet."));

  authFilters.addEventListener("submit", (e) => { e.preventDefault(); loadAuth(); });
  ledgerFilters.addEventListener("submit", (e) => { e.preventDefault(); loadLedger(); });
  append(view, [
    card("Spend authorizations", [h("div", { class: "body" }, authFilters), authBody], null, true),
    card("Ledger transactions", [h("div", { class: "body" }, ledgerFilters), ledgerBody], null, true),
  ]);
  await Promise.all([loadAuth(), loadLedger()]);
}

/** Cursor pagination: renders the first page, then appends on "Load more". */
function pagedLoader(container, fetchPage, pick, renderRows) {
  let rows = [];
  let cursor = null;
  const draw = () => {
    clear(container).append(renderRows(rows));
    if (cursor) {
      container.append(h("div", { class: "pager" }, h("button", { class: "ghost small", type: "button", onclick: more }, "Load more")));
    }
  };
  const more = async () => {
    try {
      const data = await fetchPage(cursor);
      rows = rows.concat(pick(data));
      cursor = data.next_cursor || null;
      draw();
    } catch (error) {
      container.append(h("div", { class: "notice bad" }, error.message));
    }
  };
  return async () => {
    rows = [];
    cursor = null;
    clear(container).append(h("p", { class: "empty" }, "Loading…"));
    await more();
  };
}

// Decisions

// Mirrors GovernanceOutcome in crates/hubu-core/src/spend/governance_query.rs.
const OUTCOMES = ["policy_allow", "policy_deny", "policy_needs_approval", "automatic_allow", "automatic_deny",
  "admission_failed", "escalated", "human_approved", "human_declined", "authorized", "authorization_released",
  "execution_started", "execution_failed", "execution_settled", "execution_released", "reconciliation_required",
  "reconciliation_resolved"];

// Mirrors GovernanceCategory in crates/hubu-core/src/spend/governance_query.rs.
const CATEGORIES = ["initial_policy", "automated_admission", "admission_failure", "human_escalation",
  "human_resolution", "authorization", "authorization_release", "execution_start", "execution_failure",
  "execution_finalization", "reconciliation_entry", "reconciliation_resolution"];

async function renderDecisions(view) {
  await loadAgents();
  const body = h("div", {});
  const summaryBox = h("div", {});
  const filters = h("form", { class: "filters" },
    h("label", {}, "Agent", h("select", { name: "agent_id" }, h("option", { value: "" }, "All agents"),
      state.agents.map((a) => h("option", { value: a.agent_id }, a.display_name)))),
    h("label", {}, "Category", h("select", { name: "categories" }, h("option", { value: "" }, "Any"),
      CATEGORIES.map((c) => h("option", { value: c }, c.replaceAll("_", " "))))),
    h("label", {}, "Outcome", h("select", { name: "outcomes" }, h("option", { value: "" }, "Any"),
      OUTCOMES.map((o) => h("option", { value: o }, o.replaceAll("_", " "))))),
    h("label", {}, "From", h("input", { type: "datetime-local", name: "from" })),
    h("label", {}, "Before", h("input", { type: "datetime-local", name: "before" })),
    h("button", { type: "submit", class: "ghost" }, "Apply"));
  const query = () => {
    const v = formValues(filters);
    return { agent_id: v.agent_id, categories: v.categories, outcomes: v.outcomes,
      occurred_from: isoFromLocal(v.from), occurred_before: isoFromLocal(v.before) };
  };
  const load = pagedLoader(body, async (cursor) => {
    const data = await hubu("/governance/records", { ...query(), limit: "50", cursor });
    if (!cursor) drawDecisionSummary(summaryBox, data.summary || {});
    return data;
  }, (data) => data.records || [], (rows) => decisionTable(rows));
  // Links such as the overview's denial tile open this view pre-filtered.
  for (const name of ["agent_id", "categories", "outcomes"]) {
    const value = state.routeParams.get(name);
    if (value && [...filters.elements[name].options].some((o) => o.value === value)) filters.elements[name].value = value;
  }
  filters.addEventListener("submit", (e) => { e.preventDefault(); load(); });
  append(view, [summaryBox, card("Decision records", [h("div", { class: "body" }, filters), body], null, true)]);
  await load();
}

function drawDecisionSummary(container, s) {
  const backlog = s.backlog || {};
  const denials = (s.automatic_denials || []);
  clear(container).append(h("div", { class: "stats" }, [
    stat("Facts", s.fact_count ?? 0, `${s.distinct_attempts ?? 0} attempts`),
    stat("Pending approvals", backlog.pending_human_approvals ?? 0, "backlog now", { tone: "warn", href: "#/approvals" }),
    stat("Automatic denials", denials.reduce((n, d) => n + d.distinct_attempts, 0),
      denials.map((d) => `${d.reason}: ${d.distinct_attempts}`).join(" · ") || "none", { tone: "bad" }),
    stat("Denied amount", (s.automatic_denied_requested?.per_currency || []).map(money).join(" · ") || "0.00", "requested, auto-denied"),
  ]));
}

function decisionTable(rows) {
  return table([
    { label: "When", render: (r) => when(r.occurred_at) },
    { label: "Agent", render: (r) => agentName(r.agent_id) },
    { label: "Category", render: (r) => String(r.category || "").replaceAll("_", " ") },
    { label: "Outcome", render: (r) => badge(r.outcome) },
    { label: "Amount", num: true, render: (r) => (r.requested_amount?.coverage === "known" ? money(r.requested_amount.value) : null) },
    { label: "Merchant", render: (r) => (r.execution_scope?.coverage === "known" ? r.execution_scope.value.billing_merchant : null) },
    { label: "Policy", render: (r) => (r.policy?.coverage === "known" ? short(r.policy.value.policy_id) : null) },
    { label: "", render: (r) => h("button", { class: "ghost small", type: "button",
      onclick: () => showJsonDialog("Decision record", async () => (await hubu(`/governance/records/${encodeURIComponent(r.record_id)}`)).record) }, "Details") },
  ], rows, "No decision records match.");
}

// Agents

async function renderAgents(view) {
  const showAll = h("input", { type: "checkbox", name: "all" });
  const listBody = h("div", {});
  const draw = async () => {
    const data = await hubu("/agents", showAll.checked ? { all: "true" } : {});
    if (!showAll.checked) state.agents = data.agents || state.agents;
    const mine = (a) => !state.me || a.owner_user_id === state.me.user_id;
    clear(listBody).append(table([
      { label: "Name", render: (a) => h("div", { class: "name-cell" },
        h("strong", {}, a.display_name),
        mine(a) ? h("button", { class: "icon-button", type: "button", title: `Rename ${a.display_name}`,
          "aria-label": `Rename ${a.display_name}`, onclick: () => renameAgent(a, draw) }, "✎ Rename") : null) },
      { label: "Agent ID", render: (a) => short(a.agent_id, 20) },
      { label: "Account", render: (a) => short(a.account_id, 20) },
      { label: "Owner", render: (a) => a.owner_username },
      { label: "Status", render: (a) => badge(a.status) },
      { label: "Registered", render: (a) => when(a.created_at) },
      { label: "", render: (a) => h("button", { class: "ghost small", type: "button", onclick: () => showAgentHistory(a) }, "Name history") },
    ], data.agents, "No agents registered yet."));
  };
  showAll.addEventListener("change", draw);

  const reviewBox = h("div", {});
  const form = h("form", { class: "grid" },
    h("label", {}, "Agent name", h("input", { name: "name", required: true, maxlength: 128, placeholder: "Research assistant" })),
    h("label", {}, "Version label", h("input", { name: "version_label", required: true, maxlength: 128, placeholder: "v1" })),
    h("div", { class: "form-actions" }, h("button", { type: "submit" }, "Review registration")),
    h("div", { class: "wide" }, reviewBox));
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const values = formValues(form);
    try {
      const preview = await api("POST", "/api/agents/registration", { ...values, submit: false });
      const submit = h("button", { type: "button" }, "Register agent");
      submit.addEventListener("click", async () => {
        submit.disabled = true;
        try {
          const result = await api("POST", "/api/agents/registration", { ...values, submit: true });
          const reused = ["reused", "alias"].includes(result.identity_resolution);
          toast(reused ? `Already registered; reused ${result.agent_id}.` : `Registered ${result.agent_id}.`);
          for (const warning of result.warnings || []) toast(warning.message, "bad");
          clear(reviewBox);
          form.reset();
          draw();
        } catch (error) {
          submit.disabled = false;
          toast(error.message, "bad");
        }
      });
      clear(reviewBox).append(h("div", { class: "card" }, h("div", { class: "body" },
        h("p", { class: "hint" }, "Review what will be registered. Owner and runtime fields come from the active Hubu session; fingerprints are computed by the client and re-checked by the server."),
        kv((preview.review || []).map((r) => [r.field, r.value])),
        h("div", { class: "form-actions" }, submit,
          h("button", { type: "button", class: "ghost", onclick: () => clear(reviewBox) }, "Cancel")))));
    } catch (error) {
      toast(error.message, "bad");
    }
  });

  append(view, [
    card("Register an agent", [h("p", { class: "hint" }, "Agents usually register themselves through the Hubu MCP or CLI. Use this to pre-register an agent by name and version label."), h("br"), form]),
    card("Registered agents", listBody, [h("label", { class: "check" }, showAll, "Include other users' agents")], true),
  ]);
  await draw();
}

function showAgentHistory(agent) {
  showJsonDialog(`Identity history: ${agent.display_name}`, async () => {
    const data = await hubu("/agents/history", { agent_id: agent.agent_id });
    return h("div", {}, kv([
      ["Agent ID", short(data.agent_id, 40)],
      ["Current name", data.display_name],
      ["Revision", data.current_revision],
      ["Registered", when(data.registered_at)],
      ["Identity fingerprint", short(data.identity_fingerprint, 40)],
    ]), h("br"), table([
      { label: "Revision", render: (r) => r.revision },
      { label: "Change", render: (r) => {
        const name = (r.changes || []).find((c) => c.field === "display_name") || (r.changes || [])[0];
        return name ? `${name.old_value} → ${name.new_value}` : null;
      } },
      { label: "Reason", render: (r) => r.reason },
      { label: "When", render: (r) => when(r.created_at) },
    ], data.revisions || [], "Never renamed."));
  });
}

/** Owner-only relabel. Sends the human approval capability, so the request
 * carries the confirmation header; submitting this dialog is the confirmation. */
function renameAgent(agent, done) {
  const status = h("div", {});
  const nameInput = h("input", { name: "name", required: true, maxlength: 128, value: agent.display_name });
  const form = h("form", { class: "grid" },
    h("label", { class: "wide" }, "New name", nameInput),
    h("label", { class: "wide" }, "Reason for the change", h("input", { name: "reason", required: true, maxlength: 512, placeholder: "e.g. clarify the agent's role" })));
  const save = h("button", { type: "button", onclick: () => form.requestSubmit() }, "Save name");
  const { close } = openDialog(`Rename ${agent.display_name}`, [
    kv([["Agent ID", h("span", { class: "mono" }, agent.agent_id)], ["Current name", agent.display_name]]),
    form,
    h("p", { class: "hint" }, "Only the name changes. The agent ID, account, budgets and policies stay the same, and the old name stays reserved for this agent. Renaming uses the human approval capability."),
    status,
  ], (closeFn) => [h("button", { type: "button", class: "ghost", onclick: closeFn }, "Cancel"), save]);
  setTimeout(() => { nameInput.focus(); nameInput.select(); }, 0);
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const values = formValues(form);
    if (values.name === agent.display_name) {
      clear(status).append(h("div", { class: "notice warn" }, "Enter a different name."));
      return;
    }
    save.disabled = true;
    try {
      await hubuPost("/agents/rename", { agent_id: agent.agent_id, name: values.name, reason: values.reason }, { confirmed: true });
      toast(`Renamed to ${values.name}.`);
      close();
      done();
    } catch (error) {
      save.disabled = false;
      clear(status).append(h("div", { class: "notice bad" }, error.message));
    }
  });
}

// Users and spending targets

async function renderUsers(view) {
  const [users, me] = await Promise.all([hubu("/users"), hubu("/user")]);
  const targetsBody = h("div", {});
  const showAll = h("input", { type: "checkbox" });
  const drawTargets = async () => {
    const data = await hubu("/user/spending-target", showAll.checked ? { all: "true" } : {});
    clear(targetsBody).append(table([
      { label: "Target ID", render: (t) => short(t.target_id, 20) },
      { label: "Status", render: (t) => badge(t.status) },
      { label: "Amount", num: true, render: (t) => money(t.target_amount) },
      { label: "Allocated to budgets", num: true, render: (t) => [money(t.allocated_amount), t.is_exceeded ? [" ", badge("exceeded")] : null] },
      { label: "Period", render: (t) => [when(t.starting_at), " → ", t.ending_before ? when(t.ending_before) : "open-ended"] },
      { label: "", render: (t) => (t.status === "active" ? h("button", { class: "ghost small", type: "button", onclick: async () => {
        if (!window.confirm("Revoke this advisory spending target?")) return;
        try { await hubuPost("/user/spending-target/revoke", { target_id: t.target_id }); toast("Spending target revoked."); drawTargets(); } catch (e) { toast(e.message, "bad"); }
      } }, "Revoke") : null) },
    ], data.targets, "No spending targets configured."));
  };
  showAll.addEventListener("change", drawTargets);

  const targetForm = h("form", { class: "grid" },
    h("label", {}, "Amount (USD)", h("input", { name: "amount", required: true, inputmode: "decimal", placeholder: "1000.00" })),
    h("label", {}, "Starting at", h("input", { name: "starting_at", type: "datetime-local" })),
    h("label", {}, "Ending before", h("input", { name: "ending_before", type: "datetime-local" })),
    h("div", { class: "form-actions" }, h("button", { type: "submit" }, "Set target")));
  targetForm.addEventListener("submit", async (event) => {
    event.preventDefault();
    const v = formValues(targetForm);
    try {
      await hubuPost("/user/spending-target", { amount: v.amount, starting_at: isoFromLocal(v.starting_at), ending_before: isoFromLocal(v.ending_before) });
      toast("Spending target set (advisory).");
      targetForm.reset();
      drawTargets();
    } catch (error) { toast(error.message, "bad"); }
  });

  const userForm = h("form", { class: "grid" },
    h("label", {}, "Username", h("input", { name: "username", required: true, pattern: "[a-z0-9-]{3,32}", placeholder: "alice" })),
    h("label", {}, "Display name", h("input", { name: "display_name", required: true, placeholder: "Alice Example" })),
    h("label", {}, "Email (optional)", h("input", { name: "email", type: "email" })),
    h("div", { class: "form-actions" }, h("button", { type: "submit" }, "Register human")));
  userForm.addEventListener("submit", async (event) => {
    event.preventDefault();
    const v = formValues(userForm);
    try {
      const result = await hubuPost("/init", { username: v.username, display_name: v.display_name, email: v.email || null });
      toast(`Registered ${result.user_id}.`);
      route();
    } catch (error) { toast(error.message, "bad"); }
  });

  append(view, [
    h("div", { class: "notice" }, `Signed in to Hubu as ${me.display_name} (${me.username || me.user_id}). Hubu admin actions apply to this owner.`),
    card("Human users", table([
      { label: "User ID", render: (u) => short(u.user_id, 20) },
      { label: "Username", render: (u) => u.username },
      { label: "Display name", render: (u) => u.display_name },
      { label: "Email", render: (u) => u.email },
      { label: "Status", render: (u) => badge(u.status) },
      { label: "Current", render: (u) => (u.current ? badge("active") : null) },
      { label: "Created", render: (u) => when(u.created_at) },
    ], users.users, "No human users registered."), null, true),
    card("Register a human user", userForm),
    card("Advisory spending targets", targetsBody, [h("label", { class: "check" }, showAll, "Include revoked")], true),
    card("Set a spending target", [h("p", { class: "hint" }, "Spending targets are advisory: Hubu warns when budgets exceed them but does not block spend."), h("br"), targetForm]),
  ]);
  await drawTargets();
}

// Policies

const POLICY_TEMPLATE = `id: my_policy
version: v1
default_effect: needs_approval
rules:
  - id: allow_small_spend
    effect: allow
    reason: small spends are approved automatically
    when:
      op: lte
      field: amount
      value:
        money: "25.00"
`;

/** Export YAML carries the internal owner id; the server always sets it. */
function stripOwner(yaml) {
  return String(yaml || "").replace(/^owner_user_id: .*\n/m, "");
}

/** Line diff (LCS) for small documents such as policy YAML. */
function lineDiff(before, after) {
  const a = before.split("\n");
  const b = after.split("\n");
  const dp = Array.from({ length: a.length + 1 }, () => new Array(b.length + 1).fill(0));
  for (let i = a.length - 1; i >= 0; i--) {
    for (let j = b.length - 1; j >= 0; j--) {
      dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
    }
  }
  const out = [];
  let i = 0;
  let j = 0;
  while (i < a.length && j < b.length) {
    if (a[i] === b[j]) { out.push([" ", a[i]]); i++; j++; }
    else if (dp[i + 1][j] >= dp[i][j + 1]) out.push(["-", a[i++]]);
    else out.push(["+", b[j++]]);
  }
  while (i < a.length) out.push(["-", a[i++]]);
  while (j < b.length) out.push(["+", b[j++]]);
  return out;
}

function diffView(before, after) {
  const lines = lineDiff(before, after);
  if (!lines.some(([kind]) => kind !== " ")) return h("p", { class: "hint" }, "No differences.");
  return h("pre", { class: "code diff" }, lines.map(([kind, text]) =>
    h("span", { class: kind === "+" ? "diff-add" : kind === "-" ? "diff-del" : null }, `${kind} ${text}\n`)));
}

function rulesTable(rules) {
  return table([
    { label: "#", num: true, render: (r) => r.index },
    { label: "Rule", render: (r) => h("span", { class: "mono" }, r.id) },
    { label: "Decision", render: (r) => badge(r.effect) },
    { label: "Applies when", render: (r) => r.condition },
    { label: "Reason", render: (r) => r.reason },
  ], (rules || []).map((r, i) => ({ index: i + 1, ...r })), "This policy has no rules; every request gets the default decision.");
}

function segmented(options, active, onChange) {
  return h("div", { class: "segmented", role: "tablist" }, options.map(([value, label]) =>
    h("button", { type: "button", role: "tab", "aria-selected": value === active ? "true" : "false",
      class: value === active ? "active" : null, onclick: () => value !== active && onChange(value) }, label)));
}

const renderPolicyRevision = (policy) => api("POST", "/api/policies/render", { policy });

async function renderPolicies(view) {
  await loadAgents();
  const data = await hubu("/policies");
  const policies = data.policies || [];
  const workspace = h("div", { class: "stack" });
  const known = policies.some((p) => p.policy_id === state.selectedPolicy);
  let selected = known ? state.selectedPolicy : policies[0]?.policy_id;

  const listBody = h("div", {});
  const drawList = () => clear(listBody).append(table([
    { label: "Name", render: (p) => h("button", { type: "button", class: `link-button${p.policy_id === selected ? " selected" : ""}`,
      onclick: () => open(p.policy_id) }, p.display_name) },
    { label: "Applies to", render: (p) => (p.agent_id ? agentName(p.agent_id) : "All agents (owner default)") },
    { label: "Version", render: (p) => p.policy_version },
    { label: "Default", render: (p) => badge(p.default_decision) },
    { label: "Rules", num: true, render: (p) => p.rules },
    { label: "Revision", num: true, render: (p) => p.revision },
    { label: "Updated", render: (p) => when(p.updated_at) },
  ], policies, "No policies yet. The server's built-in default applies until you add one."));
  const open = (policyId, options) => {
    selected = policyId;
    state.selectedPolicy = policyId;
    drawList();
    openPolicy(workspace, policies.find((p) => p.policy_id === policyId), options);
  };
  drawList();
  append(view, [
    card("Policies", listBody, [h("button", { type: "button", class: "small", onclick: () => {
      selected = null;
      drawList();
      newPolicyEditor(workspace);
    } }, "New policy")], true),
    workspace,
  ]);
  if (selected) await openPolicy(workspace, policies.find((p) => p.policy_id === selected));
}

/** Read-only and edit views of one policy, at any revision. */
async function openPolicy(container, policy, options = {}) {
  clear(container).append(h("p", { class: "hint" }, "Loading policy…"));
  let history;
  let exported;
  try {
    [history, exported] = await Promise.all([
      hubu("/policies/history", { policy_id: policy.policy_id }),
      hubu("/policies/export", { policy_id: policy.policy_id }),
    ]);
  } catch (error) {
    clear(container).append(h("div", { class: "notice bad" }, error.message));
    return;
  }
  const revisions = [...(history.revisions || [])].sort((a, b) => b.revision - a.revision);
  const current = exported.revision ?? policy.revision;
  const view = { mode: options.mode || "read", revision: options.revision || current, draft: options.draft };
  const rendered = new Map();
  const renderedAt = async (number) => {
    if (!rendered.has(number)) {
      const revision = revisions.find((r) => r.revision === number);
      rendered.set(number, revision ? await renderPolicyRevision(revision.policy) : null);
    }
    return rendered.get(number);
  };
  const scope = policy.agent_id ? ["Agent override", agentName(policy.agent_id)] : ["Owner default", "All agents without their own policy"];

  const draw = async () => {
    const body = h("div", { class: "body stack" });
    const header = h("header", {},
      h("div", {}, h("h2", {}, policy.display_name),
        h("div", { class: "hint" }, `${scope[0]} · ${policy.declarative_key || policy.policy_id}`)),
      segmented([["read", "Read-only"], ["edit", "Edit"]], view.mode, (mode) => {
        view.mode = mode;
        if (mode === "edit" && view.draft === undefined) view.draft = stripOwner(exported.policy_yaml);
        draw();
      }));
    clear(container).append(h("div", { class: "card" }, header, body));
    try {
      if (view.mode === "read") await drawRead(body);
      else drawEdit(body);
    } catch (error) {
      clear(body).append(h("div", { class: "notice bad" }, error.message));
    }
  };

  const drawRead = async (body) => {
    const revision = revisions.find((r) => r.revision === view.revision) || revisions[0];
    const info = await renderedAt(revision.revision);
    const isCurrent = revision.revision === current;
    const picker = h("select", { "aria-label": "Revision", onchange: (e) => { view.revision = Number(e.target.value); draw(); } },
      revisions.map((r) => h("option", { value: r.revision, selected: r.revision === revision.revision },
        `Revision ${r.revision}${r.revision === current ? " (current)" : ""} · ${new Date(r.created_at).toLocaleString()}`)));
    const compareBox = h("div", {});
    append(body, [
      h("div", { class: "toolbar" }, h("label", { class: "inline" }, "Showing", picker),
        isCurrent ? badge("current") : null),
      isCurrent ? null : h("div", { class: "notice warn" },
        `You are viewing revision ${revision.revision}. Revision ${current} is the one in effect.`,
        h("div", { class: "row-actions spaced" },
          h("button", { type: "button", class: "ghost small", onclick: async () => {
            const now = await renderedAt(current);
            clear(compareBox).append(h("h3", {}, `Changes from revision ${revision.revision} to revision ${current}`),
              diffView(stripOwner(info.policy_yaml), stripOwner(now.policy_yaml)));
          } }, "Compare with current"),
          h("button", { type: "button", class: "ghost small", onclick: () => {
            view.mode = "edit";
            view.draft = stripOwner(info.policy_yaml);
            draw();
          } }, "Restore as a new revision…"))),
      compareBox,
      kv([
        ["Applies to", [h("strong", {}, scope[0]), " · ", scope[1]]],
        ["Policy", h("span", { class: "mono" }, `${info.id} @ ${info.version}`)],
        ["Default decision", [badge(info.default_effect), h("span", { class: "hint" }, "  used when no rule decides the request")]],
        ["Saved", [when(revision.created_at), ` via ${revision.source || "unknown"}`]],
        ["Content hash", short(revision.payload_hash, 24)],
      ]),
      h("h3", {}, `Rules (${(info.rules || []).length})`),
      h("div", { class: "flush-table" }, rulesTable(info.rules)),
      (info.warnings || []).length ? h("div", { class: "notice warn" }, info.warnings.join(" ")) : null,
      h("details", {}, h("summary", {}, "YAML source"), h("pre", { class: "code" }, stripOwner(info.policy_yaml))),
      h("details", {}, h("summary", {}, `Change log (${(history.audit || []).length})`), table([
        { label: "When", render: (a) => when(a.occurred_at) },
        { label: "Action", render: (a) => badge(a.action) },
        { label: "Source", render: (a) => a.source },
        { label: "Assignments", render: (a) => (a.affected_assignments || []).join(", ") },
      ], history.audit || [], "No changes recorded.")),
    ]);
  };

  const drawEdit = (body) => {
    policyEditor(body, {
      yaml: view.draft ?? stripOwner(exported.policy_yaml),
      intro: `Saving creates revision ${current + 1}. Earlier revisions stay in the history, and the change is rejected if someone else saves first.`,
      submitLabel: `Save as revision ${current + 1}`,
      onDraft: (text) => { view.draft = text; },
      onCancel: () => { view.mode = "read"; view.draft = undefined; draw(); },
      submit: async (policyYaml) => {
        const request = { policy_yaml: policyYaml, source: "web_ui", expected_revision: current,
          display_name: policy.display_name, declarative_key: policy.declarative_key };
        if (policy.agent_id) request.agent_id = policy.agent_id;
        const result = await hubuPost("/policies", request);
        toast(result.changed === false ? "No changes: the content matches the current revision." : `Saved revision ${result.revision ?? current + 1}.`);
        route();
      },
    });
  };

  await draw();
}

/** YAML editor with server-equivalent validation before saving. */
function policyEditor(container, { yaml, intro, submitLabel, onDraft, onCancel, submit, extraFields }) {
  const textarea = h("textarea", { class: "code-editor", spellcheck: "false", "aria-label": "Policy YAML" });
  textarea.value = yaml;
  const result = h("div", {});
  const save = h("button", { type: "button", disabled: true }, submitLabel);
  const stale = h("span", { class: "hint" }, "Validate to enable saving.");
  let validatedText = null;
  const validate = async () => {
    try {
      const outcome = await api("POST", "/api/policies/validate", { policy_yaml: textarea.value });
      if (!outcome.valid) {
        validatedText = null;
        save.disabled = true;
        stale.textContent = "Fix the error, then validate again.";
        clear(result).append(h("div", { class: "notice bad" }, h("strong", {}, "Not valid. "), outcome.error));
        return;
      }
      validatedText = textarea.value;
      save.disabled = false;
      stale.textContent = "Valid. Saving sends exactly this text.";
      clear(result).append(
        h("div", { class: "notice ok" }, h("strong", {}, "Valid. "),
          `${outcome.id} @ ${outcome.version}: ${outcome.rules.length} rule(s), default decision `, badge(outcome.default_effect), "."),
        (outcome.warnings || []).length ? h("div", { class: "notice warn" }, outcome.warnings.join(" ")) : null,
        h("div", { class: "flush-table" }, rulesTable(outcome.rules)));
    } catch (error) {
      clear(result).append(h("div", { class: "notice bad" }, error.message));
    }
  };
  textarea.addEventListener("input", () => {
    onDraft?.(textarea.value);
    if (validatedText !== null && textarea.value !== validatedText) {
      save.disabled = true;
      stale.textContent = "Changed since the last validation. Validate again to save.";
    }
  });
  save.addEventListener("click", async () => {
    if (textarea.value !== validatedText) return;
    save.disabled = true;
    try { await submit(textarea.value); } catch (error) {
      save.disabled = false;
      clear(result).append(h("div", { class: "notice bad" }, error.message));
    }
  });
  append(container, [
    h("p", { class: "hint" }, intro),
    extraFields || null,
    textarea,
    h("div", { class: "form-actions" },
      h("button", { type: "button", class: "ghost", onclick: validate }, "Validate"),
      save,
      onCancel ? h("button", { type: "button", class: "ghost", onclick: onCancel }, "Discard changes") : null,
      stale),
    result,
  ]);
}

function newPolicyEditor(container) {
  const fields = h("form", { class: "grid" },
    h("label", {}, "Applies to", h("select", { name: "agent_id" }, h("option", { value: "" }, "All agents (owner default)"),
      state.agents.map((a) => h("option", { value: a.agent_id }, a.display_name)))),
    h("label", {}, "Display name (optional)", h("input", { name: "display_name" })),
    h("label", {}, "Policy key (optional)", h("input", { name: "declarative_key", placeholder: "defaults to the YAML id" })));
  const body = h("div", { class: "body stack" });
  clear(container).append(h("div", { class: "card" }, h("header", {}, h("h2", {}, "New policy")), body));
  policyEditor(body, {
    yaml: POLICY_TEMPLATE,
    intro: "Write the policy in YAML, validate it, then save. Saving a policy for a scope that already has one replaces that assignment.",
    submitLabel: "Create policy",
    extraFields: fields,
    onCancel: () => route(),
    submit: async (policyYaml) => {
      const v = formValues(fields);
      const request = { policy_yaml: policyYaml, source: "web_ui" };
      for (const key of ["agent_id", "display_name", "declarative_key"]) if (v[key]) request[key] = v[key];
      const result = await hubuPost("/policies", request);
      state.selectedPolicy = result.policy_id || state.selectedPolicy;
      toast("Policy created.");
      route();
    },
  });
  container.scrollIntoView({ behavior: "smooth", block: "start" });
}

// Budgets

async function renderBudgets(view) {
  await loadAgents();
  const showAll = h("input", { type: "checkbox" });
  const listBody = h("div", {});
  const draw = async () => {
    const data = await hubu("/budgets", showAll.checked ? { all: "true" } : {});
    clear(listBody).append(budgetTable(data.budgets || [], true, draw));
  };
  showAll.addEventListener("change", draw);

  const form = h("form", { class: "grid" },
    h("label", {}, "Agent", h("select", { name: "agent_id", required: true },
      state.agents.filter((a) => a.status === "active").map((a) => h("option", { value: a.agent_id }, a.display_name)))),
    h("label", {}, "Limit", h("input", { name: "amount", required: true, inputmode: "decimal", placeholder: "250.00" })),
    h("label", {}, "Currency", h("select", { name: "currency" }, h("option", { value: "USD" }, "USD"), h("option", { value: "USDC" }, "USDC"))),
    h("label", {}, "Starting at (optional)", h("input", { name: "starting_at", type: "datetime-local" })),
    h("label", {}, "Ending before (optional)", h("input", { name: "ending_before", type: "datetime-local" })),
    h("div", { class: "form-actions" }, h("button", { type: "submit" }, "Create budget")));
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const v = formValues(form);
    try {
      const result = await hubuPost("/budgets", { agent_id: v.agent_id, amount: v.amount, currency: v.currency,
        starting_at: isoFromLocal(v.starting_at), ending_before: isoFromLocal(v.ending_before) });
      toast(`Budget ${result.budget?.budget_id || ""} created.`);
      for (const w of result.spending_target_warnings || []) toast(`Advisory: ${w.message}`, "bad");
      form.reset();
      draw();
    } catch (error) { toast(error.message, "bad"); }
  });
  append(view, [
    card("Budgets", listBody, [h("label", { class: "check" }, showAll, "Include revoked and expired")], true),
    card("Create a budget", state.agents.length ? form : h("p", { class: "hint" }, "Register an agent first.")),
  ]);
  await draw();
}

// Mirrors BudgetAvailability::allows_limit_update in hubu-core: scheduled and
// exhausted budgets can still be raised (or revoked) by the owner.
const EDITABLE_BUDGET_STATES = new Set(["scheduled", "active", "exhausted"]);

function budgetTable(budgets, manage, refresh) {
  return table([
    { label: "Agent", render: (b) => agentName(b.agent_id) },
    { label: "Status", render: (b) => badge(b.status) },
    { label: "Usage", render: (b) => {
      const limit = Number(b.amount_limit.units);
      const used = Number(b.consumed_amount.units) + Number(b.frozen_amount.units);
      const pct = limit > 0 ? Math.min(100, (used / limit) * 100) : 0;
      return h("div", {}, `${money(b.consumed_amount)} spent · ${b.frozen_amount.display} held`,
        h("div", { class: "meter" }, h("span", { class: pct > 90 ? "bad" : pct > 70 ? "warn" : "", style: { width: `${pct.toFixed(1)}%` } })));
    } },
    { label: "Limit", num: true, render: (b) => money(b.amount_limit) },
    { label: "Remaining", num: true, render: (b) => money(b.remaining_amount) },
    { label: "Period", render: (b) => [when(b.starting_at), " → ", b.ending_before ? when(b.ending_before) : "open-ended"] },
    manage ? { label: "", render: (b) => h("div", { class: "row-actions" },
      h("button", { class: "ghost small", type: "button", onclick: () => showBudgetHistory(b) }, "History"),
      EDITABLE_BUDGET_STATES.has(b.status) ? h("button", { class: "ghost small", type: "button", onclick: () => updateBudget(b, refresh) }, "Change limit") : null,
      EDITABLE_BUDGET_STATES.has(b.status) ? h("button", { class: "ghost small", type: "button", onclick: () => revokeBudget(b, refresh) }, "Revoke") : null) } : null,
  ].filter(Boolean), budgets, "No budgets.");
}

function showBudgetHistory(budget) {
  showJsonDialog(`Budget history: ${budget.budget_id}`, async () => {
    const data = await hubu(`/budgets/${encodeURIComponent(budget.budget_id)}/versions`);
    return table([
      { label: "Revision", num: true, render: (v) => v.revision },
      { label: "Limit", num: true, render: (v) => money(v.amount_limit) },
      { label: "Reason", render: (v) => v.reason },
      { label: "Actor", render: (v) => short(v.actor, 18) },
      { label: "Source", render: (v) => v.source },
      { label: "Effective", render: (v) => when(v.effective_at || v.created_at) },
    ], data.versions || [], "No versions.");
  });
}

function updateBudget(budget, refresh) {
  const form = h("form", { class: "grid" },
    h("label", {}, `New limit (${String(budget.currency).toUpperCase()})`, h("input", { name: "amount", required: true, inputmode: "decimal", value: budget.amount_limit.display })),
    h("label", { class: "wide" }, "Reason (optional)", h("input", { name: "reason" })));
  const status = h("div", {});
  const { close } = openDialog("Change budget limit", [
    kv([["Budget", budget.budget_id], ["Current limit", money(budget.amount_limit)], ["Spent", money(budget.consumed_amount)],
      ["Held", money(budget.frozen_amount)], ["Revision", budget.current_revision]]),
    h("p", { class: "hint" }, `The change is pinned to revision ${budget.current_revision}; it is rejected if someone else changed the budget first.`),
    form, status,
  ], (closeFn) => [
    h("button", { type: "button", class: "ghost", onclick: closeFn }, "Cancel"),
    h("button", { type: "button", onclick: () => form.requestSubmit() }, "Save limit"),
  ]);
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const v = formValues(form);
    try {
      await hubuPost(`/budgets/${encodeURIComponent(budget.budget_id)}/versions`,
        { amount_limit: v.amount, expected_revision: budget.current_revision, reason: v.reason || null });
      toast("Budget limit updated.");
      close();
      refresh();
    } catch (error) { clear(status).append(h("div", { class: "notice bad" }, error.message)); }
  });
}

async function revokeBudget(budget, refresh) {
  if (!window.confirm(`Revoke budget ${budget.budget_id}? The agent will no longer be able to spend from it.`)) return;
  try {
    await hubuPost("/budgets/revoke", { budget_id: budget.budget_id });
    toast("Budget revoked.");
    refresh();
  } catch (error) { toast(error.message, "bad"); }
}

// Providers and secret references

// Friendly names for the platforms Hubu ships packs for; others show their id.
const PLATFORM_NAMES = { google: "Google Gemini", flux: "Black Forest Labs (FLUX)" };
const WORKLOAD_NAMES = { image_generation: "Text to image", image_edit: "Image edit" };

function platformName(provider) {
  return PLATFORM_NAMES[provider] || String(provider || "unknown").replace(/[-_]/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
}

function workloadName(workload) {
  return WORKLOAD_NAMES[workload] || String(workload || "").replaceAll("_", " ");
}

/** Group configured revisions and live Gongbu targets by platform, then by model + workload. */
function groupProviders(configs, liveTargets) {
  const platforms = new Map();
  const platform = (id) => {
    if (!platforms.has(id)) platforms.set(id, { id, offerings: new Map(), secrets: new Map(), live: 0 });
    return platforms.get(id);
  };
  const offering = (p, model, workload) => {
    const key = `${model}\u0000${workload}`;
    if (!p.offerings.has(key)) p.offerings.set(key, { model, workload, revisions: [], live: false });
    return p.offerings.get(key);
  };
  for (const c of configs) {
    const p = platform(c.provider);
    offering(p, c.model, c.workload_type).revisions.push(c);
    const ref = c.secret_reference || {};
    const refKey = `${ref.service}\u0000${ref.account}`;
    if (!p.secrets.has(refKey)) p.secrets.set(refKey, { ...ref, models: new Set() });
    p.secrets.get(refKey).models.add(c.model);
  }
  for (const t of liveTargets) {
    const p = platform(t.provider);
    p.live += 1;
    offering(p, t.model, t.workload_type).live = true;
  }
  return [...platforms.values()].sort((a, b) => platformName(a.id).localeCompare(platformName(b.id)));
}

async function renderProviders(view) {
  const data = await api("GET", "/api/providers");
  const stack = data.stack;
  const g = data.gongbu || {};
  const liveTargets = g.configured && !g.error && Array.isArray(g.targets) ? g.targets : [];
  const configs = stack ? stack.provider_configs || [] : [];
  const platforms = groupProviders(configs, liveTargets);

  view.append(h("div", { class: "notice" },
    "Providers and secret references are Gongbu configuration owned by your stack profile (providers.toml and credentials.toml). This view is read-only and never shows secret values; change providers with `hubu stack` and render a new generation."));
  if (data.stack_error) view.append(h("div", { class: "notice bad" }, `Stack profile unavailable: ${data.stack_error}`));
  if (!stack && !data.stack_error) view.append(h("div", { class: "notice warn" }, "No active stack profile was found. Run `hubu stack init` and `hubu stack start` to configure providers."));

  const offerings = platforms.reduce((n, p) => n + p.offerings.size, 0);
  view.append(h("div", { class: "stats" }, [
    stat("Platforms", platforms.length, stack ? `${stack.provider_mode} mode` : "no stack profile"),
    stat("Model offerings", offerings, `${configs.length} configured revisions`),
    stat("Live in Gongbu", liveTargets.length, !g.configured ? "Gongbu not configured" : g.error ? "Gongbu unreachable" : "selectable targets"),
    stat("Secret references", platforms.reduce((n, p) => n + p.secrets.size, 0), "names only, never values"),
  ]));
  if (g.configured && g.error) view.append(h("div", { class: "notice bad" }, `Gongbu at ${g.endpoint}: ${g.error}`));

  for (const p of platforms) {
    const rows = [...p.offerings.values()].sort((a, b) => a.model.localeCompare(b.model) || a.workload.localeCompare(b.workload));
    const secrets = [...p.secrets.values()];
    view.append(h("div", { class: "card platform" },
      h("header", {},
        h("div", { class: "platform-title" }, h("h2", {}, platformName(p.id)), h("span", { class: "mono hint" }, p.id)),
        h("div", { class: "row-actions" },
          badge(`${new Set(rows.map((r) => r.model)).size} models`),
          p.live ? h("span", { class: "badge ok" }, `${p.live} live`) : h("span", { class: "badge" }, "not live"))),
      h("div", { class: "body" }, kv([
        ["Secret reference", secrets.length ? secrets.map((s) => h("div", {},
          h("span", { class: "mono" }, `${s.service} / ${s.account}`),
          h("span", { class: "hint" }, `  used by ${s.models.size} model${s.models.size === 1 ? "" : "s"}`))) : "none configured in this generation"],
      ])),
      h("div", { class: "body flush" }, table([
        { label: "Model", render: (o) => h("span", { class: "mono" }, o.model) },
        { label: "Workload", render: (o) => workloadName(o.workload) },
        { label: "Config version", render: (o) => {
          const sorted = [...o.revisions].sort((a, b) => Number(b.active) - Number(a.active));
          const top = sorted[0];
          if (!top) return h("span", { class: "hint" }, "not in this generation");
          const older = sorted.slice(1);
          return h("div", {}, h("span", { class: "mono" }, top.provider_config_version),
            older.length ? h("details", {}, h("summary", { class: "hint" }, `${older.length} other revision${older.length === 1 ? "" : "s"}`),
              older.map((r) => h("div", { class: "mono hint" }, `${r.provider_config_version}${r.active ? "" : " (inactive)"}`))) : null);
        } },
        { label: "Status", render: (o) => {
          const active = o.revisions.find((r) => r.active);
          if (!o.revisions.length) return null;
          if (!active) return badge("inactive");
          return active.execution_enabled ? badge("enabled") : h("span", { class: "badge warn" }, "execution off");
        } },
        { label: "Live", render: (o) => (o.live ? badge("live") : h("span", { class: "hint" }, "—")) },
        { label: "Pack", render: (o) => {
          const pack = o.revisions.find((r) => r.active)?.pack || o.revisions[0]?.pack;
          return pack ? h("span", { class: "mono hint", title: pack.pack_digest }, `${pack.pack_id}@${pack.pack_version} · ${pack.archetype}`) : null;
        } },
      ], rows))));
  }
  if (!platforms.length && (stack || liveTargets.length === 0)) {
    view.append(card("Platforms", h("div", { class: "empty" }, "No providers are configured in this generation."), null, true));
  }
  if (stack && (stack.contract_bindings || []).length) {
    view.append(h("details", { class: "card details-card" }, h("summary", {}, `Bound provider contracts (${stack.contract_bindings.length})`),
      table([
        { label: "Contract", render: (b) => h("span", { class: "mono" }, b.contract) },
        { label: "Pricing version", render: (b) => b.pricing_version },
      ], stack.contract_bindings)));
  }
  if (stack) {
    view.append(h("p", { class: "hint" }, "Profile ", h("span", { class: "mono" }, stack.profile), " · generation ", short(stack.generation_id, 16)));
  }
}

// ---------- shell ----------

function updateNavCounts() {
  for (const link of document.querySelectorAll("nav a[data-badge]")) {
    const count = state.counts[link.dataset.badge];
    const pill = link.querySelector(".count");
    if (count) {
      if (pill) pill.textContent = String(count);
      else link.append(h("span", { class: "count" }, String(count)));
    } else if (pill) pill.remove();
  }
}

function drawNav(active) {
  const nav = clear(document.getElementById("nav"));
  for (const item of VIEWS) {
    if (item.group) { nav.append(h("div", { class: "nav-group" }, item.group)); continue; }
    nav.append(h("a", { href: `#/${item.id}`, class: item.id === active ? "active" : null,
      "data-badge": item.badge, "aria-current": item.id === active ? "page" : null }, item.label));
  }
  updateNavCounts();
}

let routeToken = 0;

async function route() {
  for (const open of document.querySelectorAll("dialog")) open.close();
  const [id, params] = (location.hash.replace(/^#\/?/, "") || "overview").split("?");
  state.routeParams = new URLSearchParams(params || "");
  const viewDef = VIEWS.find((v) => v.id === id) || VIEWS[1];
  drawNav(viewDef.id);
  document.getElementById("title").textContent = viewDef.label;
  document.title = `${viewDef.label} · Hubu Admin`;
  const token = ++routeToken;
  const view = clear(document.getElementById("view"));
  const loading = h("p", { class: "hint" }, "Loading…");
  view.append(loading);
  const container = h("div", { style: { display: "contents" } });
  try {
    await viewDef.render(container);
    if (token !== routeToken) return;
    clear(view).append(container);
  } catch (error) {
    if (token !== routeToken) return;
    clear(view).append(h("div", { class: "notice bad" }, error.message));
    if (error.status === 401) showSignIn();
  }
}

function showSignIn() {
  document.querySelector(".shell").style.display = "none";
  document.body.append(h("div", { class: "card signin" },
    h("header", {}, h("h2", {}, "Sign in to Hubu Admin")),
    h("div", { class: "body" },
      h("p", {}, "Open the one-time login link that `hubu ui` printed in your terminal."),
      h("p", { class: "hint" }, "Each link works once. If you already used it in another browser, stop `hubu ui` and start it again to get a new link."))));
}

async function refreshApprovalCount() {
  try {
    const summary = await hubu("/governance/summary");
    state.counts.approvals = summary.summary?.backlog?.pending_human_approvals || 0;
    updateNavCounts();
  } catch (_) { /* the badge is a convenience */ }
}

async function boot() {
  try {
    state.session = await api("GET", "/api/session");
  } catch (error) {
    if (error.status === 401) { showSignIn(); return; }
    document.getElementById("view").append(h("div", { class: "notice bad" }, error.message));
    return;
  }
  document.getElementById("endpoint").textContent = state.session.hubu_endpoint || "local";
  hubu("/user").then((me) => {
    state.me = me;
    document.getElementById("whoami").textContent = `Owner: ${me.display_name}`;
  }).catch(() => {});
  document.getElementById("refresh").addEventListener("click", () => { route(); refreshApprovalCount(); });
  window.addEventListener("hashchange", route);
  refreshApprovalCount();
  setInterval(refreshApprovalCount, 30000);
  route();
}

boot();
