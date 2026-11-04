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

const state = { session: null, agents: [], counts: {} };

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
  exceeded: "warn", enabled: "ok", disabled: "", inactive: "",
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
  const data = await hubu("/agents", { all: "true" });
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
  const [summary, budgets, records, agents] = await Promise.all([
    hubu("/governance/summary"),
    hubu("/budgets"),
    hubu("/governance/records", { limit: "12" }),
    loadAgents(),
  ]);
  const s = summary.summary || {};
  const backlog = s.backlog || {};
  const consumed = {};
  const limits = {};
  for (const b of budgets.budgets || []) {
    const cur = String(b.currency).toUpperCase();
    consumed[cur] = (consumed[cur] || 0) + Number(b.consumed_amount.units) / 10 ** b.consumed_amount.scale;
    limits[cur] = (limits[cur] || 0) + Number(b.amount_limit.units) / 10 ** b.amount_limit.scale;
  }
  const consumedText = Object.keys(consumed).length
    ? Object.entries(consumed).map(([c, v]) => `${v.toFixed(2)} ${c}`).join(" · ") : "0.00";
  const limitText = Object.entries(limits).map(([c, v]) => `of ${v.toFixed(2)} ${c}`).join(" · ");
  const denials = (s.automatic_denials || []).reduce((n, d) => n + (d.distinct_attempts || 0), 0);
  const activeAgents = agents.filter((a) => a.status === "active").length;

  append(view, [
    h("div", { class: "stats" }, [
      stat("Spent in active budgets", consumedText, limitText || "no active budgets"),
      stat("Pending approvals", backlog.pending_human_approvals ?? 0, "waiting on a human"),
      stat("Automatic denials", denials, "policy and budget"),
      stat("Unresolved reconciliations", backlog.unresolved_reconciliations ?? 0, "uncertain executor claims"),
      stat("Active agents", activeAgents, `${agents.length} registered`),
      stat("Spend attempts", s.distinct_attempts ?? 0, `${s.distinct_operations ?? 0} operations`),
    ]),
    card("Active budgets", budgetTable(budgets.budgets || [], false), null, true),
    card("Recent decisions", decisionTable(records.records || []),
      [h("a", { href: "#/decisions", class: "hint" }, "All decisions →")], true),
  ]);
}

function stat(label, value, sub) {
  return h("div", { class: "card stat" }, h("div", { class: "label" }, label),
    h("div", { class: "value" }, String(value)), h("div", { class: "sub" }, sub));
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
    h("label", {}, "From", h("input", { type: "datetime-local", name: "from" })),
    h("label", {}, "Before", h("input", { type: "datetime-local", name: "before" })),
    h("button", { type: "submit", class: "ghost" }, "Apply"));
  const query = () => {
    const v = formValues(filters);
    return { agent_id: v.agent_id, categories: v.categories,
      occurred_from: isoFromLocal(v.from), occurred_before: isoFromLocal(v.before) };
  };
  const load = pagedLoader(body, async (cursor) => {
    const data = await hubu("/governance/records", { ...query(), limit: "50", cursor });
    if (!cursor) drawDecisionSummary(summaryBox, data.summary || {});
    return data;
  }, (data) => data.records || [], (rows) => decisionTable(rows));
  filters.addEventListener("submit", (e) => { e.preventDefault(); load(); });
  append(view, [summaryBox, card("Decision records", [h("div", { class: "body" }, filters), body], null, true)]);
  await load();
}

