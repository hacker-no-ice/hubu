#!/usr/bin/env node
// Pure UI contract checks; no browser, running stack, provider or npm install.
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import test from "node:test";
import assert from "node:assert/strict";
const source = readFileSync(new URL("../crates/hubu-cli/src/ui/assets/app.js", import.meta.url), "utf8");
const ui = runInNewContext(`${source.replace(/\nboot\(\);\s*$/, "")}\n({ reconciliationPayload, reconciliationBudgetChange, reconciliationLabel, withReconciliation });`, { URLSearchParams });
const claim = { claim_id: "isolated-fixture", status: "claimed", reconciliation_required: true,
  spend: { currency: "usd", amount: { units: "135120", scale: 6, currency: "usd" } } };
const values = { outcome: "billed", amount: "0.13512001", evidence: "fixture evidence", provider_reference: "fixture-ref",
  provider_request_id: "fixture-request", artifact_reference: "fixture-receipt", price_model_snapshot: '{"provider":"fixture","model":"fixture","currency":"usd"}' };
test("billed cost is exact major units and budget rounds up once", () => {
  const body = ui.reconciliationPayload(claim, values);
  assert.equal(body.receipt.actual_vendor_cost.amount, 13512001);
  assert.equal(body.receipt.actual_vendor_cost.scale, 8);
  assert.match(ui.reconciliationBudgetChange(claim, body), /Consumed increases by 0.135121 USD/);
  assert.match(ui.reconciliationBudgetChange(claim, body), /available changes by -0.000001 USD/);
});
test("not billed sends no receipt and releases the entire estimate", () => {
  const body = ui.reconciliationPayload(claim, { ...values, outcome: "not_billed" });
  assert.equal(body.receipt, undefined);
  assert.match(ui.reconciliationBudgetChange(claim, body), /Consumed increases by 0.00 USD/);
  assert.match(ui.reconciliationBudgetChange(claim, body), /available changes by 0.13512 USD/);
});
test("validation rejects missing evidence, malformed snapshots and unsafe amounts", () => {
  for (const change of [{ evidence: " " }, { provider_reference: "" }, { amount: "-1" }, { amount: "1e-2" },
    { amount: "0.0000000000000000001" }, { amount: "9007199254740993" }, { price_model_snapshot: "[]" },
    { price_model_snapshot: "invalid" }, { artifact_reference: " " }]) {
    assert.throws(() => ui.reconciliationPayload(claim, { ...values, ...change }));
  }
});
test("historical events show current pending, resolved and unknown state", () => {
  assert.match(ui.reconciliationLabel(claim), /Pending/);
  assert.match(ui.reconciliationLabel({ status: "settled" }), /Reconciled.*billed \/ settled/);
  assert.match(ui.reconciliationLabel({ status: "released" }), /not billed \/ released/);
  assert.match(ui.reconciliationLabel({ status: "claimed", reconciliation_required: false }), /not eligible/);
  assert.match(ui.reconciliationLabel(null), /unavailable/);
});
