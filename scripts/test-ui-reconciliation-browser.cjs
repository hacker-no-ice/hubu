#!/usr/bin/env node
// Run with NODE_PATH pointing at an isolated Playwright install. All HTTP
// responses are fixtures; this never connects to Hubu, Gongbu or a provider.
const { chromium } = require('playwright');
const { readFileSync } = require('node:fs');
const { resolve } = require('node:path');
const assert = require('node:assert/strict');
const root = resolve(__dirname, '..');
const asset = (name) => readFileSync(resolve(root, 'crates/hubu-cli/src/ui/assets', name));
const money = (units) => ({ units: String(units), scale: 6, currency: 'usd', display: (units / 1e6).toString() });
const pending = () => ({ claim_id: 'fixture-claim', reconciliation_required: true, status: 'claimed',
  spend: { currency: 'usd', amount: money(135120), budget_hold: { budget_id: 'bgt_fixture', consumed_amount: money(0), frozen_amount: money(135120), remaining_amount: money(864880) } } });
const event = { record_id: 'lev_fixture', category: 'reconciliation_entry', outcome: 'reconciliation_required', occurred_at: '2026-11-04T23:49:42Z' };
(async () => {
  const browser = await chromium.launch({ headless: true, ...(process.env.HUBU_UI_CHROME ? { executablePath: process.env.HUBU_UI_CHROME } : {}) });
  try {
    for (const scenario of ['billed', 'not_billed', 'validation', 'concurrent', 'legacy', 'active']) {
      const page = await browser.newPage({ viewport: { width: 1440, height: 1100 } });
      const errors = []; page.on('pageerror', (error) => errors.push(error.message));
      let claim = pending(), receipt = null, posts = 0, posted;
      if (scenario === 'legacy') claim = { ...claim, status: 'settled', reconciliation_required: false, finalized_at: '2026-11-05T00:05:14Z' };
      if (scenario === 'active') claim.reconciliation_required = false;
      await page.route('**/*', async (route) => {
        const request = route.request(); const url = new URL(request.url());
        let data = {};
        if (url.pathname === '/') return route.fulfill({ contentType: 'text/html', body: asset('index.html') });
        if (url.pathname === '/app.js') return route.fulfill({ contentType: 'text/javascript', body: asset('app.js') });
        if (url.pathname === '/app.css') return route.fulfill({ contentType: 'text/css', body: asset('app.css') });
        if (url.pathname.endsWith('.svg')) return route.fulfill({ contentType: 'image/svg+xml', body: '<svg xmlns="http://www.w3.org/2000/svg"/>' });
        if (url.pathname === '/api/session') data = { authenticated: true, reconciliation_capability: true };
        if (url.pathname === '/api/hubu/agents') data = { agents: [] };
        if (url.pathname === '/api/hubu/user') data = { display_name: 'Fixture owner' };
        if (url.pathname === '/api/hubu/governance/summary') data = { summary: { backlog: {} } };
        if (url.pathname === '/api/hubu/governance/records') data = { records: [event], summary: { backlog: {} } };
        if (url.pathname.endsWith('/lev_fixture/reconciliation')) data = { claim, receipt };
        if (url.pathname === '/api/hubu/spend/executor/reconciliation') data = { claims: claim.reconciliation_required ? [claim] : [] };
        if (url.pathname === '/api/hubu/spend/executor/claim') {
          if (scenario === 'concurrent') claim = { ...claim, status: 'released', reconciliation_required: false };
          data = claim;
        }
        if (request.method() === 'POST') {
          posts++; posted = request.postDataJSON();
          assert.equal(request.headers()['x-hubu-ui-confirmed'], '1');
          if (scenario === 'validation' && posts === 1) return route.fulfill({ status: 400, json: { error: 'Fixture receipt validation failed' } });
          claim = { ...claim, reconciliation_required: false, status: posted.receipt ? 'settled' : 'released',
            evidence: posted.evidence, provider_reference: posted.provider_reference, reconciled_at: '2026-11-18T01:00:00Z', reconciled_by_user_id: 'usr_fixture' };
          if (posted.receipt) receipt = { ...posted.receipt, budget_charge: money(135121) };
          data = claim;
        }
        return route.fulfill({ json: data });
      });
      await page.goto('http://hubu-fixture.test/#/decisions');
      await page.getByRole('button', { name: 'Details', exact: true }).click();
      if (scenario === 'legacy' || scenario === 'active') {
        if (scenario === 'legacy') await page.getByText('Not recorded (legacy evidence unavailable)', { exact: true }).waitFor();
        assert.equal(await page.getByRole('button', { name: 'Resolve reconciliation…' }).count(), 0);
        if (scenario === 'active') await page.locator('dialog').getByText('Active claim · not eligible', { exact: true }).waitFor();
        assert.match(await page.locator('dialog').innerText(), scenario === 'legacy' ? /Reconciled · billed \/ settled/ : /Active claim · not eligible/);
      } else {
        await page.getByRole('button', { name: 'Resolve reconciliation…' }).click();
        const dialog = page.locator('dialog').last();
        await dialog.getByLabel('Provider reference', { exact: true }).fill('fixture-provider-ref');
        await dialog.getByLabel('Supporting evidence').fill('Isolated fixture evidence');
        if (scenario !== 'not_billed' && scenario !== 'concurrent') {
          await dialog.getByLabel(/^Outcome/).selectOption('billed');
          await dialog.getByLabel('Confirmed actual cost').fill('0.13512001');
          await dialog.getByLabel('Provider request ID').fill('fixture-request');
          await dialog.getByLabel('Artifact / receipt reference').fill('fixture-receipt');
          await dialog.getByLabel('Price/model snapshot').fill('{"provider":"fixture","model":"fixture","currency":"usd"}');
        }
        await dialog.getByRole('button', { name: 'Review reconciliation', exact: true }).click();
        await dialog.getByText(/Consumed increases by/).waitFor();
        if (process.env.HUBU_UI_SCREENSHOT && scenario === 'billed') await page.screenshot({ path: process.env.HUBU_UI_SCREENSHOT, fullPage: true });
        await dialog.getByRole('button', { name: 'Confirm reconciliation', exact: true }).evaluate((button) => { button.dispatchEvent(new MouseEvent('click')); button.dispatchEvent(new MouseEvent('click')); });
        if (scenario === 'validation' || scenario === 'concurrent') {
          await dialog.getByText(scenario === 'validation' ? 'Fixture receipt validation failed' : /Claim is no longer eligible/).waitFor();
          assert.equal(await dialog.getByLabel('Supporting evidence').inputValue(), 'Isolated fixture evidence');
          assert.equal(await dialog.getByRole('button', { name: 'Confirm reconciliation', exact: true }).count(), 0);
          assert.equal(posts, scenario === 'validation' ? 1 : 0);
          if (scenario === 'validation') {
            await dialog.getByLabel('Price/model snapshot').fill('{"provider":"fixture","model":"corrected","currency":"usd"}');
            await dialog.getByRole('button', { name: 'Review reconciliation', exact: true }).click();
            await dialog.getByRole('button', { name: 'Confirm reconciliation', exact: true }).click();
            await page.getByText('No outstanding reconciliation claims.', { exact: true }).waitFor();
            assert.equal(posts, 2);
            assert.equal(posted.evidence, 'Isolated fixture evidence');
          }
        } else {
          await page.getByText('No outstanding reconciliation claims.', { exact: true }).waitFor();
          assert.equal(posts, 1);
          assert.equal(posted.claim_id, 'fixture-claim');
          assert.equal(Boolean(posted.receipt), scenario === 'billed');
          if (scenario === 'billed') assert.equal(posted.receipt.actual_vendor_cost.amount, 13512001);
          await page.getByRole('button', { name: 'Details', exact: true }).click();
          await page.getByText('Isolated fixture evidence', { exact: true }).first().waitFor();
        }
      }
      assert.deepEqual(errors, []);
      console.log(`PASS ${scenario}`);
      await page.close();
    }
  } finally { await browser.close(); }
})().catch((error) => { console.error(error); process.exitCode = 1; });