function drawDecisionSummary(container, s) {
  const backlog = s.backlog || {};
  const denials = (s.automatic_denials || []);
  clear(container).append(h("div", { class: "stats" }, [
    stat("Facts", s.fact_count ?? 0, `${s.distinct_attempts ?? 0} attempts`),
    stat("Pending approvals", backlog.pending_human_approvals ?? 0, "backlog now"),
    stat("Automatic denials", denials.reduce((n, d) => n + d.distinct_attempts, 0),
      denials.map((d) => `${d.reason}: ${d.distinct_attempts}`).join(" · ") || "none"),
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
    state.agents = data.agents || state.agents;
    clear(listBody).append(table([
      { label: "Name", render: (a) => a.display_name },
      { label: "Agent ID", render: (a) => short(a.agent_id, 20) },
      { label: "Account", render: (a) => short(a.account_id, 20) },
      { label: "Owner", render: (a) => a.owner_username },
      { label: "Status", render: (a) => badge(a.status) },
      { label: "Registered", render: (a) => when(a.created_at) },
      { label: "", render: (a) => h("div", { class: "row-actions" },
        h("button", { class: "ghost small", type: "button", onclick: () => showAgentHistory(a) }, "History"),
        h("button", { class: "ghost small", type: "button", onclick: () => renameAgent(a, draw) }, "Rename")) },
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
    card("Registered agents", listBody, [h("label", { class: "check" }, showAll, "Include inactive")], true),
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

function renameAgent(agent, done) {
  const form = h("form", { class: "grid" },
    h("label", {}, "New name", h("input", { name: "name", required: true, value: agent.display_name })),
    h("label", { class: "wide" }, "Reason", h("input", { name: "reason", required: true, placeholder: "Why the label changes" })));
  const { close } = openDialog(`Rename ${agent.display_name}`, [
    h("p", { class: "hint" }, "Renaming changes only the label. The agent ID stays the same and the previous fingerprint still resolves to this agent."),
    form,
  ], (closeFn) => [
    h("button", { type: "button", class: "ghost", onclick: closeFn }, "Cancel"),
    h("button", { type: "button", onclick: () => form.requestSubmit() }, "Continue…"),
  ]);
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const values = formValues(form);
    close();
    confirmCapability({
      title: "Confirm rename",
      summary: kv([["Agent", agent.agent_id], ["Name", `${agent.display_name} → ${values.name}`], ["Reason", values.reason]]),
      confirmLabel: "Rename",
      onConfirm: async () => {
        await hubuPost("/agents/rename", { agent_id: agent.agent_id, name: values.name, reason: values.reason }, { confirmed: true });
        toast("Agent renamed.");
        done();
      },
    });
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

async function renderPolicies(view) {
  await loadAgents();
  const data = await hubu("/policies");
  const form = h("form", { class: "grid" },
    h("label", {}, "Attach to", h("select", { name: "agent_id" }, h("option", { value: "" }, "Owner default (all agents)"),
      state.agents.map((a) => h("option", { value: a.agent_id }, a.display_name)))),
    h("label", {}, "Display name (optional)", h("input", { name: "display_name" })),
    h("label", {}, "Declarative key (optional)", h("input", { name: "declarative_key" })),
    h("label", {}, "Expected revision (optional)", h("input", { name: "expected_revision", inputmode: "numeric" })),
    h("label", { class: "wide" }, "Policy YAML", h("textarea", { name: "policy_yaml", required: true, spellcheck: "false", placeholder: "id: my_policy\nversion: v1\ndefault_effect: needs_approval\nrules: []" })),
    h("div", { class: "form-actions" }, h("button", { type: "submit" }, "Apply policy")));
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const v = formValues(form);
    const body = { policy_yaml: form.elements.policy_yaml.value, source: "web_ui" };
    for (const key of ["agent_id", "display_name", "declarative_key"]) if (v[key]) body[key] = v[key];
    if (v.expected_revision) body.expected_revision = Number(v.expected_revision);
    try {
      const result = await hubuPost("/policies", body);
      toast(result.changed === false ? "Policy unchanged (same content)." : `Policy applied (revision ${result.revision ?? "?"}).`);
      route();
    } catch (error) { toast(error.message, "bad"); }
  });

  append(view, [
    card("Policies", table([
      { label: "Name", render: (p) => p.display_name },
      { label: "Scope", render: (p) => (p.agent_id ? agentName(p.agent_id) : badge("user_default")) },
      { label: "Version", render: (p) => p.policy_version },
      { label: "Default", render: (p) => badge(p.default_decision) },
      { label: "Rules", num: true, render: (p) => p.rules },
      { label: "Revision", num: true, render: (p) => p.revision },
      { label: "Updated", render: (p) => when(p.updated_at) },
      { label: "", render: (p) => h("div", { class: "row-actions" },
        h("button", { class: "ghost small", type: "button", onclick: () => showJsonDialog(`${p.display_name} YAML`, async () => {
          const exported = await hubu("/policies/export", { policy_id: p.policy_id });
          return h("div", {}, h("pre", { class: "json" }, exported.policy_yaml || ""),
            h("div", { class: "form-actions" }, h("button", { type: "button", class: "ghost small", onclick: () => {
              // Load every scope field from this policy so a previous
              // selection can never retarget the revision to another agent.
              form.elements.policy_yaml.value = exported.policy_yaml || "";
              form.elements.agent_id.value = p.agent_id || "";
              form.elements.display_name.value = p.display_name || "";
              form.elements.declarative_key.value = p.declarative_key || "";
              form.elements.expected_revision.value = String(p.revision);
              document.querySelector("dialog")?.close();
              form.scrollIntoView({ behavior: "smooth" });
            } }, "Edit as new revision")));
        }) }, "YAML"),
        h("button", { class: "ghost small", type: "button", onclick: () => showPolicyHistory(p) }, "History")) },
    ], data.policies, "No policies yet. The server default applies until you add one."), null, true),
    card("Apply a policy", [h("p", { class: "hint" }, "Policies are validated by the server. Applying identical content is a no-op; set an expected revision to guard against concurrent edits."), h("br"), form]),
  ]);
}

function showPolicyHistory(policy) {
  showJsonDialog(`History: ${policy.display_name}`, async () => {
    const data = await hubu("/policies/history", { policy_id: policy.policy_id });
    const revisions = data.revisions || [];
    const diffBox = h("div", {});
    const from = h("input", { inputmode: "numeric", value: String(Math.max(1, policy.revision - 1)), size: 4 });
    const to = h("input", { inputmode: "numeric", value: String(policy.revision), size: 4 });
    const diff = h("button", { class: "ghost small", type: "button", onclick: async () => {
      try {
        const d = await hubu("/policies/diff", { policy_id: policy.policy_id, from_revision: from.value, to_revision: to.value });
        clear(diffBox).append(h("p", { class: "hint" }, `Changed paths: ${(d.changed_paths || []).join(", ") || "none"}`), details("Full diff", d));
      } catch (e) { clear(diffBox).append(h("div", { class: "notice bad" }, e.message)); }
    } }, "Diff");
    return h("div", {},
      table([
        { label: "Revision", render: (r) => r.revision },
        { label: "Hash", render: (r) => short(r.payload_hash, 22) },
        { label: "Actor", render: (r) => short(r.actor, 18) },
        { label: "Created", render: (r) => when(r.created_at) },
      ], revisions.map((r, i) => ({ revision: i + 1, ...r })), "No revisions."),
      h("br"),
      h("div", { class: "filters" }, h("label", {}, "From revision", from), h("label", {}, "To revision", to), diff),
      diffBox,
      h("br"),
      h("h4", {}, "Audit trail"),
      table([
        { label: "Action", render: (a) => badge(a.action) },
        { label: "Source", render: (a) => a.source },
        { label: "Assignments", render: (a) => (a.affected_assignments || []).join(", ") },
        { label: "When", render: (a) => when(a.occurred_at) },
      ], data.audit || [], "No audit entries."));
  });
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
      b.status === "active" ? h("button", { class: "ghost small", type: "button", onclick: () => updateBudget(b, refresh) }, "Change limit") : null,
      b.status === "active" ? h("button", { class: "ghost small", type: "button", onclick: () => revokeBudget(b, refresh) }, "Revoke") : null) } : null,
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

async function renderProviders(view) {
  const data = await api("GET", "/api/providers");
  const stack = data.stack;
  view.append(h("div", { class: "notice" },
    "Provider revisions and secret references are Gongbu configuration, owned by your stack profile (providers.toml and credentials.toml). This view is read-only and never shows secret values. Change them with `hubu stack` and render a new generation."));
  if (data.stack_error) view.append(h("div", { class: "notice bad" }, `Stack profile unavailable: ${data.stack_error}`));
  if (stack) {
    const configs = stack.provider_configs || [];
    view.append(card(`Configured provider revisions — ${stack.provider_mode} mode`, [
      h("div", { class: "body" }, kv([["Profile", h("span", { class: "mono" }, stack.profile)], ["Generation", short(stack.generation_id, 16)]])),
      table([
        { label: "Provider", render: (c) => c.provider },
        { label: "Model", render: (c) => c.model },
        { label: "Workload", render: (c) => c.workload_type },
        { label: "Config version", render: (c) => h("span", { class: "mono" }, c.provider_config_version) },
        { label: "Selectable", render: (c) => badge(c.active ? "active" : "inactive") },
        { label: "Execution", render: (c) => badge(c.execution_enabled ? "enabled" : "disabled") },
        { label: "Pack", render: (c) => (c.pack ? `${c.pack.pack_id}@${c.pack.pack_version}` : c.settings_type) },
      ], configs, "No provider revisions in this generation."),
    ], null, true));
    const refs = new Map();
    for (const c of configs) {
      const key = `${c.secret_reference.service}\u0000${c.secret_reference.account}`;
      if (!refs.has(key)) refs.set(key, { ...c.secret_reference, used_by: [] });
      refs.get(key).used_by.push(`${c.provider}/${c.model}`);
    }
    view.append(card("Secret references", table([
      { label: "Service", render: (r) => h("span", { class: "mono" }, r.service) },
      { label: "Account", render: (r) => h("span", { class: "mono" }, r.account) },
      { label: "Used by", render: (r) => r.used_by.join(", ") },
    ], [...refs.values()], "No secret references."), null, true));
    if ((stack.contract_bindings || []).length) {
      view.append(card("Bound provider contracts", table([
        { label: "Contract", render: (b) => h("span", { class: "mono" }, b.contract) },
        { label: "Pricing version", render: (b) => b.pricing_version },
      ], stack.contract_bindings), null, true));
    }
  } else if (!data.stack_error) {
    view.append(h("div", { class: "notice warn" }, "No active stack profile was found. Run `hubu stack init` and `hubu stack start` to manage providers."));
  }
  const g = data.gongbu || {};
  let gongbuBody;
  if (!g.configured) gongbuBody = h("p", { class: "hint" }, "Gongbu is not configured for this connection. Pass --gongbu-endpoint and --gongbu-token-file to `hubu ui`, or use a stack profile with Gongbu.");
  else if (g.error) gongbuBody = h("div", { class: "notice bad" }, `Gongbu at ${g.endpoint}: ${g.error}`);
  else gongbuBody = table([
    { label: "Target ID", render: (t) => short(t.target_id, 26) },
    { label: "Provider", render: (t) => t.provider },
    { label: "Model", render: (t) => t.model },
    { label: "Workload", render: (t) => t.workload_type },
    { label: "Merchant", render: (t) => t.execution_scope?.billing_merchant },
    { label: "", render: (t) => h("button", { class: "ghost small", type: "button", onclick: () => openDialog("Execution target", jsonBlock(t)) }, "Details") },
  ], Array.isArray(g.targets) ? g.targets : [], "Gongbu reports no execution targets.");
  view.append(card("Live Gongbu execution targets", gongbuBody, null, !!(g.configured && !g.error)));
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
  const id = (location.hash.replace(/^#\/?/, "") || "overview").split("?")[0];
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
    document.getElementById("whoami").textContent = `Owner: ${me.display_name}`;
  }).catch(() => {});
  document.getElementById("refresh").addEventListener("click", () => { route(); refreshApprovalCount(); });
  window.addEventListener("hashchange", route);
  refreshApprovalCount();
  setInterval(refreshApprovalCount, 30000);
  route();
}

boot();
