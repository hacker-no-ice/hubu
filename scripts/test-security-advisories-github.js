const assert = require('node:assert/strict');
const fs = require('node:fs');
const crypto = require('node:crypto');
const os = require('node:os');
const path = require('node:path');
const test = require('node:test');
const publish = require('./security-advisories-github.js');
const { FIX_BRANCH, ISSUE_MARKER } = publish.constants;
const VALIDATED_SHA = 'a'.repeat(40);

function report(overrides = {}) {
  return {
    schema_version: 1,
    status: 'fix_available',
    advisories: [{
      id: 'RUSTSEC-2099-0001', title: 'Example security advisory',
      package: 'vulnerable', version: '1.0.0', patched_versions: ['>=1.0.1'],
      dependency_paths: ['hubu-api → vulnerable 1.0.0'],
    }],
    updates: [{ package: 'vulnerable', from: '1.0.0', to: '1.0.1' }],
    errors: [],
    ...overrides,
  };
}

function harness(t, initialReport = report()) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'hubu-security-publisher-'));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const reportPath = path.join(directory, 'report.json');
  fs.writeFileSync(reportPath, JSON.stringify(initialReport));
  const state = { issues: [], pulls: [], writes: [], locks: new Map(), outputs: {}, runs: [], runReads: [], branchSha: VALIDATED_SHA, refReads: [] };
  const repoName = 'hacker-no-ice/hubu';
  let nextNumber = 1;
  function create(kind, args) {
    state.writes.push({ kind: `${kind}.create`, args });
    const item = {
      ...args, number: nextNumber++, state: 'open',
      assignees: (args.assignees || []).map(login => ({ login })),
    };
    item.html_url = `https://github.com/${repoName}/${kind === 'issues' ? 'issues' : 'pull'}/${item.number}`;
    if (kind === 'pulls') {
      item.head = { ref: args.head, sha: state.branchSha, repo: { full_name: repoName } };
      item.base = { ref: args.base };
    }
    state[kind].push(item);
    return { data: item };
  }
  function update(kind, args) {
    state.writes.push({ kind: `${kind}.update`, args });
    const item = state[kind].find(value => value.number === (args.issue_number || args.pull_number));
    Object.assign(item, args);
    if (args.assignees) item.assignees = args.assignees.map(login => ({ login }));
    return { data: item };
  }
  const github = {
    rest: {
      git: {
        getRef: async args => {
          state.refReads.push(args);
          return { data: { object: { sha: state.branchSha } } };
        },
      },
      actions: {
        createWorkflowDispatch: async args => {
          state.writes.push({ kind: 'actions.createWorkflowDispatch', args });
          const pull = state.pulls.find(item => item.head?.ref === args.ref);
          state.runs.push({ head_sha: pull.head.sha, event: 'workflow_dispatch', status: 'queued' });
          return { status: 204 };
        },
        listWorkflowRuns: async args => {
          state.runReads.push(args);
          return { data: { workflow_runs: state.runs } };
        },
      },
      issues: {
        listForRepo: async () => ({ data: state.issues }),
        create: async args => create('issues', args),
        update: async args => update('issues', args),
      },
      pulls: {
        list: async () => ({ data: state.pulls.filter(pull => pull.state === 'open') }),
        create: async args => create('pulls', args),
        update: async args => update('pulls', args),
      },
      repos: {
        getContent: async args => {
          assert.equal(args.path, 'Cargo.lock');
          const lock = state.locks.get(args.ref);
          if (lock instanceof Error) throw lock;
          if (!lock) throw new Error(`Unexpected lockfile read: ${args.ref}`);
          return { data: { encoding: 'base64', content: Buffer.from(lock).toString('base64') } };
        },
      },
    },
    paginate: async (method, args) => (await method(args)).data,
  };
  const context = {
    eventName: 'schedule', ref: 'refs/heads/main',
    repo: { owner: 'hacker-no-ice', repo: 'hubu' },
    payload: { repository: { default_branch: 'main', owner: { type: 'User' } } },
  };
  const core = { info: () => {}, setOutput: (key, value) => { state.outputs[key] = value; } };
  const env = {
    REPORT_PATH: reportPath, FIX_VALIDATION_STATUS: 'passed',
    FIX_CHANGED: 'true', FIX_BRANCH_PUSHED: 'true', FIX_HEAD_SHA: VALIDATED_SHA,
    EXPECTED_REPORT_SHA: crypto.createHash('sha256').update(fs.readFileSync(reportPath)).digest('hex'),
  };
  return {
    state, github, context, env,
    run: () => publish({ github, context, core }, env),
    setReport: value => {
      fs.writeFileSync(reportPath, JSON.stringify(value));
      env.EXPECTED_REPORT_SHA = crypto.createHash('sha256').update(fs.readFileSync(reportPath)).digest('hex');
    },
  };
}

