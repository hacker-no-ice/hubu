#!/usr/bin/env node
// Exercise the real portal and hubu-server with an isolated fixture database.
const assert = require('node:assert/strict');
const { execFileSync, spawn } = require('node:child_process');
const { once } = require('node:events');
const { mkdtempSync, rmSync, writeFileSync } = require('node:fs');
const { connect, createServer } = require('node:net');
const { tmpdir } = require('node:os');
const { join, resolve } = require('node:path');
const { setTimeout: delay } = require('node:timers/promises');
const { chromium } = require('playwright');

const loopback = '127.0.0.1';
const bin = resolve(process.env.HUBU_UI_BIN_DIR || join(__dirname, '..', 'target', 'debug'));

async function listen(server) {
  server.listen(0, loopback);
  await once(server, 'listening');
  return server.address().port;
}

async function stop(child) {
  if (child && child.exitCode === null && child.signalCode === null) {
    const exited = once(child, 'exit');
    child.kill();
    await exited;
  }
}

async function main() {
  const directory = mkdtempSync(join(tmpdir(), 'hubu-ui-owner-presence-'));
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('HUBU_')));
  Object.assign(env, {
    HUBU_HOME: directory,
    HUBU_LOG_STDERR: '0',
    HUBU_DB_PATH: join(directory, 'hubu.sqlite'),
    HUBU_AUTH_TOKEN: 'isolated-portal-owner-fixture',
    HUBU_AGENT_TOKEN: 'isolated-portal-agent-fixture',
    HUBU_OWNER_PRESENCE_PROVIDER: 'fixture',
    HUBU_OWNER_PRESENCE_FIXTURE_RATE_LIMIT: 'on',
    // Init, policy, budget, two override changes, approval, two budget changes, then refusals.
    HUBU_OWNER_PRESENCE_FIXTURE: 'confirm,confirm,confirm,confirm,confirm,confirm,confirm,confirm,timeout,deny,unavailable,timeout',
  });
  let server, portal, proxy, browser;
  try {
    const socket = createServer();
    const port = await listen(socket);
    await new Promise((done) => socket.close(done));
    const serverUrl = `http://${loopback}:${port}`;
    server = spawn(join(bin, 'hubu-server'), [`${loopback}:${port}`], { cwd: directory, env, stdio: ['ignore', 'ignore', 'inherit'] });
    const serverRequest = async (path, body) => {
      const response = await fetch(serverUrl + path, {
        method: body ? 'POST' : 'GET',
        headers: { Authorization: `Bearer ${env.HUBU_AUTH_TOKEN}`, 'Content-Type': 'application/json' },
        body: body ? JSON.stringify(body) : undefined,
      });
      const data = await response.json();
      assert.ok(response.ok, `${path} returned ${response.status}: ${JSON.stringify(data)}`);
      return data;
    };
    for (let retries = 0; ; retries++) {
      try { await serverRequest('/health'); break; } catch (error) {
        if (server.exitCode !== null || retries === 100) throw error;
        await delay(50);
      }
    }
    const cli = (...args) => execFileSync(join(bin, 'hubu'), ['--url', serverUrl, ...args],
      { cwd: directory, env, encoding: 'utf8' });
    cli('register', 'human', '--username', 'fixture-owner', '--display-name', 'Fixture owner');
    const agent = cli('register', 'agent', '--name', 'fixture-agent', '--version', 'browser-test');
    const agentId = agent.match(/^\s+agent_id:\s+(\S+)/m)[1];
    const accountId = agent.match(/^\s+account_id:\s+(\S+)/m)[1];
    const policy = join(directory, 'policy.yaml');
    writeFileSync(policy, 'id: fixture-policy\nversion: v1\ndefault_effect: needs_approval\nrules: []\n');
    cli('policy', 'add', '--path', policy);
    cli('budget', 'create', '--agent-id', agentId, '--amount', '20.00');
    const pending = async (name, executionScope) => {
      const result = await serverRequest('/spend/authorize', {
        operation_key: name, account_id: accountId, amount: '1.00', reason: name, execution_scope: executionScope,
      });
      assert.equal(result.decision, 'needs_approval');
      return result.approval.approval_request_id;
    };
    const approvedId = await pending('fixture-approved', {
      schema_version: 2,
      provider: 'provider:black-forest-labs:flux',
      executor: 'executor:gongbu:image',
      capability: 'capability:image:generate',
      billing_merchant: 'merchant:black-forest-labs',
      model: 'flux-2-klein-4b',
      image_size: '2k',
    });
    // Dummy capability values verify that the live portal sends only the owner bearer.
    env.HUBU_APPROVAL_TOKEN = 'retired-approval-fixture';
    env.HUBU_RECONCILIATION_TOKEN = 'retired-reconciliation-fixture';
    let nextApproval = 'slow';
    const portalHeaders = [];
    proxy = createServer({ allowHalfOpen: true }, (downstream) => {
      downstream.setEncoding('utf8');
      let request = '';
      downstream.on('data', (chunk) => { request += chunk; });
      downstream.on('end', async () => {
        if (request.startsWith('POST /spend/approval/resolve ')) {
          portalHeaders.push(request.split('\r\n\r\n')[0]);
          const mode = nextApproval;
          nextApproval = undefined;
          // Simulate transport latency across the real relay. The scripted
          // owner-presence provider itself answers immediately.
          if (mode === 'slow') await delay(31_500);
          // The scripted provider cannot hold a prompt open for a second
          // client. Inject that server refusal at the relay boundary.
          if (mode === 'busy') {
            const body = JSON.stringify({ error_code: 'owner_prompt_in_progress', approval_status: 'pending' });
            downstream.end(`HTTP/1.1 409 Conflict\r\nContent-Type: application/json\r\nContent-Length: ${body.length}\r\n\r\n${body}`);
            return;
          }
        }
        const upstream = connect(port, loopback, () => upstream.end(request));
        upstream.on('error', () => downstream.destroy());
        upstream.pipe(downstream);
      });
      downstream.on('error', () => downstream.destroy());
    });
    const proxyPort = await listen(proxy);
    portal = spawn(join(bin, 'hubu'), ['--url', `http://${loopback}:${proxyPort}`, 'ui', '--listen', `${loopback}:0`],
      { cwd: directory, env, stdio: ['ignore', 'pipe', 'inherit'] });
    const login = await new Promise((done, fail) => {
      let output = '';
      portal.stdout.on('data', (chunk) => {
        output += chunk;
        const match = output.match(/(http:\/\/[^\s]+\/login\?code=[^\s]+)\s/);
        if (match) done(match[1]);
      });
      portal.once('exit', (code) => fail(new Error(`Portal exited with status ${code}`)));
    });

    const approvalUrl = (id) => `${new URL(login).origin}/#approvals?request=${encodeURIComponent(id)}`;
    browser = await chromium.launch(process.env.HUBU_UI_CHROME ? { executablePath: process.env.HUBU_UI_CHROME } : {});
    const page = await browser.newPage();
    const errors = [];
    page.on('pageerror', (error) => errors.push(error.message));
    await page.goto(approvalUrl(approvedId));
    await page.getByRole('heading', { name: 'Sign in to Hubu Admin', exact: true }).waitFor();
    assert.equal(await page.getByText('fixture-approved', { exact: true }).count(), 0);
    await page.goto(login);
    await page.getByRole('link', { name: 'Budgets', exact: true }).click();
    await page.getByRole('heading', { name: 'Budgets', exact: true, level: 1 }).waitFor();
    await page.getByRole('link', { name: 'Settings', exact: true }).click();
    assert.equal(new URL(page.url()).pathname, '/settings');
    await page.getByRole('heading', { name: 'Agent action permissions' }).waitFor();
    assert.equal(await page.getByRole('navigation', { name: 'Sections' }).count(), 0);
    const strictTable = page.getByRole('table', { name: 'Allowed in Strict' });
    await strictTable.getByRole('columnheader', { name: 'Action', exact: true }).waitFor();
    await strictTable.getByRole('row', { name: /Register agents/ }).getByText('allowed', { exact: true }).waitFor();
    assert.equal(await strictTable.getByRole('button').count(), 0);
    const strict = page.getByText('No additional permissions granted.', { exact: true });
    await strict.waitFor();
    const add = page.getByRole('button', { name: 'Add permission…', exact: true });
    const warning = page.getByRole('dialog');
    const confirm = warning.getByRole('button', { name: 'I understand, add permission' });
    await add.click();
    assert.deepEqual(await warning.getByRole('option').allTextContents(), [
      'Choose a permission', 'Lower an existing budget', 'Append a deny rule',
    ]);
    assert.ok(await confirm.isDisabled());
    await warning.getByLabel('Permission', { exact: true }).selectOption('lower_budget');
    await warning.getByText('Agents can lower existing budgets without your confirmation.').waitFor();
    await warning.getByText(/A prompt-injected agent could cut spending limits/).waitFor();
    await warning.getByRole('button', { name: 'Close', exact: true }).click();
    assert.ok((await serverRequest('/governance/overrides')).overrides.every((setting) => !setting.enabled));
    await add.click();
    await warning.getByLabel('Permission', { exact: true }).selectOption('lower_budget');
    await confirm.click();
    const grantedRow = page.getByRole('table', { name: 'Additional permissions' })
      .getByRole('row', { name: /Lower an existing budget/ });
    await grantedRow.getByText('granted', { exact: true }).waitFor();
    const remove = grantedRow.getByRole('button', { name: 'Remove permission: Lower an existing budget' });
    await remove.waitFor();
    await add.click();
    assert.deepEqual(await warning.getByRole('option').allTextContents(), ['Choose a permission', 'Append a deny rule']);
    await warning.getByRole('button', { name: 'Close', exact: true }).click();
    await remove.click();
    await strict.waitFor();
    const overrideRecords = (await serverRequest('/governance/owner-presence')).records
      .filter((record) => record.action === 'governance.override.set');
    assert.equal(overrideRecords.length, 2);
    assert.ok(overrideRecords.every((record) => record.outcome === 'confirmed' && record.client === 'portal' && record.method === 'fixture'));
    assert.match(overrideRecords[0].reason, /^remove permission for agents to lower existing budgets$/);
    assert.match(overrideRecords[1].reason, /^allow agents to lower existing budgets without you/);

    await page.getByRole('link', { name: 'Back to console' }).click();
    await page.getByRole('heading', { name: 'Budgets', exact: true, level: 1 }).waitFor();
    await page.getByRole('link', { name: /^Approvals/ }).click();
    const approve = page.getByRole('button', { name: 'Approve', exact: true });
    const assertApproval = async (id, status) => {
      const approval = await serverRequest(`/spend/approval?approval_request_id=${id}`);
      assert.equal(approval.status, status);
    };
    const presence = async () => (await serverRequest('/governance/owner-presence')).records.filter((r) => r.client === 'portal');

    const approvalConfirmations = async () => (await presence()).filter((r) => r.action === 'spend.approval.approve');
    const reviewLink = page.getByRole('link', { name: 'Review', exact: true });
    const reviewHeading = page.getByRole('heading', { name: 'Spend approval', exact: true });
    const backToApprovals = page.getByText('Back to approvals', { exact: true });
    const expectReview = async () => {
      await reviewHeading.waitFor();
      await page.getByText(approvedId, { exact: true }).waitFor();
      await page.getByText('fixture-approved', { exact: true }).waitFor();
    };
    await reviewLink.click();
    await expectReview();
    await backToApprovals.click();
    await reviewLink.waitFor();

    await page.goto(approvalUrl(approvedId));
    await page.reload();
    await expectReview();
    await page.getByText('Hubu requests your approval for fixture-agent to spend up to 1.00 USD with Black Forest Labs FLUX.', { exact: true }).waitFor();
    await page.getByText('flux-2-klein-4b', { exact: true }).waitFor();
    await page.getByText('2k', { exact: true }).waitFor();
    await assertApproval(approvedId, 'pending');
    assert.equal(portalHeaders.length, 0);
    assert.equal((await approvalConfirmations()).length, 0);

    const waiting = page.getByText('Confirm on your Mac…', { exact: true });
    const startedAt = Date.now();
    await approve.click();
    await waiting.waitFor();
    await page.getByText('Spend approved. Budget reserved.', { exact: true }).waitFor({ timeout: 45_000 });
    assert.ok(Date.now() - startedAt > 30_000);
    await waiting.waitFor({ state: 'hidden' });
    await assertApproval(approvedId, 'approved');
    const confirmations = await approvalConfirmations();
    assert.equal(confirmations.length, 1);
    assert.equal(confirmations[0].outcome, 'confirmed');
    assert.equal(confirmations[0].method, 'fixture');
    assert.equal(
      confirmations[0].reason,
      'get your approval for fixture-agent to spend up to 1.00 USD on black-forest-labsflux/flux-2-klein-4b',
    );

    await page.goto(approvalUrl(approvedId));
    await expectReview();
    await page.getByText('This request is no longer pending.', { exact: true }).waitFor();
    assert.equal(await approve.count(), 0);
    assert.equal(await page.getByRole('button', { name: 'Deny', exact: true }).count(), 0);
    assert.equal(portalHeaders.length, 1);
    await backToApprovals.click();
    await page.getByText('No spend is waiting for approval.', { exact: true }).waitFor();

    await page.getByRole('link', { name: 'Budgets', exact: true }).click();
    for (const amount of ['21.00', '22.00']) {
      await page.getByRole('button', { name: 'Change limit', exact: true }).click();
      const dialog = page.getByRole('dialog');
      await dialog.getByLabel('New limit (USD)').fill(amount);
      await dialog.getByRole('button', { name: 'Save limit' }).click();
      await dialog.waitFor({ state: 'hidden' });
      await page.getByRole('cell', { name: amount + ' USD', exact: true }).first().waitFor();
    }
    assert.equal((await presence()).filter((r) => r.action === 'budget.version.create' && r.outcome === 'confirmed').length, 2);
    await page.getByRole('link', { name: /^Approvals/ }).click();

    const pendingId = await pending('fixture-refusals');
    await page.getByRole('button', { name: 'Refresh', exact: true }).click();
    const refuse = async (message, status = 'pending') => {
      await approve.click();
      await page.getByText(message).waitFor();
      await assertApproval(pendingId, status);
    };
    nextApproval = 'busy';
    await refuse(/Another owner confirmation is already open/);
    await refuse(/Confirmation timed out. The request is still pending/);
    await refuse(/Confirmation denied. This request is closed/, 'denied');
    await page.getByText('No spend is waiting for approval.').waitFor();

    const staleId = await pending('fixture-stale');
    await page.getByRole('button', { name: 'Refresh', exact: true }).click();
    await approve.waitFor();
    await serverRequest('/spend/approval/resolve', { approval_request_id: staleId, decision: 'deny' });
    await approve.click();
    await page.getByText(/This request has already been resolved/).waitFor();
    await page.getByText('No spend is waiting for approval.').waitFor();

    const limitedId = await pending('fixture-rate-limit');
    await page.getByRole('button', { name: 'Refresh', exact: true }).click();
    await approve.click();
    await page.getByText(/Owner confirmation is unavailable/).waitFor();
    for (let i = 0; i < 3; i++) {
      const response = page.waitForResponse((r) => r.request().method() === 'POST' && r.url().endsWith('/spend/approval/resolve'));
      await approve.click();
      assert.equal((await (await response).json()).error_code, 'owner_presence_timeout');
    }
    await approve.click();
    await page.getByText(/Too many recent confirmations were denied or timed out. Try again in \d+ seconds/).waitFor();
    await assertApproval(limitedId, 'pending');
    await page.getByRole('button', { name: 'Deny', exact: true }).click();
    await page.getByText('Spend denied.', { exact: true }).waitFor();
    await assertApproval(limitedId, 'denied');
    await page.getByRole('link', { name: 'Budgets', exact: true }).click();
    page.once('dialog', (dialog) => dialog.accept());
    await page.getByRole('button', { name: 'Revoke', exact: true }).click();
    await page.getByText('Budget revoked.', { exact: true }).waitFor();
    const records = await presence();
    assert.ok(records.some((r) => r.action === 'spend.approval.deny' && r.outcome === 'not_required'));
    assert.ok(records.some((r) => r.outcome === 'rate_limited'));
    assert.ok(records.some((r) => r.action === 'budget.revoke' && r.outcome === 'not_required'));
    for (const headers of portalHeaders) {
      assert.match(headers, /^Authorization: Bearer isolated-portal-owner-fixture$/im);
      assert.match(headers, /^X-Hubu-Client: portal$/im);
      assert.doesNotMatch(headers, /capability|X-Hubu-UI-Confirmed/i);
    }
    assert.deepEqual(errors, []);
    console.log('PASS portal approval review links, owner presence, override warnings, refusals, pending state, and >30-second wait');
  } finally {
    await browser?.close();
    await stop(portal);
    proxy?.close();
    await stop(server);
    rmSync(directory, { recursive: true, force: true });
  }
}

main().catch((error) => { console.error(error); process.exitCode = 1; });
