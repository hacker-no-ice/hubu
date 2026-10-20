// Metadata-only publisher invoked by actions/github-script after trusted-main scans.
// It never checks out or executes pull-request content.
const fs = require('node:fs');
const crypto = require('node:crypto');

const FIX_BRANCH = 'codex/security-advisories';
const ISSUE_MARKER = '<!-- hubu-security-advisories:tracker:v1 -->';
const PR_MARKER = '<!-- hubu-security-advisories:fix:v1 -->';
const PREFIX = '🤖 **Codex:**';
const ISSUE_TITLE = 'Security: remediate advisories on main';

function plain(value) {
  return String(value ?? '').replace(/[\u0000-\u001f\u007f]/g, ' ').replace(/@/g, '&#64;');
}

function code(value) {
  return `\`${plain(value).replace(/`/g, "'")}\``;
}

function markdown(value) {
  return plain(value).replace(/[\\`*_{}\[\]<>]/g, '\\$&');
}

function validateReport(report) {
  const statuses = ['healthy', 'advisories', 'fix_available', 'blocked', 'error'];
  if (report.schema_version !== 1 || !statuses.includes(report.status)
      || !Array.isArray(report.advisories) || !Array.isArray(report.updates)
      || !Array.isArray(report.errors)) {
    throw new Error('Invalid security report schema; refusing to publish or close tracking issues');
  }
  for (const advisory of report.advisories) {
    if (!advisory.id || !advisory.package || !advisory.version
        || !Array.isArray(advisory.dependency_paths)) {
      throw new Error('Invalid security advisory record');
    }
  }
  for (const update of report.updates) {
    if (!update.package || !update.from || !update.to || update.from === update.to) {
      throw new Error('Invalid candidate dependency update');
    }
  }
  if (report.status === 'healthy' && (report.advisories.length || report.errors.length)) {
    throw new Error('Healthy security report contains advisories or errors');
  }
  return report;
}

function renderAdvisories(report) {
  if (!report.advisories.length) return 'No advisory details are available.';
  return report.advisories.map(advisory => {
    const patches = Array.isArray(advisory.patched_versions)
      ? advisory.patched_versions.join(', ') : advisory.patched_versions;
    const paths = advisory.dependency_paths.map(path => (
      `  - ${code(Array.isArray(path) ? path.join(' → ') : path)}`
    )).join('\n');
    return [
      `- ${code(advisory.id)} — ${code(advisory.package)} ${code(advisory.version)}: ${markdown(advisory.title)}`,
      `  - Patched versions: ${code(patches || 'none published')}`,
      paths ? `  - Dependency paths:\n${paths.replace(/^  /gm, '    ')}` : '  - Dependency paths: unavailable',
    ].join('\n');
  }).join('\n');
}

function renderUpdates(report) {
  return report.updates.map(update => (
    `- ${code(update.package)}: ${code(update.from)} → ${code(update.to)}`
  )).join('\n') || 'No compatible dependency update was produced.';
}

function validationReason(report, env) {
  if (report.errors.length) return report.message || 'The scan or remediation reported infrastructure errors.';
  if (report.status !== 'fix_available') {
    return report.message || (report.status === 'error'
      ? 'The trusted-main scan or remediation infrastructure failed.'
      : 'A compatible, validated fix is not available.');
  }
  if (!report.advisories.length) return 'The candidate report contains no advisory details; manual investigation is required.';
  if (!report.updates.length || env.FIX_CHANGED !== 'true') {
    return 'The candidate did not change dependencies; a remediation PR was not created.';
  }
  if (env.FIX_VALIDATION_STATUS !== 'passed') {
    return `Candidate validation ${env.FIX_VALIDATION_STATUS === 'failed' ? 'failed' : 'did not complete'}.`;
  }
  if (env.FIX_BRANCH_PUSHED !== 'true') return 'The validated candidate branch could not be published.';
  return null;
}

function issueBody(report, reason, workflowUrl, runbookUrl, pull) {
  return [
    PREFIX,
    ISSUE_MARKER,
    '',
    pull ? `A compatible remediation is ready for human review: ${pull.html_url}` : markdown(reason),
    '',
    renderAdvisories(report),
    '',
    'Candidate dependency updates:',
    renderUpdates(report),
    ...(report.errors.length ? ['', 'Failures:', ...report.errors.map(error => `- ${code(error)}`)] : []),
    '',
    `Inspect the [scheduled security workflow](${workflowUrl}) for scan and validation logs.`,
    'This issue remains open until the default branch passes the advisory scan.',
    'Remediation must preserve Rust 1.88 compatibility and pass workspace tests, Clippy, security policy, and production builds. Advisory ignores and policy exceptions require human review.',
    ...(!pull ? [
      `The assigned maintainer should choose whether to wait for an upstream fix, review a major upgrade or replacement, or request a timeboxed exception with an owner and removal condition. Follow the [exception review requirements](${runbookUrl}).`,
    ] : []),
  ].join('\n');
}

function pullBody(report, workflowUrl) {
  return [
    PREFIX,
    PR_MARKER,
    '',
    'The default branch contains security advisories. These targeted compatible dependency updates passed the scheduled remediation checks.',
    '',
    renderAdvisories(report),
    '',
    'Dependency updates:',
    renderUpdates(report),
    '',
    `Validation: Rust 1.88 workspace check, workspace tests, Clippy, advisory/license/source policy checks, and production builds. See the [security workflow](${workflowUrl}).`,
    'Human review and merge are required. No advisory ignores or policy exceptions were added.',
  ].join('\n');
}

function lockVersions(text) {
  const versions = new Map();
  let current = {};
  function save() {
    if (current.name && current.version) {
      const entries = versions.get(current.name) || new Set();
      entries.add(current.version);
      versions.set(current.name, entries);
    }
  }
  for (const line of text.split(/\r?\n/)) {
    if (line.trim() === '[[package]]') {
      save();
      current = {};
    } else {
      const match = line.match(/^(name|version)\s*=\s*"([^"\\]+)"\s*$/);
      if (match) current[match[1]] = match[2];
    }
  }
  save();
  return versions;
}

async function findDependabotFix({ github, repo, pulls, report, defaultBranch }) {
  for (const pull of pulls) {
    if (!['dependabot[bot]', 'dependabot-preview[bot]'].includes(pull.user?.login)
        || pull.base?.ref !== defaultBranch
        || pull.head?.repo?.full_name?.toLowerCase() !== `${repo.owner}/${repo.repo}`.toLowerCase()) continue;
    // Read data at the immutable PR SHA. Never check out or execute the PR.
    const { data } = await github.rest.repos.getContent({
      ...repo, path: 'Cargo.lock', ref: pull.head.sha,
    });
    if (Array.isArray(data) || data.encoding !== 'base64' || !data.content) {
      throw new Error(`Cannot inspect Cargo.lock for Dependabot PR #${pull.number}; duplicate suppression is unresolved`);
    }
    const versions = lockVersions(Buffer.from(data.content, 'base64').toString('utf8'));
    if (report.updates.every(update => (
      versions.get(update.package)?.has(update.to) && !versions.get(update.package)?.has(update.from)
    ))) return pull;
  }
  return null;
}