test('compatible validated candidate produces one stable non-draft PR', async t => {
  const h = harness(t);
  await h.run();
  assert.equal(h.state.pulls.length, 1);
  const pull = h.state.pulls[0];
  assert.equal(pull.head.ref, FIX_BRANCH);
  assert.equal(pull.draft, false);
  assert.match(pull.body, /^🤖 \*\*Codex:\*\*/);
  assert.match(pull.body, /RUSTSEC-2099-0001/);
  assert.match(pull.body, /1\.0\.0.*→.*1\.0\.1/);
  assert.match(pull.body, /hubu-api → vulnerable 1\.0\.0/);
  assert.equal(h.state.issues.length, 0);
  await h.run();
  assert.deepEqual(h.state.writes.map(write => write.kind), ['pulls.create', 'actions.createWorkflowDispatch']);
  assert.deepEqual(h.state.writes[1].args, {
    owner: 'hacker-no-ice', repo: 'hubu', workflow_id: 'ci.yml', ref: FIX_BRANCH,
  });
  assert.ok(h.state.refReads.length >= 2);
  assert.ok(h.state.refReads.every(read => read.ref === `heads/${FIX_BRANCH}`));
});

test('an updated managed branch dispatches full CI once for the changed commit', async t => {
  const h = harness(t);
  await h.run();
  h.env.FIX_BRANCH_UPDATED = 'true';
  await h.run();
  assert.equal(h.state.writes.filter(write => write.kind === 'actions.createWorkflowDispatch').length, 2);
  h.env.FIX_BRANCH_UPDATED = 'false';
  await h.run();
  assert.equal(h.state.writes.filter(write => write.kind === 'actions.createWorkflowDispatch').length, 2);
  assert.equal(h.state.pulls.length, 1);
});

test('failed CI dispatch is escalated and still fails the publisher visibly', async t => {
  const h = harness(t);
  h.github.rest.actions.createWorkflowDispatch = async () => { throw new Error('workflow dispatch denied: 403'); };
  await assert.rejects(h.run(), /workflow dispatch denied/);
  assert.equal(h.state.pulls.length, 1);
  assert.equal(h.state.issues.length, 1);
  assert.match(h.state.issues[0].body, /workflow dispatch denied/);
});

test('a missing dispatch is recovered on the next unchanged run without repeating it', async t => {
  const h = harness(t);
  const dispatch = h.github.rest.actions.createWorkflowDispatch;
  h.github.rest.actions.createWorkflowDispatch = async () => { throw new Error('Transient dispatch failure'); };
  await assert.rejects(h.run(), /Transient dispatch failure/);
  h.github.rest.actions.createWorkflowDispatch = dispatch;
  h.env.FIX_BRANCH_UPDATED = 'false';
  await h.run();
  assert.equal(h.state.runs.length, 1);
  assert.equal(h.state.runReads[0].head_sha, VALIDATED_SHA);
  assert.equal(h.state.runReads[0].event, 'workflow_dispatch');
  assert.match(h.state.issues[0].body, /compatible remediation is ready/);
  await h.run();
  assert.equal(h.state.runs.length, 1);
  assert.equal(h.state.pulls.length, 1);
});

test('existing failed CI is not retried automatically', async t => {
  const h = harness(t);
  await h.run();
  h.state.runs[0].status = 'completed';
  h.state.runs[0].conclusion = 'failure';
  await h.run();
  assert.equal(h.state.writes.filter(write => write.kind === 'actions.createWorkflowDispatch').length, 1);
});

test('CI from another head or event does not suppress missing-dispatch recovery', async t => {
  const h = harness(t);
  await h.run();
  h.state.runs = [
    { head_sha: 'older-head', event: 'workflow_dispatch', status: 'completed' },
    { head_sha: VALIDATED_SHA, event: 'pull_request', status: 'queued' },
  ];
  await h.run();
  assert.equal(h.state.writes.filter(write => write.kind === 'actions.createWorkflowDispatch').length, 2);
});

for (const invalidSha of [undefined, '', 'short', 'g'.repeat(40)]) {
  test(`missing or malformed validated SHA (${String(invalidSha)}) refuses managed publication`, async t => {
    const h = harness(t);
    h.env.FIX_HEAD_SHA = invalidSha;
    await assert.rejects(h.run(), /FIX_HEAD_SHA/);
    assert.equal(h.state.pulls.length, 0);
    assert.equal(h.state.issues.length, 1);
    assert.equal(h.state.writes.filter(write => write.kind === 'actions.createWorkflowDispatch').length, 0);
  });
}

