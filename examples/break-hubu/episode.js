"use strict";

/* Kept independent of the DOM so the same evidence checks can run in Node. */
function validateEvidence(data) {
  const checks = [];
  const require = (condition, message) => {
    if (!condition) throw new Error(message);
  };
  const object = (value) => value !== null && typeof value === "object" && !Array.isArray(value);
  const safeUnits = (value) => Number.isSafeInteger(value) && value >= 0;
  const nonempty = (value) => typeof value === "string" && value.length > 0 && value.length <= 500;
  require(object(data) && data.schema_version === 1, "Expected Episode 01 evidence with schema_version 1.");
  const scenario = data.scenario;
  require(object(scenario), "The scenario is missing.");
  require(scenario.workers === 100 && scenario.budget_units === 1000000 && scenario.request_units === 30000 && scenario.currency === "USD" && scenario.scale === 6, "This replay requires 100 requests of $0.03 against a $1 USD budget at scale 6.");
  require(scenario.registered_accounts === 1 && scenario.provider_mode === "synthetic_executor_settlement", "Expected one registered account and synthetic executor settlement.");
  for (const field of ["agent_id", "account_id", "budget_id"]) require(nonempty(scenario[field]), `The scenario ${field} is missing.`);
  require(nonempty(scenario.scope), "The scenario scope is missing.");
  checks.push("Episode scenario and one registered account");

  require(Array.isArray(data.requests) && data.requests.length === 100, "Expected exactly 100 recorded decisions.");
  const workers = new Set();
  const operations = new Set();
  const decisions = new Set();
  const allowed = [];
  const denied = [];
  for (const request of data.requests) {
    require(object(request), "A recorded request is malformed.");
    require(Number.isSafeInteger(request.worker) && request.worker >= 0 && request.worker < 100 && !workers.has(request.worker), "Worker IDs must be unique integers from 0 through 99.");
    require(nonempty(request.operation_key) && !operations.has(request.operation_key), "Every request needs a distinct operation key.");
    require(nonempty(request.decision_id) && !decisions.has(request.decision_id), "Every request needs a distinct decision ID.");
    require(request.decision === "allow" || request.decision === "deny", "Every request must record allow or deny.");
    require(request.amount_units === 30000, "A request amount differs from the episode's $0.03 scenario.");
    require(Array.isArray(request.reason) && request.reason.length <= 20 && request.reason.every((reason) => typeof reason === "string" && reason.length <= 1000), "Decision reasons must be an array of text.");
    require(request.decision !== "deny" || request.reason.includes("budget does not have enough remaining balance"), "A denied request does not record an insufficient budget reason.");
    workers.add(request.worker);
    operations.add(request.operation_key);
    decisions.add(request.decision_id);
    (request.decision === "allow" ? allowed : denied).push(request);
  }
  require(allowed.length === 33 && denied.length === 67, "This recording does not contain the expected 33 allowed and 67 denied decisions.");
  checks.push("100 distinct workers, operations, and decisions");

  const summary = data.summary;
  require(object(summary), "The run summary is missing.");
  require(summary.allowed === allowed.length && summary.denied === denied.length && summary.errors === 0 && summary.settled === allowed.length, "Summary counts do not reconcile with the recorded decisions.");
  require(summary.authorized_units === 990000 && summary.settled_units === 990000 && summary.released_units === 0, "Authorized, settled, or released units do not reconcile.");
  require(object(data.snapshots), "Recorded budget snapshots are missing.");
  const expected = {
    initial: { available_units: 1000000, reserved_units: 0, consumed_units: 0 },
    reserved: { available_units: 10000, reserved_units: 990000, consumed_units: 0 },
    final: { available_units: 10000, reserved_units: 0, consumed_units: 990000 }
  };
  for (const [name, values] of Object.entries(expected)) {
    const snapshot = data.snapshots[name];
    require(object(snapshot), `The ${name} budget snapshot is missing.`);
    for (const [field, value] of Object.entries(values)) require(safeUnits(snapshot[field]) && snapshot[field] === value, `The ${name} snapshot has inconsistent ${field}.`);
    require(snapshot.available_units + snapshot.reserved_units + snapshot.consumed_units === scenario.budget_units, `The ${name} snapshot does not conserve the budget.`);
  }
  checks.push("Summary and all three budget snapshots reconcile");

  const ledger = data.ledger;
  require(object(ledger) && Array.isArray(ledger.transactions) && ledger.transactions.length === 33 && ledger.next_cursor === null, "Expected the complete ledger of 33 settlements.");
  const coverage = ledger.coverage;
  require(object(coverage), "Ledger coverage is missing.");
  const amount = (value, units) => object(value) && value.units === String(units) && value.scale === 6 && value.currency === "usd";
  require(amount(coverage.consumed_amount, 990000) && amount(coverage.recorded_budget_charges, 990000) && amount(coverage.unaccounted_consumption, 0) && coverage.pending_hold_count === 0, "Ledger coverage does not reconcile to $0.99 settled with no pending holds.");
  require(coverage.agent_id === scenario.agent_id && coverage.budget_id === scenario.budget_id, "Ledger coverage belongs to a different agent or budget.");
  const authorizations = new Set(allowed.map((request) => request.decision_id));
  const transactions = new Set();
  const settlements = new Set();
  const entryAmount = (entry) => {
    require(object(entry) && object(entry.amount), "A ledger entry is malformed.");
    const value = entry.amount;
    require(typeof value.amount === "string" && /^\d{1,40}$/.test(value.amount) && Number.isInteger(value.scale) && value.scale >= 0 && value.scale <= 18 && value.currency === "usd", "A ledger entry amount is malformed.");
    return BigInt(value.amount) * (10n ** BigInt(18 - value.scale));
  };
  for (const transaction of ledger.transactions) {
    require(object(transaction) && nonempty(transaction.id) && !transactions.has(transaction.id), "Every ledger transaction needs a distinct ID.");
    require(nonempty(transaction.settlement_id) && !settlements.has(transaction.settlement_id), "Every ledger settlement needs a distinct ID.");
    require(authorizations.has(transaction.authorization_id), "A settlement is duplicated or does not match an allowed decision.");
    require(transaction.agent_id === scenario.agent_id && transaction.account_id === scenario.account_id && transaction.budget_id === scenario.budget_id, "A settlement belongs to a different agent, account, or budget.");
    require(object(transaction.budget_charge_delta) && transaction.budget_charge_delta.units === "30000" && transaction.budget_charge_delta.scale === 6, "A settlement's budget charge is not $0.03.");
    require(object(transaction.effective_cost) && transaction.effective_cost.amount === "3" && transaction.effective_cost.scale === 2 && transaction.effective_cost.currency === "usd", "A settlement's effective cost is not $0.03 USD.");
    require(Array.isArray(transaction.entries) && transaction.entries.length === 2, "A settlement must have two balanced ledger entries.");
    const [first, second] = transaction.entries;
    const firstAmount = entryAmount(first);
    const secondAmount = entryAmount(second);
    require(firstAmount === 30000000000000000n && secondAmount === firstAmount && [first.direction, second.direction].sort().join(",") === "credit,debit", "A settlement's ledger entries do not balance to $0.03.");
    require(transaction.entries.some((entry) => entry.account_kind === "AgentSpendExpense" && entry.direction === "debit") && transaction.entries.some((entry) => entry.account_kind === "ExternallyBilledClearing" && entry.direction === "credit"), "A settlement does not debit agent expense and credit externally billed clearing.");
    transactions.add(transaction.id);
    settlements.add(transaction.settlement_id);
    authorizations.delete(transaction.authorization_id);
  }
  require(authorizations.size === 0, "An allowed authorization has no ledger settlement.");
  checks.push("All 33 allowed decisions match balanced $0.03 ledger settlements");

  require(Array.isArray(data.assertions) && data.assertions.length > 0 && data.assertions.length <= 50, "Recorded test assertions are missing.");
  require(data.assertions.every((assertion) => object(assertion) && nonempty(assertion.name) && assertion.passed === true), "A recorded test assertion failed or is malformed; passing playback is unavailable.");
  checks.push(`${data.assertions.length} recorded test assertions passed`);
  if (data.provenance !== undefined) {
    require(object(data.provenance), "Run provenance is malformed.");
    const provenance = data.provenance;
    require(typeof provenance.commit === "string" && /^[a-f0-9]{40}$/.test(provenance.commit) && typeof provenance.dirty === "boolean", "Run provenance needs a full commit SHA and a working-tree status.");
    require(provenance.repository_url === "https://github.com/hacker-no-ice/hubu" && provenance.test_file === "crates/hubu-api/src/tests/break_hubu.rs" && nonempty(provenance.test_command), "Run provenance must identify the Episode 01 source test and command.");
    require(provenance.elapsed_ms === undefined || (Number.isFinite(provenance.elapsed_ms) && provenance.elapsed_ms >= 0), "Captured elapsed time is malformed.");
  }
  return { run: data, checks, allowed: allowed.length, denied: denied.length };
}

