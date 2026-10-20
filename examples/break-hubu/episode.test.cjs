"use strict";

const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const { validateEvidence } = require("./episode.js");

// Use an actual harness result, never a prefilled passing viewer fixture.
const path = process.env.HUBU_BREAK_TEST_EVIDENCE;
assert.ok(path, "Set HUBU_BREAK_TEST_EVIDENCE to the harness evidence/run.json");
const recorded = JSON.parse(fs.readFileSync(path, "utf8"));
const changed = (mutate) => {
  const run = structuredClone(recorded);
  mutate(run);
  return run;
};

test("accepts the real run and reconciles all decisions", () => {
  const result = validateEvidence(recorded);
  assert.equal(result.allowed, 33);
  assert.equal(result.denied, 67);
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
