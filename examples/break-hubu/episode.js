"use strict";

/* The validator and digest checks live in evidence.js, shared with the capture tests. */
if (typeof document !== "undefined") {
  (() => {
    const $ = (id) => document.getElementById(id);
    const duration = 75;
    const maxFileSize = 3000000;
    const money = (units) => new Intl.NumberFormat("en-US", { style: "currency", currency: "USD", maximumFractionDigits: 2 }).format(units / 1000000);
    const json = (value) => JSON.stringify(value, null, 2);
    const elapsed = (value) => `${value.toLocaleString("en-US", { maximumFractionDigits: 1 })} ms`;
    const clock = (value) => `${String(Math.floor(value / 60)).padStart(2, "0")}:${String(Math.floor(value % 60)).padStart(2, "0")}`;
    const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
    const chapters = [
      { start: 0, label: "01 / THE SETUP", title: "Here is what we tested.", description: "The test creates one agent, a $1 budget, and an allow policy in an isolated SQLite database. These are the IDs and policy captured from that setup.", excerpt: "setup", observation: "setup", caption: "Setup calls real Hubu registration, policy, and storage functions. Authentication is a test fixture." },
      { start: 12, label: "02 / THE TEST SCRIPT", title: "One agent. One shared starting line.", description: "100 worker tasks wait at one barrier. Each then requests $0.03 through Hubu's /spend/authorize route using the same agent account.", excerpt: "burst", observation: "burst", caption: "This is concurrent in-process route dispatch, using the real Hubu handler. The test does not send HTTP over a network." },
      { start: 25, label: "03 / THE CAPTURED RUN", title: "The command. The actual output.", description: "This is the exact command and Cargo output saved by the capture script. Read the runner alongside it to see how the recording was made.", source: "runner", caption: "Output is displayed as captured. Playback pacing is editorial; nothing here is a live terminal." },
      { start: 35, label: "04 / HUBU'S DECISIONS", title: "33 allowed. 67 denied.", description: "Inspect actual request and response examples. The denied request records insufficient budget; the captured snapshot has $0.99 held and $0.01 available.", excerpt: "burst", observation: "authorized", caption: "The file includes all 100 decisions. No per-request ordering or timing is inferred from the walkthrough." },
      { start: 50, label: "05 / SETTLE AND RECONCILE", title: "Follow an authorization into the ledger.", description: "The test feeds synthetic executor settlement events to Hubu. All 33 allowed authorizations produce balanced $0.03 ledger transactions.", excerpt: "settlement", observation: "settled", caption: "Settlement inputs are synthetic. The Hubu settlement and ledger code is real; no provider executes or bills a request." },
      { start: 65, label: "06 / REOPEN THE DATABASE", title: "Does the result survive reopening?", description: "The harness closes and reopens SQLite, then checks the budget and ledger again. Compare the captured snapshots and inspect the persistence assertion.", excerpt: "persistence", observation: "reopened", caption: "These are snapshots and transactions read from the reopened database, alongside the exact test code that checks them." }
    ];
    let evidence = null;
    let rawRecording = "";
    let position = 0;
    let playing = false;
    let frame = 0;
    let lastFrame = 0;
    let chapterIndex = -1;
    let loadVersion = 0;
    let focused = false;
    let savedScroll = 0;
    let fullSource = false;
    let activeSource = null;
    let activeExcerpt = null;
    let captureOptions = [];
    const objectUrls = [];
    const put = (id, value) => { $(id).textContent = value; };
    const controls = ["play-button", "previous-button", "next-button", "restart-button", "timeline", "view-evidence", "view-reproduce", "focus-evidence", "download-evidence", "capture-select", "source-mode"];
    const chapterButtons = [...$("chapter-nav").querySelectorAll("button")];

    function recordingUrl(content, type = "text/plain") {
      const url = URL.createObjectURL(new Blob([content], { type }));
      objectUrls.push(url);
      return url;
    }
    function setFocused(value, restoreScroll = true) {
      if (focused === value) return;
      if (value) savedScroll = window.scrollY;
      focused = value;
      document.body.classList.toggle("is-focused", value);
      $("focus-button").setAttribute("aria-pressed", String(value));
      put("focus-button", value ? "Exit focus ↙" : "Focus view ⤢");
      if (value || restoreScroll) window.scrollTo({ top: value ? 0 : savedScroll, behavior: "instant" });
    }
    function setPlaying(value) {
      playing = Boolean(value && evidence);
      put("play-label", playing ? "Pause" : position >= duration ? "Replay" : "Play walkthrough");
      put("play-icon", playing ? "Ⅱ" : "▶");
      $("play-button").setAttribute("aria-label", playing ? "Pause walkthrough" : position >= duration ? "Replay from beginning" : "Play walkthrough");
      cancelAnimationFrame(frame);
      if (playing) {
        lastFrame = performance.now();
        frame = requestAnimationFrame(tick);
      }
    }
    function tick(now) {
      if (!playing) return;
      position = Math.min(duration, position + (now - lastFrame) / 1000);
      lastFrame = now;
      render();
      if (position >= duration) setPlaying(false);
      else frame = requestAnimationFrame(tick);
    }
    function facts(entries) {
      $("chapter-facts").replaceChildren(...entries.map(([label, value]) => {
        const row = document.createElement("div");
        const term = document.createElement("dt");
        const detail = document.createElement("dd");
        term.textContent = label;
        detail.textContent = value;
        row.append(term, detail);
        return row;
      }));
    }
    function snapshotsText(run) {
      const lines = ["CAPTURED USD BUDGET SNAPSHOTS", "", "                 Available   Reserved    Consumed", "                 ─────────   ────────    ────────"];
      for (const [label, snapshot] of [["Initial", run.snapshots.initial], ["After burst", run.snapshots.reserved], ["After settle", run.snapshots.final], ["Reopened", run.persistence.reopened_snapshot]]) {
        lines.push(`${label.padEnd(17)}${money(snapshot.available_units).padEnd(12)}${money(snapshot.reserved_units).padEnd(12)}${money(snapshot.consumed_units)}`);
      }
      lines.push("", `Persisted transactions: ${run.persistence.transaction_count}`, `Transactions match before / after reopen: ${run.persistence.transactions_match}`, "", "Exact reopened snapshot:", json(run.persistence.reopened_snapshot));
      return lines.join("\n");
    }
    function chapterArtifacts(index, run) {
      const setup = run.setup;
      const option = (label, title, context, content, log = false) => ({ label, title, context, content: typeof content === "string" ? content : json(content), log });
      switch (index) {
        case 0:
          facts([["Identity", `${setup.agent.name} · ${setup.agent.version}`], ["Budget", `${money(setup.budget.limit_units)} ${setup.budget.currency} · scale ${setup.budget.scale}`], ["Database", setup.storage.database_name], ["Policy", `${setup.policy.id} · ${setup.policy.version}`]]);
          return [option("Setup + initial balance", "Actual setup", "Temporary SQLite · one agent account", { agent: setup.agent, budget: setup.budget, storage: setup.storage, initial_snapshot: run.snapshots.initial }), option("Policy YAML", "Installed policy", `${setup.policy.id} / ${setup.policy.version}`, setup.policy.yaml)];
        case 1:
          facts([["Agent accounts", String(run.scenario.registered_accounts)], ["Concurrent tasks", String(run.scenario.workers)], ["Per request", money(run.scenario.request_units)], ["Barrier participants", String(setup.barrier_participants)]]);
          return [option("Burst configuration", "Actual dispatch", "One agent · 100 independent operation keys", { authorization_path: setup.authorization_path, dispatch: setup.dispatch, authentication: setup.authentication, barrier_participants: setup.barrier_participants, concurrent_requests: run.scenario.workers, request_units: run.scenario.request_units, requested_total_units: run.scenario.workers * run.scenario.request_units }), option("Example request", "Captured request", setup.authorization_path, run.examples.allowed.request)];
        case 2:
          facts([["Cargo exit code", String(run.recording.exit_code)], ["Output integrity", "SHA-256 checked"], ["Source files", String(run.recording.sources.length)], ["Source tree", run.provenance.dirty ? "Captured working tree" : "Clean commit"]]);
          return [option("Actual Cargo output", "Captured run output", "Complete saved output · result shown; scroll up for build", run.recording.log, true), option("Recording provenance", "Source provenance", "Captured checkout and exact command", run.provenance)];
        case 3:
          facts([["Allowed", `${run.summary.allowed} × ${money(run.scenario.request_units)}`], ["Denied", String(run.summary.denied)], ["Reserved", money(run.snapshots.reserved.reserved_units)], ["Available", money(run.snapshots.reserved.available_units)]]);
          return [option("Denied request + response", "A real denied response", "Captured example · no inferred request order", run.examples.denied), option("Allowed request + response", "A real allowed response", "Token and hold presence recorded as booleans", run.examples.allowed), option("Budget after authorization", "Captured hold snapshot", "Read after all 100 requests complete", run.snapshots.reserved)];
        case 4: {
          const transaction = run.ledger.transactions.find((entry) => entry.authorization_id === run.examples.allowed.response.decision_id);
          const correlation = { operation_key: run.examples.allowed.request.operation_key, decision_id: run.examples.allowed.response.decision_id, ledger_authorization_id: transaction.authorization_id, claim_id: transaction.claim_id, settlement_id: transaction.settlement_id, transaction_id: transaction.id };
          facts([["Settlement input", "Synthetic executor events"], ["Settled", `${run.summary.settled} transactions`], ["Ledger charges", money(Number(run.ledger.coverage.recorded_budget_charges.units))], ["Unaccounted", money(Number(run.ledger.coverage.unaccounted_consumption.units))]]);
          return [option("Follow the allowed request", "Authorization → ledger", "The allowed example from chapter 04 · matching IDs", { correlation, ledger_transaction: transaction }), option("Ledger reconciliation", "Actual ledger coverage", "Charges, consumed budget, and pending holds", run.ledger.coverage), option("Final budget", "Captured final balance", "Read after synthetic settlement completes", run.snapshots.final)];
        }
        default:
          facts([["Consumed", money(run.persistence.reopened_snapshot.consumed_units)], ["Available", money(run.persistence.reopened_snapshot.available_units)], ["Pending holds", String(run.ledger.coverage.pending_hold_count)], ["Transactions match", String(run.persistence.transactions_match)]]);
          return [option("Snapshot comparison", "Before and after reopening", "Actual reads from the temporary SQLite database", snapshotsText(run)), option("Persistence evidence", "Reopened database", "Persisted budget and transaction count", run.persistence)];
      }
    }
    function renderCapture() {
      const selected = captureOptions[Number($("capture-select").value) || 0];
      if (!selected) return;
      put("capture-title", selected.title);
      put("capture-context", selected.context);
      put("capture-content", selected.content);
      $("capture-content").scrollTop = 0;
      $("capture-content").scrollLeft = 0;
      $("capture-content").classList.toggle("log-output", selected.log);
      $("command-box").hidden = !selected.log;
      if (selected.log) {
        put("run-command", evidence.run.recording.command);
        put("command-exit", `exit ${evidence.run.recording.exit_code}`);
        /* Start with the captured outcome visible; the complete output remains scrollable. */
        $("capture-content").scrollTop = $("capture-content").scrollHeight;
      }
    }
    function sourceWindow(source, startNeedle, endNeedle) {
      const lines = source.content.replace(/\n$/, "").split("\n");
      const start = lines.findIndex((line) => line.includes(startNeedle));
      const end = lines.findIndex((line, index) => index >= start && line.includes(endNeedle));
      if (start < 0 || end < start) return source;
      return { path: source.path, start_line: (source.start_line || 1) + start, end_line: (source.start_line || 1) + end, content: lines.slice(start, end + 1).join("\n") };
    }
    function renderSource() {
      if (!activeSource) return;
      const showExcerpt = activeExcerpt && !fullSource;
      const text = showExcerpt ? activeExcerpt.content : activeSource.content;
      const start = showExcerpt ? activeExcerpt.start_line : 1;
      put("source-title", showExcerpt ? activeSource.id === "runner" ? "Capture script excerpt" : "Relevant test code" : activeSource.id === "runner" ? "Capture script" : "Full captured source");
      put("source-mode", showExcerpt ? "Full source ↗" : activeExcerpt ? "Relevant excerpt ↙" : "Full source shown");
      $("source-mode").disabled = !activeExcerpt;
      put("source-path", `${activeSource.path}${showExcerpt ? ` : ${activeExcerpt.start_line}–${activeExcerpt.end_line}` : ""}`);
      const lines = text.replace(/\n$/, "").split("\n");
      const padding = String(start + lines.length - 1).length;
      put("source-content", lines.map((line, index) => `${String(start + index).padStart(padding)} │ ${line}`).join("\n"));
      $("source-content").scrollTop = 0;
      $("source-content").scrollLeft = 0;
      put("source-digest", `SHA-256 ${activeSource.sha256.slice(0, 12)}…`);
      $("source-digest").title = activeSource.sha256;
      const link = $("download-source");
      link.href = activeSource.downloadUrl;
      link.download = activeSource.path.split("/").pop();
      link.hidden = false;
    }
    function renderChapter(index) {
      const chapter = chapters[index];
      const run = evidence.run;
      put("phase-kicker", chapter.label);
      put("phase-title", chapter.title);
      put("phase-description", chapter.description);
      put("scope-caption", chapter.caption);
      const observation = run.observations.find((item) => item.id === chapter.observation);
      put("observation-time", observation ? `Captured observation: ${observation.id} · +${elapsed(observation.elapsed_ms)}` : `Full harness elapsed: ${elapsed(run.provenance.elapsed_ms)}`);
      captureOptions = chapterArtifacts(index, run);
      $("capture-select").replaceChildren(...captureOptions.map((entry, optionIndex) => {
        const node = document.createElement("option");
        node.value = String(optionIndex);
        node.textContent = entry.label;
        return node;
      }));
      renderCapture();
      activeExcerpt = chapter.excerpt ? run.recording.excerpts.find((excerpt) => excerpt.id === chapter.excerpt) : null;
      activeSource = chapter.source ? run.recording.sources.find((source) => source.id === chapter.source) : run.recording.sources.find((source) => source.path === activeExcerpt.path);
      if (chapter.excerpt === "burst") {
        activeExcerpt = sourceWindow(activeExcerpt, "std::thread::spawn", "(worker, operation_key, request, response)");
      } else if (chapter.source === "runner") {
        activeExcerpt = sourceWindow(activeSource, 'with (evidence_dir / "test.log").open', "returncode = process.wait()");
        /* If an older capture has no recognized window, leave its full source available. */
        if (activeExcerpt === activeSource) activeExcerpt = null;
      }
      fullSource = false;
      renderSource();
      chapterButtons.forEach((button, buttonIndex) => {
        button.classList.toggle("active", buttonIndex === index);
        if (buttonIndex === index) button.setAttribute("aria-current", "step");
        else button.removeAttribute("aria-current");
      });
      $("previous-button").disabled = index === 0;
      $("next-button").disabled = index === chapters.length - 1;
      put("phase-announcement", `${chapter.label}. ${chapter.title}`);
      put("requested-total", `${run.scenario.workers} × ${money(run.scenario.request_units)}`);
      put("decisions-total", index >= 3 ? `${run.summary.allowed} / ${run.summary.denied}` : "Inspect in chapter 04");
      const snapshot = index >= 4 ? run.snapshots.final : index >= 3 ? run.snapshots.reserved : run.snapshots.initial;
      put("balance-label", index >= 4 ? "SETTLED / AVAILABLE" : index >= 3 ? "RESERVED / AVAILABLE" : "INITIAL AVAILABLE");
      put("balance-total", index >= 3 ? `${money(index >= 4 ? snapshot.consumed_units : snapshot.reserved_units)} / ${money(snapshot.available_units)}` : money(snapshot.available_units));
      put("result-label", index >= 5 ? "PERSISTENCE" : index >= 4 ? "LEDGER" : index >= 2 ? "CARGO RUN" : "RECORDED SOURCE");
      put("result-total", index >= 5 ? `${run.persistence.transaction_count} txns match` : index >= 4 ? `${run.ledger.transactions.length} balanced txns` : index >= 2 ? `Exit ${run.recording.exit_code}` : `${run.recording.sources.length} files captured`);
    }
    function render() {
      $("timeline").value = String(position);
      $("timeline").setAttribute("aria-valuetext", `${Math.floor(position)} seconds of 75 seconds`);
      $("timeline").style.background = `linear-gradient(to right, var(--orange) ${position / duration * 100}%, var(--line) ${position / duration * 100}%)`;
      put("playback-time", clock(position));
      if (!evidence) return;
      const nextChapter = chapters.findLastIndex((chapter) => position >= chapter.start);
      if (nextChapter !== chapterIndex) {
        chapterIndex = nextChapter;
        renderChapter(chapterIndex);
      }
    }
    function tableRows(id, rows) {
      $(id).replaceChildren(...rows.map((values) => {
        const row = document.createElement("tr");
        values.forEach((value) => {
          const cell = document.createElement("td");
          cell.textContent = String(value);
          row.append(cell);
        });
        return row;
      }));
    }
    function reset() {
      setPlaying(false);
      evidence = null;
      rawRecording = "";
      position = 0;
      chapterIndex = -1;
      activeSource = null;
      activeExcerpt = null;
      captureOptions = [];
      objectUrls.splice(0).forEach((url) => URL.revokeObjectURL(url));
      controls.forEach((id) => { $(id).disabled = true; });
      chapterButtons.forEach((button) => { button.disabled = true; button.classList.remove("active"); button.removeAttribute("aria-current"); });
      $("evidence-status").classList.remove("loaded");
      put("evidence-status", "Evidence not loaded");
      put("phase-kicker", "WAITING FOR A RECORDING");
      put("phase-title", "Show the test. Then the result.");
      put("phase-description", "This walkthrough needs a captured run with its setup, source code, output, and persisted results.");
      $("chapter-facts").replaceChildren();
      put("observation-time", "No execution evidence loaded.");
      put("scope-caption", "The harness exercises Hubu routes in process with test authentication and a temporary SQLite database.");
      put("capture-title", "Captured evidence");
      put("capture-context", "No recorded artifacts loaded");
      $("capture-select").replaceChildren(Object.assign(document.createElement("option"), { textContent: "Awaiting run" }));
      $("command-box").hidden = true;
      put("capture-content", "Run the Episode 01 harness, then load evidence/run.json.\n\nThe recording must include actual setup, source excerpts, Cargo output, request and response examples, and reopened database evidence.");
      put("source-title", "Test source");
      put("source-mode", "Full source ↗");
      put("source-path", "Source not loaded");
      put("source-content", "Exact source captured with this run will appear here.");
      put("source-digest", "No source hash loaded");
      $("download-source").hidden = true;
      $("download-source").removeAttribute("href");
      for (const id of ["requested-total", "decisions-total", "balance-total", "receipt-commit", "receipt-elapsed", "receipt-command", "receipt-log-digest", "ledger-charges", "ledger-consumed", "ledger-unaccounted", "ledger-pending"]) put(id, "—");
      put("balance-label", "CAPTURED BALANCE");
      put("result-label", "RECORDING");
      put("result-total", "Not loaded");
      put("receipt-file", "Not loaded");
      put("reproduce-commit", "Load a recording to see its source commit.");
      put("reproduce-commands", "Load a recording to see its checkout and rerun commands.");
      put("reproduce-caveat", "The recording contains the exact source files used by its harness.");
      $("reproduce-repository").href = "https://github.com/hacker-no-ice/hubu";
      put("ledger-count", "No transactions loaded");
      put("receipt-checks", "A loaded file is not independently authenticated by this viewer.");
      $("recorded-source-links").replaceChildren();
      $("assertion-list").replaceChildren(Object.assign(document.createElement("li"), { textContent: "No assertion evidence loaded." }));
      tableRows("observation-table", [["No observations loaded.", "—", "—"]]);
      tableRows("ledger-table", [["No transactions loaded.", "—", "—", "—", "—"]]);
      tableRows("decision-table", [["No decisions loaded.", "—", "—", "—", "—"]]);
      render();
    }
    function populateReceipt(name) {
      const run = evidence.run;
      const provenance = run.provenance;
      put("receipt-file", name);
      put("reproduce-commit", provenance.commit);
      put("reproduce-commands", `git clone ${provenance.repository_url}.git\ncd hubu\ngit checkout ${provenance.commit}\npython3 scripts/break-hubu-episode-01.py --serve`);
      $("reproduce-repository").href = `${provenance.repository_url}/tree/${provenance.commit}`;
      put("reproduce-caveat", provenance.dirty ? "This recording used a working tree with changes. Download the captured source files below to inspect the exact test and runner; the commit alone may not reproduce those changes." : "This recording used a clean source tree at the commit above. The exact test, runner, and scenario files are also included below.");
      put("receipt-commit", `${provenance.commit}${provenance.dirty ? " · captured working tree" : " · clean tree"}`);
      put("receipt-command", run.recording.command);
      put("receipt-elapsed", elapsed(provenance.elapsed_ms));
      put("receipt-log-digest", run.recording.log_sha256);
      $("assertion-list").replaceChildren(...run.assertions.map((assertion) => {
        const item = document.createElement("li");
        const mark = document.createElement("span");
        mark.className = "assertion-mark";
        mark.textContent = "✓";
        mark.setAttribute("aria-label", "Passed:");
        item.append(mark, document.createTextNode(assertion.name));
        return item;
      }));
      const links = $("recorded-source-links");
      run.recording.sources.forEach((source) => {
        source.downloadUrl = recordingUrl(source.content);
        const link = document.createElement("a");
        link.href = source.downloadUrl;
        link.download = source.path.split("/").pop();
        link.textContent = `Download captured ${source.path}`;
        links.append(link);
      });
      tableRows("observation-table", run.observations.map((observation) => [observation.id, `+${elapsed(observation.elapsed_ms)}`, json(observation.data)]));
      tableRows("decision-table", [...run.requests].sort((a, b) => a.worker - b.worker).map((request) => [String(request.worker).padStart(3, "0"), request.decision, money(request.amount_units), request.decision_id, request.reason.join("; ") || "—"]));
      put("ledger-count", `${run.ledger.transactions.length} canonical transactions`);
      put("ledger-charges", money(Number(run.ledger.coverage.recorded_budget_charges.units)));
      put("ledger-consumed", money(Number(run.ledger.coverage.consumed_amount.units)));
      put("ledger-unaccounted", money(Number(run.ledger.coverage.unaccounted_consumption.units)));
      put("ledger-pending", String(run.ledger.coverage.pending_hold_count));
      tableRows("ledger-table", run.ledger.transactions.map((transaction) => [transaction.id, transaction.authorization_id, money(Number(transaction.budget_charge_delta.units)), money(Number(transaction.budget_charge_delta.units)), money(Number(transaction.budget_charge_delta.units))]));
      put("receipt-checks", `${evidence.checks.length} viewer checks passed: ${evidence.checks.join("; ")}. Captured source and log SHA-256 digests verified. Hashes check integrity; the viewer does not independently authenticate the recording.`);
    }
    async function loadText(raw, name, version) {
      if (version !== loadVersion) return;
      reset();
      try {
        if (raw.length > maxFileSize) throw new Error("The evidence file exceeds the 3 MB limit.");
        const checked = validateEvidence(JSON.parse(raw));
        await verifyRecordingDigests(checked.run);
        if (version !== loadVersion) return;
        evidence = checked;
        rawRecording = raw;
        controls.forEach((id) => { $(id).disabled = false; });
        chapterButtons.forEach((button) => { button.disabled = false; });
        $("evidence-status").classList.add("loaded");
        put("evidence-status", "Captured evidence loaded");
        $("load-message").classList.remove("error");
        put("load-message", `${name} · source and log integrity checked · inspect or replay all six chapters.`);
        populateReceipt(name);
        render();
      } catch (error) {
        if (version !== loadVersion) return;
        reset();
        $("load-message").classList.add("error");
        put("load-message", `Evidence not loaded: ${error instanceof Error ? error.message : "The file could not be read."} Rerun the Episode 01 capture harness to create a complete recording.`);
      }
    }
    function jump(index) {
      if (!evidence) return;
      position = chapters[Math.max(0, Math.min(chapters.length - 1, index))].start;
      render();
    }
    chapterButtons.forEach((button) => button.addEventListener("click", () => { setPlaying(false); jump(Number(button.dataset.chapter)); }));
    $("previous-button").addEventListener("click", () => { setPlaying(false); jump(chapterIndex - 1); });
    $("next-button").addEventListener("click", () => { setPlaying(false); jump(chapterIndex + 1); });
    $("play-button").addEventListener("click", () => {
      if (position >= duration) { position = 0; chapterIndex = -1; render(); }
      setPlaying(!playing);
    });
    $("restart-button").addEventListener("click", () => { setPlaying(false); position = 0; chapterIndex = -1; render(); });
    $("timeline").addEventListener("input", (event) => { position = Number(event.target.value); render(); if (position >= duration) setPlaying(false); });
    $("capture-select").addEventListener("change", () => { setPlaying(false); renderCapture(); });
    $("source-mode").addEventListener("click", () => { setPlaying(false); fullSource = !fullSource; renderSource(); });
    $("focus-button").addEventListener("click", () => setFocused(!focused));
    document.addEventListener("visibilitychange", () => { if (document.hidden) setPlaying(false); });
    document.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && focused) { setFocused(false); $("focus-button").focus({ preventScroll: true }); }
    });
    function inspectEvidence() {
      setPlaying(false);
      setFocused(false, false);
      $("evidence-drawer").open = true;
      $("evidence-drawer").scrollIntoView({ behavior: reducedMotion.matches ? "instant" : "smooth", block: "start" });
      $("evidence-drawer").querySelector("summary").focus({ preventScroll: true });
    }
    $("view-evidence").addEventListener("click", inspectEvidence);
    $("view-reproduce").addEventListener("click", () => { inspectEvidence(); $("reproduce-title").scrollIntoView({ behavior: reducedMotion.matches ? "instant" : "smooth", block: "start" }); });
    $("focus-evidence").addEventListener("click", inspectEvidence);
    $("download-evidence").addEventListener("click", () => {
      if (!evidence) return;
      const link = document.createElement("a");
      link.href = recordingUrl(rawRecording, "application/json");
      link.download = "episode-01-evidence.json";
      link.click();
    });
    $("evidence-file").addEventListener("change", async (event) => {
      const file = event.target.files[0];
      if (!file) return;
      const version = ++loadVersion;
      if (file.size > maxFileSize) {
        reset();
        $("load-message").classList.add("error");
        put("load-message", "Evidence not loaded: the evidence file exceeds the 3 MB limit.");
      } else {
        try { await loadText(await file.text(), file.name, version); }
        catch { if (version === loadVersion) { reset(); $("load-message").classList.add("error"); put("load-message", "Evidence not loaded: the selected file could not be read."); } }
      }
      event.target.value = "";
    });
    reset();
    const embedded = $("recorded-run");
    if (embedded) {
      loadText(embedded.textContent, "Embedded recorded run", loadVersion);
    } else if (location.protocol === "http:" || location.protocol === "https:") {
      const version = loadVersion;
      fetch("evidence/run.json", { cache: "no-store" }).then(async (response) => {
        if (!response.ok || version !== loadVersion) return;
        const length = Number(response.headers.get("content-length"));
        if (length > maxFileSize) throw new Error("Evidence exceeds 3 MB.");
        await loadText(await response.text(), "evidence/run.json", version);
      }).catch((error) => {
        if (version !== loadVersion) return;
        $("load-message").classList.add("error");
        put("load-message", `Evidence not loaded: ${error instanceof Error ? error.message : "The recording could not be fetched."} Run the Episode 01 capture harness or load a recording.`);
      });
    }
  })();
}