test('a branch moved before publication cannot create a PR or dispatch CI', async t => {
  const h = harness(t);
  h.state.branchSha = 'b'.repeat(40);
  await assert.rejects(h.run(), /branch moved after validation/);
  assert.equal(h.state.pulls.length, 0);
  assert.equal(h.state.issues.length, 1);
});

test('a stale managed PR head cannot be reused or dispatched', async t => {
  const h = harness(t);
  await h.run();
  h.state.pulls[0].head.sha = 'b'.repeat(40);
  h.env.FIX_BRANCH_UPDATED = 'true';
  await assert.rejects(h.run(), /PR head differs/);
  assert.equal(h.state.writes.filter(write => write.kind === 'actions.createWorkflowDispatch').length, 1);
  assert.equal(h.state.issues.length, 1);
});

test('ref drift immediately before dispatch fails visibly and does not execute CI', async t => {
  const h = harness(t);
  const create = h.github.rest.pulls.create;
  h.github.rest.pulls.create = async args => {
    const response = await create(args);
    h.state.branchSha = 'b'.repeat(40);
    return response;
  };
  await assert.rejects(h.run(), /branch moved after validation/);
  assert.equal(h.state.pulls.length, 1);
  assert.equal(h.state.writes.filter(write => write.kind === 'actions.createWorkflowDispatch').length, 0);
  assert.equal(h.state.issues.length, 1);
});