if (typeof module !== "undefined" && module.exports) module.exports = { validateEvidence };

if (typeof document !== "undefined") {
  (() => {
    const $ = (id) => document.getElementById(id);
    const duration = 45;
    const money = (units) => new Intl.NumberFormat("en-US", { style: "currency", currency: "USD", minimumFractionDigits: 2, maximumFractionDigits: 2 }).format(units / 1000000);
    const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
    const phases = [
      { start: 0, number: "01", kicker: "SET THE LIMIT", title: "One agent.\nOne dollar.", description: "One registered agent account starts with $1 available. Each of its 100 concurrent requests asks to authorize $0.03." },
      { start: 6, number: "02", kicker: "RELEASE THE BURST", title: "100 requests.\nAll at once.", description: "The local test releases 100 worker tasks through the API handlers. Together, they request $3 of authorization from a $1 budget." },
      { start: 14, number: "03", kicker: "CHECK EVERY REQUEST", title: "33 get through.\n67 hit the limit.", description: "Recorded decisions allow 33 requests and deny 67. The captured hold snapshot has $0.99 reserved, with $0.01 still available." },
      { start: 28, number: "04", kicker: "FOLLOW THE MONEY", title: "Every allowed request\ngets a receipt.", description: "The test synthetically settles all 33 authorizations. Each has a balanced $0.03 ledger transaction. No external provider is called." },
      { start: 38, number: "05", kicker: "THE RECEIPT", title: "$0.99 settled.\n$0.01 left.", description: "The final snapshot reconciles to the $1 limit. No holds remain, and the recorded assertions include reopening SQLite to check persistence." }
    ];
    const dots = [];
    for (let index = 0; index < 100; index += 1) {
      const dot = document.createElement("span");
      dot.className = "worker";
      dot.setAttribute("aria-hidden", "true");
      $("worker-grid").append(dot);
      dots.push(dot);
    }
    let evidence = null;
    let sourceText = "";
    let position = 0;
    let playing = false;
    let frame = 0;
    let lastFrame = 0;
    let phaseIndex = -1;
    let loadVersion = 0;
    let focused = false;
    let savedScroll = 0;
    const put = (id, value) => { $(id).textContent = value; };
    function setFocused(value, restoreScroll = true) {
      if (value === focused) return;
      if (value) savedScroll = window.scrollY;
      focused = value;
      document.body.classList.toggle("is-focused", focused);
      $("focus-button").setAttribute("aria-pressed", String(focused));
      $("focus-button").setAttribute("aria-label", focused ? "Exit focus view" : "Enter focus view");
      $("focus-button").replaceChildren(document.createTextNode(focused ? "Exit focus " : "Focus view "), Object.assign(document.createElement("span"), { textContent: focused ? "↙" : "⤢", ariaHidden: "true" }));
      put("stage-title", focused ? "BREAK HUBU / EPISODE 01" : "The concurrency test");
      if (focused || restoreScroll) window.scrollTo({ top: focused ? 0 : savedScroll, behavior: "instant" });
    }
    const withSmall = (id, value, suffix) => {
      const small = document.createElement("small");
      small.textContent = suffix;
      $(id).replaceChildren(document.createTextNode(value), small);
    };
    function setPlaying(value) {
      playing = Boolean(value && evidence);
      put("play-label", playing ? "Pause" : position >= duration ? "Replay" : "Play replay");
      put("play-icon", playing ? "Ⅱ" : "▶");
      $("play-button").setAttribute("aria-label", playing ? "Pause replay" : position >= duration ? "Replay from beginning" : "Play replay");
      if (playing) {
        lastFrame = performance.now();
        cancelAnimationFrame(frame);
        frame = requestAnimationFrame(tick);
      } else cancelAnimationFrame(frame);
    }
    function render() {
      $("timeline").value = String(position);
      $("timeline").setAttribute("aria-valuetext", `${Math.floor(position)} seconds of 45 seconds`);
      $("timeline").style.background = `linear-gradient(to right, var(--orange) ${position / duration * 100}%, var(--line) ${position / duration * 100}%)`;
      put("playback-time", `00:${String(Math.floor(position)).padStart(2, "0")}`);
      if (!evidence) return;
      const run = evidence.run;
      const nextPhase = phases.findLastIndex((phase) => position >= phase.start);
      if (nextPhase !== phaseIndex) {
        phaseIndex = nextPhase;
        const phase = phases[phaseIndex];
        put("chapter-number", phase.number);
        put("phase-kicker", phase.kicker);
        put("phase-title", phase.title);
        $("phase-title").style.whiteSpace = "pre-line";
        put("phase-description", phase.description);
        put("phase-announcement", `${phase.kicker}. ${phase.title.replace("\n", " ")}`);
      }
      const requested = position < 6 ? 0 : Math.min(100, Math.floor((position - 6) / 8 * 100));
      const revealed = position < 14 ? 0 : Math.min(100, Math.floor((position - 14) / 14 * 100));
      let allowCount = 0;
      let denyCount = 0;
      run.requests.forEach((request, index) => {
        const shown = index < revealed;
        const dot = dots[request.worker];
        dot.className = `worker${shown ? ` ${request.decision === "allow" ? "allowed" : "denied"}` : ""}${!reducedMotion.matches && position >= 6 && position < 14 && index < requested ? " active" : ""}`;
        dot.title = `Worker ${String(request.worker).padStart(3, "0")}${shown ? ` · ${request.decision === "allow" ? "allowed" : "denied"}` : " · awaiting replay reveal"}`;
        if (shown && request.decision === "allow") allowCount += 1;
        if (shown && request.decision === "deny") denyCount += 1;
      });
      put("grid-progress", position < 6 ? "Ready · one agent account" : position < 14 ? `${requested} / 100 requests in replay` : `${revealed} / 100 decisions revealed`);
      $("worker-grid").setAttribute("aria-label", `Illustrative reveal of recorded results: ${allowCount} allowed, ${denyCount} denied, ${100 - revealed} not yet revealed.`);
      withSmall("requested-total", money(requested * run.scenario.request_units), " / $3.00");
      withSmall("allowed-total", String(allowCount), " requests");
      withSmall("denied-total", String(denyCount), " requests");
      const snapshotKey = position < 14 ? "initial" : position < 34 ? "reserved" : "final";
      const snapshot = run.snapshots[snapshotKey];
      put("budget-caption", `CAPTURED · ${snapshotKey === "initial" ? "INITIAL BUDGET" : snapshotKey === "reserved" ? "ALL HOLDS" : "FINAL BUDGET"}`);
      $("budget-unloaded").hidden = true;
      $("budget-consumed").style.width = `${snapshot.consumed_units / 10000}%`;
      $("budget-reserved").style.width = `${snapshot.reserved_units / 10000}%`;
      put("settled-value", money(snapshot.consumed_units));
      put("reserved-value", money(snapshot.reserved_units));
      put("available-value", money(snapshot.available_units));
      $("budget-track").setAttribute("aria-label", `Recorded ${snapshotKey} snapshot: ${money(snapshot.consumed_units)} settled, ${money(snapshot.reserved_units)} reserved, ${money(snapshot.available_units)} available.`);
      put("overspend-total", position >= 38 ? money(Math.max(0, snapshot.consumed_units - run.scenario.budget_units)) : "—");
      put("result-title", position >= 38 ? "The recorded budget held." : "Evidence ready to replay");
      put("result-description", position >= 38 ? "$0.99 settled · $0.01 available · $0.00 released" : `${run.assertions.length} recorded assertions · 100 decisions · 33 ledger settlements`);
    }
    function tick(now) {
      if (!playing) return;
      position = Math.min(duration, position + (now - lastFrame) / 1000);
      lastFrame = now;
      render();
      if (position >= duration) setPlaying(false);
      else frame = requestAnimationFrame(tick);
    }
    function reset() {
      setPlaying(false);
      evidence = null;
      sourceText = "";
      position = 0;
      phaseIndex = -1;
      setPlaying(false);
      for (const id of ["play-button", "restart-button", "timeline", "view-evidence", "focus-evidence", "download-evidence"]) $(id).disabled = true;
      $("evidence-status").classList.remove("loaded");
      $("evidence-status").replaceChildren(Object.assign(document.createElement("span"), { ariaHidden: "true" }), document.createTextNode("Evidence not loaded"));
      $("empty-state").hidden = false;
      $("result-note").hidden = true;
      $("budget-unloaded").hidden = false;
      $("budget-consumed").style.width = "0";
      $("budget-reserved").style.width = "0";
      $("worker-grid").setAttribute("aria-label", "100 workers, recorded decisions not loaded");
      $("budget-track").setAttribute("aria-label", "Budget balances not loaded");
      dots.forEach((dot) => { dot.className = "worker"; dot.removeAttribute("title"); });
      put("chapter-number", "00");
      put("phase-kicker", "READY WHEN THE EVIDENCE IS");
      put("phase-title", "A small limit.\nA crowded starting line.");
      $("phase-title").style.whiteSpace = "pre-line";
      put("phase-description", "Load a recorded test run to replay its decisions. Every dot represents a worker in that run.");
      put("grid-progress", "Awaiting evidence");
      put("budget-caption", "ACCOUNT BUDGET");
      for (const id of ["settled-value", "reserved-value", "available-value", "overspend-total"]) put(id, "—");
      withSmall("requested-total", "—", " / $3.00");
      withSmall("allowed-total", "—", " requests");
      withSmall("denied-total", "—", " requests");
      for (const id of ["receipt-commit", "receipt-elapsed", "receipt-command"]) put(id, "—");
      put("receipt-file", "Not loaded");
      $("source-link").hidden = true;
      $("source-link").removeAttribute("href");
      const noAssertions = document.createElement("li");
      noAssertions.className = "empty-assertion";
      noAssertions.textContent = "No assertion evidence loaded.";
      $("assertion-list").replaceChildren(noAssertions);
      const row = document.createElement("tr");
      const cell = document.createElement("td");
      cell.colSpan = 5;
      cell.className = "empty-table";
      cell.textContent = "Load a run to inspect all 100 decisions.";
      row.append(cell);
      $("decision-table").replaceChildren(row);
      put("receipt-checks", "A loaded file is not independently authenticated by this viewer.");
      put("ledger-count", "No transactions loaded");
      for (const id of ["ledger-charges", "ledger-consumed", "ledger-unaccounted", "ledger-pending"]) put(id, "—");
      const ledgerRow = document.createElement("tr");
      const ledgerCell = document.createElement("td");
      ledgerCell.colSpan = 5;
      ledgerCell.className = "empty-table";
      ledgerCell.textContent = "Load a run to inspect its ledger.";
      ledgerRow.append(ledgerCell);
      $("ledger-table").replaceChildren(ledgerRow);
      render();
    }
    function populateReceipt(name) {
      const run = evidence.run;
      const provenance = run.provenance;
      put("receipt-file", name);
      if (provenance) {
        put("receipt-commit", `${provenance.commit}${provenance.dirty ? " · working tree run" : " · clean tree"}`);
        put("receipt-command", provenance.test_command);
        put("receipt-elapsed", provenance.elapsed_ms === undefined ? "Not recorded" : `${provenance.elapsed_ms.toLocaleString("en-US", { maximumFractionDigits: 1 })} ms · full harness elapsed time`);
        if (!provenance.dirty) {
          $("source-link").href = `${provenance.repository_url}/blob/${provenance.commit}/${provenance.test_file.split("/").map(encodeURIComponent).join("/")}`;
          $("source-link").hidden = false;
        }
      } else {
        put("receipt-commit", "Source commit not recorded");
        put("receipt-elapsed", "Not recorded");
        put("receipt-command", "Not recorded");
      }
      const assertions = run.assertions.map((assertion) => {
        const item = document.createElement("li");
        const mark = document.createElement("span");
        mark.className = "assertion-mark";
        mark.textContent = "✓";
        mark.setAttribute("aria-label", "Passed:");
        item.append(mark, document.createTextNode(assertion.name));
        return item;
      });
      $("assertion-list").replaceChildren(...assertions);
      const rows = [...run.requests].sort((a, b) => a.worker - b.worker).map((request) => {
        const row = document.createElement("tr");
        const values = [String(request.worker).padStart(3, "0"), request.decision === "allow" ? "Allowed" : "Denied", money(request.amount_units), request.decision_id, request.reason.join("; ") || "—"];
        values.forEach((value, index) => {
          const cell = document.createElement("td");
          cell.textContent = value;
          if (index === 1) cell.className = `decision-${request.decision}`;
          row.append(cell);
        });
        return row;
      });
      $("decision-table").replaceChildren(...rows);
      put("ledger-count", `${run.ledger.transactions.length} canonical transactions`);
      put("ledger-charges", money(Number(run.ledger.coverage.recorded_budget_charges.units)));
      put("ledger-consumed", money(Number(run.ledger.coverage.consumed_amount.units)));
      put("ledger-unaccounted", money(Number(run.ledger.coverage.unaccounted_consumption.units)));
      put("ledger-pending", String(run.ledger.coverage.pending_hold_count));
      const ledgerRows = run.ledger.transactions.map((transaction) => {
        const row = document.createElement("tr");
        const values = [transaction.id, transaction.authorization_id, money(Number(transaction.budget_charge_delta.units)), money(Number(transaction.budget_charge_delta.units)), money(Number(transaction.budget_charge_delta.units))];
        values.forEach((value) => {
          const cell = document.createElement("td");
          cell.textContent = value;
          row.append(cell);
        });
        return row;
      });
      $("ledger-table").replaceChildren(...ledgerRows);
      put("receipt-checks", `${evidence.checks.length} viewer checks passed: ${evidence.checks.join("; ")}. Source authenticity is not independently verified by this viewer.`);
    }
    function loadText(raw, name) {
      reset();
      try {
        if (raw.length > 3000000) throw new Error("The evidence file exceeds the 3 MB limit.");
        evidence = validateEvidence(JSON.parse(raw));
        sourceText = raw;
        for (const id of ["play-button", "restart-button", "timeline", "view-evidence", "focus-evidence", "download-evidence"]) $(id).disabled = false;
        $("evidence-status").classList.add("loaded");
        $("evidence-status").replaceChildren(Object.assign(document.createElement("span"), { ariaHidden: "true" }), document.createTextNode("Recorded evidence loaded"));
        $("empty-state").hidden = true;
        $("result-note").hidden = false;
        $("load-message").classList.remove("error");
        put("load-message", `${name} loaded. Decision reveal order is editorial; use the receipt to inspect the recorded results.`);
        populateReceipt(name);
        render();
      } catch (error) {
        reset();
        $("load-message").classList.add("error");
        put("load-message", `Evidence not loaded: ${error instanceof Error ? error.message : "The file could not be read."}`);
      }
    }
    $("play-button").addEventListener("click", () => {
      if (position >= duration) { position = 0; phaseIndex = -1; render(); }
      setPlaying(!playing);
    });
    $("restart-button").addEventListener("click", () => { position = 0; phaseIndex = -1; render(); setPlaying(false); });
    $("timeline").addEventListener("input", (event) => { position = Number(event.target.value); render(); if (position >= duration) setPlaying(false); });
    document.addEventListener("visibilitychange", () => { if (document.hidden) setPlaying(false); });
    function inspectEvidence() {
      setFocused(false, false);
      $("evidence-drawer").open = true;
      $("evidence-drawer").scrollIntoView({ behavior: reducedMotion.matches ? "instant" : "smooth", block: "start" });
      $("evidence-drawer").querySelector("summary").focus({ preventScroll: true });
    }
    $("view-evidence").addEventListener("click", inspectEvidence);
    $("focus-evidence").addEventListener("click", inspectEvidence);
    $("focus-button").addEventListener("click", () => setFocused(!focused));
    document.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && focused) {
        setFocused(false);
        $("focus-button").focus({ preventScroll: true });
      }
    });
    $("download-evidence").addEventListener("click", () => {
      if (!evidence) return;
      const url = URL.createObjectURL(new Blob([sourceText], { type: "application/json" }));
      const link = document.createElement("a");
      link.href = url;
      link.download = "episode-01-evidence.json";
      link.click();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    });
    $("evidence-file").addEventListener("change", async (event) => {
      const file = event.target.files[0];
      if (!file) return;
      loadVersion += 1;
      const version = loadVersion;
      if (file.size > 3000000) { reset(); $("load-message").classList.add("error"); put("load-message", "Evidence not loaded: the evidence file exceeds the 3 MB limit."); return; }
      try {
        const raw = await file.text();
        if (version === loadVersion) loadText(raw, file.name);
      } catch {
        if (version === loadVersion) { reset(); $("load-message").classList.add("error"); put("load-message", "Evidence not loaded: the selected file could not be read."); }
      }
      event.target.value = "";
    });
    /* A file opened directly remains usable via the picker without CORS setup. */
    if (location.protocol === "http:" || location.protocol === "https:") {
      const version = loadVersion;
      fetch("evidence/run.json", { cache: "no-store" }).then(async (response) => {
        if (!response.ok || version !== loadVersion) return;
        const contentLength = Number(response.headers.get("content-length"));
        if (contentLength > 3000000) throw new Error("Evidence exceeds 3 MB.");
        const raw = await response.text();
        if (version === loadVersion) loadText(raw, "evidence/run.json");
      }).catch(() => { /* Missing optional evidence leaves the honest preview state intact. */ });
    }
  })();
}
