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
  require(object(data) && data.schema_version === 2, "Expected Episode 01 evidence with schema_version 2; rerun the harness to capture the walkthrough.");
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
  const claims = new Set();
  const entryAmount = (entry) => {
    require(object(entry) && object(entry.amount), "A ledger entry is malformed.");
    const value = entry.amount;
    require(typeof value.amount === "string" && /^\d{1,40}$/.test(value.amount) && Number.isInteger(value.scale) && value.scale >= 0 && value.scale <= 18 && value.currency === "usd", "A ledger entry amount is malformed.");
    return BigInt(value.amount) * (10n ** BigInt(18 - value.scale));
  };
  for (const transaction of ledger.transactions) {
    require(object(transaction) && nonempty(transaction.id) && !transactions.has(transaction.id), "Every ledger transaction needs a distinct ID.");
    require(transaction.kind === "external_provider" && nonempty(transaction.claim_id) && !claims.has(transaction.claim_id), "Expected distinct executor claims and external-provider settlement postings.");
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
    claims.add(transaction.claim_id);
    authorizations.delete(transaction.authorization_id);
  }
  require(authorizations.size === 0, "An allowed authorization has no ledger settlement.");
  checks.push("All 33 allowed decisions match balanced $0.03 ledger settlements");

  require(Array.isArray(data.assertions) && data.assertions.length > 0 && data.assertions.length <= 50, "Recorded test assertions are missing.");
  require(data.assertions.every((assertion) => object(assertion) && nonempty(assertion.name) && assertion.passed === true), "A recorded test assertion failed or is malformed; passing playback is unavailable.");
  const expectedAssertions = ["callers_completed", "budget_decisions", "holds_before_settlement", "denied_no_authority", "balanced_settlements", "ledger_reconciled", "remaining_below_request", "durable_reopen"];
  require(data.assertions.length === expectedAssertions.length && new Set(data.assertions.map((assertion) => assertion.id)).size === expectedAssertions.length && expectedAssertions.every((id) => data.assertions.some((assertion) => assertion.id === id)), "The recording must include all eight Episode 01 assertions, including SQLite persistence.");
  checks.push(`${data.assertions.length} recorded test assertions passed`);
  {
    require(object(data.provenance), "Run provenance is malformed.");
    const provenance = data.provenance;
    require(typeof provenance.commit === "string" && /^[a-f0-9]{40}$/.test(provenance.commit) && typeof provenance.dirty === "boolean", "Run provenance needs a full commit SHA and a working-tree status.");
    require(provenance.repository_url === "https://github.com/hacker-no-ice/hubu" && provenance.test_file === "crates/hubu-api/src/tests/break_hubu.rs" && nonempty(provenance.test_command), "Run provenance must identify the Episode 01 source test and command.");
    require(provenance.elapsed_ms === undefined || (Number.isFinite(provenance.elapsed_ms) && provenance.elapsed_ms >= 0), "Captured elapsed time is malformed.");
  }
  const setup = data.setup;
  require(object(setup) && object(setup.agent) && object(setup.budget) && object(setup.policy) && object(setup.storage), "The captured setup is missing; rerun the harness.");
  require(setup.agent.name === "break-hubu-episode-01" && setup.agent.version === "v1" && setup.agent.agent_id === scenario.agent_id && setup.agent.account_id === scenario.account_id, "Captured agent setup does not match the run.");
  require(setup.budget.budget_id === scenario.budget_id && setup.budget.limit_units === scenario.budget_units && setup.budget.currency === "USD" && setup.budget.scale === 6, "Captured budget setup does not match the run.");
  require(setup.policy.id === "break_hubu_episode_01" && setup.policy.version === "v1" && setup.policy.yaml === "id: break_hubu_episode_01\nversion: v1\ndefault_effect: allow\nrules: []\n", "Expected the actual permissive policy used to isolate the budget gate.");
  require(setup.storage.kind === "temporary_sqlite" && setup.storage.database_name === "episode-01.sqlite" && setup.dispatch === "in_process_route" && setup.authentication === "test_fixture" && setup.authorization_path === "/spend/authorize" && setup.barrier_participants === 101, "Captured setup does not describe the isolated API concurrency test.");

  const observationIds = ["setup", "burst", "authorized", "settled", "reopened"];
  require(Array.isArray(data.observations) && data.observations.length === observationIds.length, "Captured test milestones are missing.");
  let previousElapsed = 0;
  data.observations.forEach((observation, index) => {
    require(object(observation) && observation.id === observationIds[index] && Number.isFinite(observation.elapsed_ms) && observation.elapsed_ms >= previousElapsed && object(observation.data), "Test milestones must retain their captured order and elapsed times.");
    previousElapsed = observation.elapsed_ms;
  });
  const sameSnapshot = (actual, expected) => object(actual) && ["available_units", "reserved_units", "consumed_units"].every((field) => actual[field] === expected[field]);
  const sameIds = (actual, expected) => Array.isArray(actual) && actual.length === expected.size && new Set(actual).size === expected.size && actual.every((id) => expected.has(id));
  const [observedSetup, observedBurst, observedAuthorization, observedSettlement, observedReopen] = data.observations.map((observation) => observation.data);
  require(observedSetup.agent_id === scenario.agent_id && observedSetup.account_id === scenario.account_id && observedSetup.budget_id === scenario.budget_id && sameSnapshot(observedSetup.snapshot, data.snapshots.initial), "The setup milestone does not match the registered account and initial budget.");
  require(observedBurst.spawned_callers === 100 && observedBurst.barrier_participants === 101 && observedBurst.request_units === 30000, "The captured burst does not match the concurrency scenario.");
  require(observedAuthorization.allowed === 33 && observedAuthorization.denied === 67 && observedAuthorization.holds === 33 && observedAuthorization.tokens === 33 && observedAuthorization.claims === 0 && observedAuthorization.transactions === 0 && sameSnapshot(observedAuthorization.snapshot, data.snapshots.reserved) && sameIds(observedAuthorization.decision_ids, new Set(allowed.map((request) => request.decision_id))), "The authorization milestone does not match the actual decisions and holds.");
  require(observedSettlement.transaction_count === 33 && sameSnapshot(observedSettlement.snapshot, data.snapshots.final) && sameIds(observedSettlement.claim_ids, claims) && sameIds(observedSettlement.settlement_ids, settlements) && JSON.stringify(observedSettlement.coverage) === JSON.stringify(coverage), "The settlement milestone does not match the canonical ledger.");
  require(observedReopen.transaction_count === 33 && observedReopen.settled_claims === 33 && observedReopen.denied_attempts === 67 && observedReopen.transactions_match === true && sameSnapshot(observedReopen.snapshot, data.snapshots.final), "The SQLite reopen milestone does not match the final state.");
  require(object(data.persistence) && data.persistence.transactions_match === true && data.persistence.transaction_count === 33 && object(data.persistence.reopened_snapshot), "The reopened database result is missing.");
  for (const field of ["available_units", "reserved_units", "consumed_units"]) require(data.persistence.reopened_snapshot[field] === data.snapshots.final[field], "Reopened SQLite balances differ from the final snapshot.");

  require(object(data.examples), "Representative captured responses are missing.");
  for (const [id, decision] of [["allowed", "allow"], ["denied", "deny"]]) {
    const example = data.examples[id];
    require(object(example) && object(example.request) && object(example.response), "A captured request/response example is missing.");
    const request = data.requests.find((item) => item.operation_key === example.request.operation_key);
    require(request && request.decision === decision && example.request.account_id === scenario.account_id && Number(example.request.amount) === 0.03 && example.request.currency === "usd", "A captured request example does not match a recorded operation.");
    const response = example.response;
    require(response.status === 200 && response.decision === decision && response.decision_id === request.decision_id && JSON.stringify(response.reasons) === JSON.stringify(request.reason) && response.has_authorization_token === (decision === "allow") && response.has_budget_hold === (decision === "allow") && !("auth_token_id" in response), "A captured response does not match its recorded decision.");
  }

  const recording = data.recording;
  const digest = (value) => typeof value === "string" && /^[a-f0-9]{64}$/.test(value);
  require(object(recording) && recording.exit_code === 0 && recording.command === data.provenance.test_command && typeof recording.log === "string" && recording.log.length > 0 && recording.log.length <= 1000000 && digest(recording.log_sha256), "The actual command, successful exit status, or captured test log is missing.");
  require(recording.log.includes("break_hubu_episode_01_hundred_concurrent_callers_cannot_overdraw_one_dollar") && recording.log.includes("test result: ok. 1 passed; 0 failed;"), "The captured Cargo output does not show the Episode 01 test passing.");
  const sourcePaths = new Map([["test", "crates/hubu-api/src/tests/break_hubu.rs"], ["runner", "scripts/break-hubu-episode-01.py"], ["scenario", "examples/break-hubu/scenario.json"]]);
  require(Array.isArray(recording.sources) && recording.sources.length === sourcePaths.size && new Set(recording.sources.map((source) => source?.id)).size === sourcePaths.size, "The three captured source files are required.");
  for (const source of recording.sources) require(object(source) && source.path === sourcePaths.get(source.id) && typeof source.content === "string" && source.content.length > 0 && source.content.length <= 100000 && digest(source.sha256), "A captured source file is missing or malformed.");
  const capturedScenario = recording.sources.find((source) => source.id === "scenario");
  const scenarioInput = JSON.parse(capturedScenario.content);
  require(["workers", "registered_accounts", "budget_units", "request_units", "currency", "scale", "provider_mode"].every((field) => scenarioInput[field] === scenario[field]) && capturedScenario.sha256 === data.provenance.scenario_sha256, "Captured scenario source does not match the run configuration.");
  const excerptIds = ["setup", "burst", "settlement", "persistence"];
  require(Array.isArray(recording.excerpts) && recording.excerpts.length === excerptIds.length && new Set(recording.excerpts.map((excerpt) => excerpt?.id)).size === excerptIds.length, "Captured source excerpts are missing.");
  const testSource = recording.sources.find((source) => source.id === "test");
  const lines = testSource.content.match(/[^\n]*\n|[^\n]+$/g) || [];
  for (const excerpt of recording.excerpts) {
    require(object(excerpt) && excerptIds.includes(excerpt.id) && excerpt.path === testSource.path && Number.isInteger(excerpt.start_line) && Number.isInteger(excerpt.end_line) && excerpt.start_line >= 1 && excerpt.end_line >= excerpt.start_line && excerpt.end_line <= lines.length, "A source excerpt has invalid line references.");
    require(excerpt.content === lines.slice(excerpt.start_line - 1, excerpt.end_line).join(""), "A displayed source excerpt differs from its captured source file.");
  }
  checks.push("Actual setup, request examples, runtime milestones, and database reopen result");
  checks.push("Command output and exact captured source excerpts");
  return { run: data, checks, allowed: allowed.length, denied: denied.length };
}

async function verifyRecordingDigests(run) {
  const cryptoApi = globalThis.crypto?.subtle ? globalThis.crypto :
    typeof require === "function" ? require("node:crypto").webcrypto : null;
  if (!cryptoApi?.subtle) throw new Error("This browser cannot verify captured artifact hashes. Open the bundle in a current browser or on localhost.");
  const hash = async (content) => {
    const bytes = await cryptoApi.subtle.digest("SHA-256", new TextEncoder().encode(content));
    return Array.from(new Uint8Array(bytes), (byte) => byte.toString(16).padStart(2, "0")).join("");
  };
  if (await hash(run.recording.log) !== run.recording.log_sha256) throw new Error("Captured test output does not match its recorded SHA-256.");
  for (const source of run.recording.sources) {
    if (await hash(source.content) !== source.sha256) throw new Error(`Captured source does not match its recorded SHA-256: ${source.path}`);
  }
  return true;
}

if (typeof module !== "undefined" && module.exports) module.exports = { validateEvidence, verifyRecordingDigests };