test('blocked advisory opens one assigned issue and repeat runs do not notify', async t => {
  const h = harness(t, report({ status: 'blocked', updates: [], message: 'No compatible patched version.' }));
  h.env.SECURITY_MAINTAINER = '@security-owner';
  await h.run();
  const issue = h.state.issues[0];
  assert.deepEqual(issue.assignees, [{ login: 'security-owner' }]);
  assert.match(issue.body, /^🤖 \*\*Codex:\*\*/);
  assert.match(issue.body, /No compatible patched version/);
  assert.match(issue.body, /wait for an upstream fix/);
  assert.match(issue.body, /major upgrade or replacement/);
  assert.match(issue.body, /timeboxed exception with an owner and removal condition/);
  assert.match(issue.body, /repository-security\.md#advisory-and-license-exceptions/);
  await h.run();
  assert.deepEqual(h.state.writes.map(write => write.kind), ['issues.create']);
  assert.equal(h.state.pulls.length, 0);
});

test('only healthy main closes tracking; returning incidents reopen the same issue', async t => {
  const h = harness(t, report({ status: 'advisories', updates: [] }));
  await h.run();
  h.setReport(report());
  await h.run();
  assert.equal(h.state.issues[0].state, 'open');
  assert.match(h.state.issues[0].body, /compatible remediation is ready/);
  h.setReport(report({ status: 'healthy', advisories: [], updates: [] }));
  await h.run();
  assert.equal(h.state.issues[0].state, 'closed');
  const count = h.state.writes.length;
  await h.run();
  assert.equal(h.state.writes.length, count);
  h.setReport(report({ status: 'blocked', updates: [] }));
  await h.run();
  assert.equal(h.state.issues.length, 1);
  assert.equal(h.state.issues[0].state, 'open');
});

test('healthy first run makes no writes', async t => {
  const h = harness(t, report({ status: 'healthy', advisories: [], updates: [] }));
  await h.run();
  assert.deepEqual(h.state.writes, []);
});

for (const [name, changes] of [
  ['validation failure', { FIX_VALIDATION_STATUS: 'failed' }],
  ['interrupted validation', { FIX_VALIDATION_STATUS: 'not_run' }],
  ['zero change candidate', { FIX_CHANGED: 'false' }],
  ['push failure', { FIX_BRANCH_PUSHED: 'false' }],
]) {
  test(`${name} creates an escalation issue without a PR`, async t => {
    const h = harness(t);
    Object.assign(h.env, changes);
    await h.run();
    assert.equal(h.state.pulls.length, 0);
    assert.equal(h.state.issues.length, 1);
    assert.equal(h.state.issues[0].assignees[0].login, 'hacker-no-ice');
  });
}

test('infrastructure failure is owned and unchanged repeat does not create noise', async t => {
  const h = harness(t, report({
    status: 'error', advisories: [], updates: [], errors: ['Advisory database fetch failed'],
  }));
  await h.run();
  await h.run();
  assert.equal(h.state.issues.length, 1);
  assert.match(h.state.issues[0].body, /Advisory database fetch failed/);
  assert.equal(h.state.writes.length, 1);
});

test('partial candidate errors never publish a fix even when validation claims success', async t => {
  const h = harness(t, report({ errors: ['Candidate generation timed out'] }));
  await h.run();
  assert.equal(h.state.pulls.length, 0);
  assert.equal(h.state.issues.length, 1);
  assert.match(h.state.issues[0].body, /Candidate generation timed out/);
});

test('compatible Dependabot PR is reused by exact lockfile versions without mutating it', async t => {
  const h = harness(t);
  h.state.pulls.push({
    number: 24, state: 'open', title: 'Bump vulnerable', body: 'Dependabot body without a RustSec ID',
    html_url: 'https://github.com/hacker-no-ice/hubu/pull/24', user: { login: 'dependabot[bot]' },
    base: { ref: 'main' }, head: { ref: 'dependabot/cargo/vulnerable-1.0.1', sha: 'dependabot-head', repo: { full_name: 'hacker-no-ice/hubu' } },
  });
  h.state.locks.set('dependabot-head', 'version = 4\n[[package]]\nname = "vulnerable"\nversion = "1.0.1"\n');
  await h.run();
  assert.equal(h.state.outputs.pull_request, 'https://github.com/hacker-no-ice/hubu/pull/24');
  assert.deepEqual(h.state.writes, []);
});

test('a managed duplicate is closed once when Dependabot provides the compatible fix', async t => {
  const h = harness(t);
  await h.run();
  const managed = h.state.pulls[0];
  h.state.pulls.push({
    number: 24, state: 'open', user: { login: 'dependabot[bot]' },
    html_url: 'https://github.com/hacker-no-ice/hubu/pull/24', base: { ref: 'main' },
    head: { sha: 'dependabot-head', repo: { full_name: 'hacker-no-ice/hubu' } },
  });
  h.state.locks.set('dependabot-head', '[[package]]\nname = "vulnerable"\nversion = "1.0.1"\n');
  await h.run();
  assert.equal(managed.state, 'closed');
  const count = h.state.writes.length;
  await h.run();
  assert.equal(h.state.writes.length, count);
  assert.equal(h.state.pulls.length, 2);
});

test('Dependabot PR retaining a vulnerable version cannot suppress remediation', async t => {
  const h = harness(t);
  h.state.pulls.push({
    number: 24, state: 'open', user: { login: 'dependabot[bot]' }, base: { ref: 'main' },
    head: { sha: 'dependabot-head', repo: { full_name: 'hacker-no-ice/hubu' } },
  });
  h.state.locks.set('dependabot-head', '[[package]]\nname = "vulnerable"\nversion = "1.0.1"\n[[package]]\nname = "vulnerable"\nversion = "1.0.0"\n');
  await h.run();
  assert.equal(h.state.writes[0].kind, 'pulls.create');
});

test('foreign or non-default-branch PR data is never fetched', async t => {
  const h = harness(t);
  h.state.pulls.push({
    number: 24, state: 'open', user: { login: 'dependabot[bot]' }, base: { ref: 'main' },
    head: { sha: 'foreign', repo: { full_name: 'attacker/hubu' } },
  }, {
    number: 25, state: 'open', user: { login: 'dependabot[bot]' }, base: { ref: 'other' },
    head: { sha: 'other', repo: { full_name: 'hacker-no-ice/hubu' } },
  });
  await h.run();
  assert.equal(h.state.writes[0].kind, 'pulls.create');
});

test('GitHub API errors remain visible and owned without creating a duplicate PR', async t => {
  const h = harness(t);
  h.state.pulls.push({
    number: 24, state: 'open', user: { login: 'dependabot[bot]' }, base: { ref: 'main' },
    head: { sha: 'unreadable', repo: { full_name: 'hacker-no-ice/hubu' } },
  });
  h.state.locks.set('unreadable', new Error('GitHub API 503'));
  await assert.rejects(h.run(), /GitHub API 503/);
  assert.deepEqual(h.state.writes.map(write => write.kind), ['issues.create']);
  assert.equal(h.state.issues[0].assignees[0].login, 'hacker-no-ice');
  assert.match(h.state.issues[0].body, /GitHub remediation publication failed/);
});

test('issue assignment errors are not swallowed', async t => {
  const h = harness(t, report({ status: 'blocked', updates: [] }));
  h.github.rest.issues.create = async () => { throw new Error('assignee is not assignable: 422'); };
  await assert.rejects(h.run(), /422/);
});

test('a previously unassigned tracking issue is assigned without a new issue', async t => {
  const h = harness(t, report({ status: 'blocked', updates: [] }));
  h.state.issues.push({ number: 10, title: 'Existing tracker', body: ISSUE_MARKER, assignees: [], state: 'open' });
  await h.run();
  assert.equal(h.state.issues.length, 1);
  assert.deepEqual(h.state.issues[0].assignees, [{ login: 'hacker-no-ice' }]);
});

test('untrusted refs and events cannot publish', async t => {
  const h = harness(t);
  h.context.eventName = 'pull_request';
  await assert.rejects(h.run(), /default branch/);
  h.context.eventName = 'workflow_dispatch';
  h.context.ref = 'refs/heads/feature';
  await assert.rejects(h.run(), /default branch/);
  assert.deepEqual(h.state.writes, []);
});

test('malformed or contradictory reports cannot close existing incidents', async t => {
  const h = harness(t, report({ status: 'healthy' }));
  h.state.issues.push({ number: 10, body: ISSUE_MARKER, state: 'open' });
  await assert.rejects(h.run(), /contains advisories/);
  h.setReport({ status: 'healthy' });
  await assert.rejects(h.run(), /Invalid security report schema/);
  assert.equal(h.state.issues[0].state, 'open');
  assert.equal(h.state.writes.length, 1);
  assert.equal(h.state.writes[0].kind, 'issues.update');
  assert.match(h.state.issues[0].body, /Trusted security scan evidence is unavailable or invalid/);
});

test('a healthy report with failed validation cannot close an incident', async t => {
  const h = harness(t, report({ status: 'healthy', advisories: [], updates: [] }));
  h.state.issues.push({ number: 10, body: ISSUE_MARKER, state: 'open' });
  h.env.FIX_VALIDATION_STATUS = 'failed';
  await assert.rejects(h.run(), /Failed validation cannot establish a healthy/);
  assert.equal(h.state.issues[0].state, 'open');
  assert.equal(h.state.issues[0].assignees[0].login, 'hacker-no-ice');
});

test('healthy main with no candidate validation can close an incident', async t => {
  const h = harness(t, report({ status: 'healthy', advisories: [], updates: [] }));
  h.state.issues.push({ number: 10, body: ISSUE_MARKER, state: 'open' });
  h.env.FIX_VALIDATION_STATUS = 'not_run';
  await h.run();
  assert.equal(h.state.issues[0].state, 'closed');
});

for (const invalidDigest of [undefined, '', 'short', 'a'.repeat(64)]) {
  test(`missing, malformed, or mismatched report digest (${String(invalidDigest)}) is owned and visible`, async t => {
    const h = harness(t, report({ status: 'healthy', advisories: [], updates: [] }));
    h.env.EXPECTED_REPORT_SHA = invalidDigest;
    await assert.rejects(h.run(), /trusted pre-build SHA-256 digest/);
    assert.equal(h.state.issues.length, 1);
    assert.equal(h.state.issues[0].state, 'open');
    assert.equal(h.state.pulls.length, 0);
    assert.match(h.state.issues[0].body, /Trusted scan report authentication or validation failed/);
  });
}

test('a tampered healthy report is never trusted or published', async t => {
  const h = harness(t);
  h.state.issues.push({ number: 10, body: ISSUE_MARKER, state: 'open' });
  fs.writeFileSync(h.env.REPORT_PATH, JSON.stringify(report({
    status: 'healthy', advisories: [], updates: [], message: 'ATTACKER-CONTROLLED CONTENT',
  })));
  h.env.FIX_VALIDATION_STATUS = 'failed';
  await assert.rejects(h.run(), /trusted pre-build SHA-256 digest/);
  assert.equal(h.state.issues[0].state, 'open');
  assert.doesNotMatch(h.state.issues[0].body, /ATTACKER-CONTROLLED CONTENT/);
});

test('authenticated invalid JSON creates a canonical incident before failing visibly', async t => {
  const h = harness(t);
  fs.writeFileSync(h.env.REPORT_PATH, '{"untrusted": "ATTACKER-CONTROLLED CONTENT"');
  h.env.EXPECTED_REPORT_SHA = crypto.createHash('sha256').update(fs.readFileSync(h.env.REPORT_PATH)).digest('hex');
  await assert.rejects(h.run(), SyntaxError);
  assert.equal(h.state.issues.length, 1);
  assert.equal(h.state.issues[0].assignees[0].login, 'hacker-no-ice');
  assert.doesNotMatch(h.state.issues[0].body, /ATTACKER-CONTROLLED CONTENT/);
});
