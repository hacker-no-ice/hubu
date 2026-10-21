"use strict";

const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const paths = require("node:path");
const { validateEvidence, verifyRecordingDigests } = require("./evidence.js");

// Use an actual harness result, never a prefilled passing viewer fixture.
const path = process.env.HUBU_BREAK_TEST_EVIDENCE;
assert.ok(path, "Set HUBU_BREAK_TEST_EVIDENCE to the harness evidence/run.json");
const recorded = JSON.parse(fs.readFileSync(path, "utf8"));
const changed = (mutate) => {
  const run = structuredClone(recorded);
  mutate(run);
  return run;
};

test("accepts the real run, reconciles decisions, and verifies captured bytes", async () => {
  const result = validateEvidence(recorded);
  assert.equal(result.allowed, 33);
  assert.equal(result.denied, 67);
  assert.equal(await verifyRecordingDigests(recorded), true);
});

test("standalone HTML embeds the same captured run as inert JSON", async () => {
  const html = fs.readFileSync(paths.resolve(paths.dirname(path), "../index.html"), "utf8");
  const embedded = html.match(/<script id="recorded-run" type="application\/json">([\s\S]*?)<\/script>/);
  assert.ok(embedded, "Generated HTML must carry its recording for offline replay");
  assert.equal(embedded[1].includes("<"), false);
  const run = JSON.parse(embedded[1]);
  assert.deepEqual(run, recorded);
  validateEvidence(run);
  assert.equal(await verifyRecordingDigests(run), true);
});

test("rejects truncated and duplicated decisions", () => {
  assert.throws(() => validateEvidence(changed((r) => r.requests.pop())));
  assert.throws(() => validateEvidence(changed((r) => { r.requests[1] = r.requests[0]; })));
});

test("rejects an oversubscribed budget or unreconciled summary", () => {
  assert.throws(() => validateEvidence(changed((r) => { r.snapshots.reserved.reserved_units = 1020000; })));
  assert.throws(() => validateEvidence(changed((r) => { r.summary.allowed = 34; })));
});

test("rejects missing or unaccounted ledger charges", () => {
  assert.throws(() => validateEvidence(changed((r) => r.ledger.transactions.pop())));
  assert.throws(() => validateEvidence(changed((r) => { r.ledger.coverage.unaccounted_consumption.units = "30000"; })));
});

test("rejects balanced postings with the wrong amount", () => {
  assert.throws(() => validateEvidence(changed((r) => {
    for (const entry of r.ledger.transactions[0].entries) entry.amount.amount = "40000000000000000";
  })));
});

test("rejects a denied authorization appearing in the ledger", () => {
  assert.throws(() => validateEvidence(changed((r) => {
    r.ledger.transactions[0].authorization_id = r.requests.find((q) => q.decision === "deny").decision_id;
  })));
});

test("rejects policy denials presented as budget denials", () => {
  assert.throws(() => validateEvidence(changed((r) => {
    r.requests.find((q) => q.decision === "deny").reason = ["merchant denied by policy"];
  })));
});

test("rejects a balanced posting to the wrong ledger account kind", () => {
  assert.throws(() => validateEvidence(changed((r) => {
    r.ledger.transactions[0].entries.find((e) => e.direction === "credit").account_kind = "AgentSpendExpense";
  })));
});

test("rejects failed assertions and forged source destinations", () => {
  assert.throws(() => validateEvidence(changed((r) => { r.assertions[0].passed = false; })));
  assert.throws(() => validateEvidence(changed((r) => { r.provenance.repository_url = "https://example.com"; })));
});

test("rejects an unrelated passing assertion replacing the persistence proof", () => {
  assert.throws(() => validateEvidence(changed((r) => { r.assertions = [{ id: "other", name: "Other test", passed: true }]; })));
});

test("rejects adjustment rows presented as executor settlements", () => {
  assert.throws(() => validateEvidence(changed((r) => {
    for (const transaction of r.ledger.transactions) transaction.kind = "adjustment";
  })));
});

test("rejects setup and representative responses from another account", () => {
  assert.throws(() => validateEvidence(changed((r) => { r.setup.agent.account_id = "unrelated"; })));
  assert.throws(() => validateEvidence(changed((r) => { r.examples.denied.request.account_id = "unrelated"; })));
});

test("rejects a persistence snapshot that differs from the final database", () => {
  assert.throws(() => validateEvidence(changed((r) => { r.persistence.reopened_snapshot.consumed_units = 0; })));
});

test("rejects runtime milestones that contradict the recorded results", () => {
  assert.throws(() => validateEvidence(changed((r) => { r.observations[2].data.allowed = 34; })));
  assert.throws(() => validateEvidence(changed((r) => { r.observations[3].data.claim_ids.pop(); })));
});

test("rejects invented excerpts and nonzero command exit status", () => {
  assert.throws(() => validateEvidence(changed((r) => { r.recording.excerpts[0].content = "pretend setup"; })));
  assert.throws(() => validateEvidence(changed((r) => { r.recording.exit_code = 1; })));
});

test("detects changed captured output and source bytes", async () => {
  await assert.rejects(verifyRecordingDigests(changed((r) => { r.recording.log += "tampered"; })));
  await assert.rejects(verifyRecordingDigests(changed((r) => { r.recording.sources[0].content += "tampered"; })));
});