async function verifyValidatedRef(github, repo, expectedSha, pull) {
  if (!/^[a-f\d]{40}$/i.test(expectedSha || '')) {
    throw new Error('FIX_HEAD_SHA must identify the validated remediation commit with a full 40-character SHA');
  }
  const { data } = await github.rest.git.getRef({ ...repo, ref: `heads/${FIX_BRANCH}` });
  if (data.object?.sha !== expectedSha) {
    throw new Error('The remediation branch moved after validation; refusing managed PR changes and CI dispatch');
  }
  if (pull && pull.head?.sha !== expectedSha) {
    throw new Error('The managed remediation PR head differs from the validated commit; refusing changes and CI dispatch');
  }
}

function securityMaintainer(context, env) {
  const fallback = context.payload.repository.owner?.type === 'User' ? context.repo.owner : 'hacker-no-ice';
  const maintainer = (env.SECURITY_MAINTAINER || fallback).replace(/^@/, '');
  if (!/^[a-z\d](?:[a-z\d-]{0,37}[a-z\d])?$/i.test(maintainer)) {
    throw new Error('SECURITY_MAINTAINER must be one GitHub username');
  }
  return maintainer;
}

async function publish({ github, context, core }, env = process.env) {
  const defaultBranch = context.payload.repository?.default_branch;
  if (!['schedule', 'workflow_dispatch'].includes(context.eventName)
      || !defaultBranch || context.ref !== `refs/heads/${defaultBranch}`) {
    throw new Error('Security publishing accepts only scheduled or manual runs from the default branch');
  }
  let report;
  let reportError;
  try {
    const bytes = fs.readFileSync(env.REPORT_PATH);
    if (!/^[a-f\d]{64}$/i.test(env.EXPECTED_REPORT_SHA || '')
        || crypto.createHash('sha256').update(bytes).digest('hex') !== env.EXPECTED_REPORT_SHA) {
      throw new Error('Security report does not match the trusted pre-build SHA-256 digest');
    }
    report = validateReport(JSON.parse(bytes.toString('utf8')));
    if (report.status === 'healthy' && env.FIX_VALIDATION_STATUS === 'failed') {
      throw new Error('Failed validation cannot establish a healthy default branch');
    }
  } catch (error) {
    reportError = error;
    // Never publish fields from evidence that failed authentication or parsing.
    report = {
      schema_version: 1, status: 'error', advisories: [], updates: [],
      errors: ['Trusted scan report authentication or validation failed.'],
      message: 'Trusted security scan evidence is unavailable or invalid; manual investigation is required.',
    };
  }
  const repo = context.repo;
  const workflowUrl = `https://github.com/${repo.owner}/${repo.repo}/actions/workflows/security-advisories.yml`;
  const runbookUrl = `https://github.com/${repo.owner}/${repo.repo}/blob/${encodeURIComponent(defaultBranch)}/docs/operations/repository-security.md#advisory-and-license-exceptions`;
  const issues = await github.paginate(github.rest.issues.listForRepo, {
    ...repo, state: 'all', sort: 'created', direction: 'desc', per_page: 100,
  });
  const issue = issues.find(item => !item.pull_request && item.body?.includes(ISSUE_MARKER));

  if (report.status === 'healthy') {
    if (issue?.state === 'open') {
      await github.rest.issues.update({ ...repo, issue_number: issue.number, state: 'closed', state_reason: 'completed' });
    }
    core.info('The default branch is healthy; no remediation is required.');
    core.setOutput('result', 'healthy');
    return;
  }

  let reason = validationReason(report, env);
  let pull;
  let publicationError = reportError;
  if (!reason) {
    try {
      let dispatchCi = false;
      let managedPublication = false;
      const pulls = await github.paginate(github.rest.pulls.list, {
        ...repo, state: 'open', base: defaultBranch, per_page: 100,
      });
      const managed = pulls.find(item => item.head?.ref === FIX_BRANCH
        && item.head?.repo?.full_name?.toLowerCase() === `${repo.owner}/${repo.repo}`.toLowerCase());
      // Reuse Dependabot's compatible fix and retire our duplicate if it exists.
      pull = await findDependabotFix({ github, repo, pulls, report, defaultBranch });
      if (pull) {
        if (managed?.body?.includes(PR_MARKER)) {
          await verifyValidatedRef(github, repo, env.FIX_HEAD_SHA, managed);
          await github.rest.pulls.update({ ...repo, pull_number: managed.number, state: 'closed' });
        }
      } else {
        managedPublication = true;
        pull = managed;
        await verifyValidatedRef(github, repo, env.FIX_HEAD_SHA, pull);
        dispatchCi = env.FIX_BRANCH_UPDATED === 'true';
        const ids = [...new Set(report.advisories.map(advisory => advisory.id))].sort().join(', ');
        const title = `fix(security): remediate ${ids || 'dependency advisories'}`.slice(0, 240);
        const body = pullBody(report, workflowUrl);
        if (pull) {
          if (pull.title !== title || pull.body !== body) {
            const response = await github.rest.pulls.update({ ...repo, pull_number: pull.number, title, body });
            pull = response.data;
          }
          if (!dispatchCi) {
            const { data } = await github.rest.actions.listWorkflowRuns({
              ...repo, workflow_id: 'ci.yml', branch: FIX_BRANCH,
              event: 'workflow_dispatch', head_sha: env.FIX_HEAD_SHA, per_page: 100,
            });
            dispatchCi = !data.workflow_runs.some(run => (
              run.head_sha === env.FIX_HEAD_SHA && run.event === 'workflow_dispatch'
            ));
          }
        } else {
          const response = await github.rest.pulls.create({
            ...repo, head: FIX_BRANCH, base: defaultBranch, title, body, draft: false,
          });
          pull = response.data;
          dispatchCi = true;
        }
        if (!pull.assignees?.length) {
          await verifyValidatedRef(github, repo, env.FIX_HEAD_SHA, pull);
          await github.rest.issues.addAssignees({
            ...repo, issue_number: pull.number, assignees: [securityMaintainer(context, env)],
          });
        }
      }
      // GITHUB_TOKEN-created PR checks can require human workflow approval.
      // Explicit dispatch runs read-only CI and recovers a missing dispatch;
      // an existing run, including failed CI, is left for human review.
      if (dispatchCi) {
        await verifyValidatedRef(github, repo, env.FIX_HEAD_SHA, pull);
        await github.rest.actions.createWorkflowDispatch({
          ...repo, workflow_id: 'ci.yml', ref: FIX_BRANCH,
        });
        core.setOutput('ci_dispatched', 'true');
      } else if (managedPublication) {
        await verifyValidatedRef(github, repo, env.FIX_HEAD_SHA, pull);
      }
      core.setOutput('pull_request', pull.html_url);
    } catch (error) {
      // Keep infrastructure failures owned when issue writes remain available,
      // while still failing the action visibly after publishing the incident.
      publicationError = error;
      reason = `GitHub remediation publication failed: ${error.message}`;
      pull = null;
    }
  }

  if (reason || issue) {
    const body = issueBody(report, reason, workflowUrl, runbookUrl, pull);
    const maintainer = securityMaintainer(context, env);
    if (!issue) {
      const response = await github.rest.issues.create({
        ...repo, title: ISSUE_TITLE, body, assignees: [maintainer],
      });
      core.setOutput('tracking_issue', response.data.html_url);
    } else {
      const changes = {};
      if (issue.title !== ISSUE_TITLE) changes.title = ISSUE_TITLE;
      if (issue.body !== body) changes.body = body;
      if (issue.state !== 'open') changes.state = 'open';
      if (!issue.assignees?.length) changes.assignees = [maintainer];
      if (Object.keys(changes).length) {
        await github.rest.issues.update({ ...repo, issue_number: issue.number, ...changes });
      }
      core.setOutput('tracking_issue', issue.html_url);
    }
  }
  core.setOutput('result', pull ? 'pull_request' : 'tracking_issue');
  if (publicationError) throw publicationError;
}

module.exports = publish;
module.exports.constants = { FIX_BRANCH, ISSUE_MARKER, PR_MARKER };
