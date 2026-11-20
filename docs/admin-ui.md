# Admin web UI

`hubu ui` runs a local web UI for Hubu administration. It does what the CLI
admin commands do, in a browser, and adds views of spend history and
decisions.

```bash
hubu ui
```

The command prints a one-time login link:

```text
Hubu admin UI is running
  listening: http://127.0.0.1:8790
  hubu: http://127.0.0.1:8787
  open this one-time login link in your browser:
    http://127.0.0.1:8790/login?code=…
```

Open the link in your browser. It works once. To sign in from another
browser, stop `hubu ui` and start it again. Sign-in and reads use the owner
bearer without a Touch ID dialog.

## What you can do

| Section | Actions |
| --- | --- |
| Decisions | Immutable decision history plus current reconciliation state. Outstanding expired claims have a review/action flow; resolved events retain their history and show outcome, time, evidence, provider reference, recorded amount, cost basis and resolver when available. |
| Overview | Three groups: people and agents; the spend lifecycle (pending approvals, automatic denials, unresolved reconciliations, recent decisions); and policies and budgets (spend against limits, budgets near their limit, policies, spending targets). A non-zero approval or denial count is highlighted, and every tile opens the matching page, pre-filtered where it helps. |
| Approvals | Review pending spend requests, then approve or deny them. |
| Spend history | A spend summary with charts: total spent, the split by platform (billing merchant) and by agent, and spend per day over the last 7, 30, 90 or 365 days, filtered by agent. Each currency is totaled and charted separately, and the daily chart has a table view. Below it, authorization records filtered by agent and status, with details, and ledger transactions with their entries. |
| Decisions | Governance records filtered by agent, category and time, with the summary for the same filter. |
| Agents | Register an agent from a name and a version label, list agents, rename an agent from the pencil next to its name, view its name history. |
| Users & targets | List human users, register a human user, set or revoke advisory spending targets. |
| Policies | Read any revision of a policy (rules in plain language and the YAML source), compare a past revision with the current one, then switch to Edit to change the YAML, validate it and save it as a new revision. |
| Budgets | Create budgets, change a limit, revoke, view version history. |
| Providers & secrets | Read-only view grouped by platform: each platform's models, config versions, secret reference name and which targets Gongbu currently offers. |

The spend summary comes from the read-only
[`GET /ledger/summary`](ledger-history.md#spend-summary) endpoint, so totals
cover the whole ledger rather than the loaded table rows. Charts are plain CSS
bars; hover a bar for its exact amount.

**Overview** and **Spend history** reload themselves every 15 seconds while
the browser tab is visible, keeping your scroll position. Auto-refresh pauses
while a dialog is open and, once you change a filter, load more rows or expand
a section, until you reload the view with **Refresh** or navigate away. Turn it
off with the **Auto-refresh** checkbox; the choice is remembered in that
browser. Views with editors (agents, users, policies, budgets) and the other
views reload only when you click **Refresh**.

Agent registration follows the [registration protocol](agent-registration.md).
You enter the agent name and version label. The UI fills owner and runtime
fields from the active Hubu session, computes the fingerprints, and shows the
review before you submit. The server recomputes the fingerprints.

In the policy editor, **Validate** checks the YAML locally with the same
loader and validator the Hubu server uses, then lists the rules as they will
be read. Saving is enabled only for the exact text that passed validation, and
it is pinned to the revision you started from, so a concurrent change is
rejected instead of overwritten.

Changing a budget limit is pinned to the revision you saw. If someone else
changed the budget first, the server rejects the change.

## Confirming owner actions

When you approve spend, create or change a budget, save a policy, rename an
agent, set a spending target, initialize Hubu or reconcile a claim, the portal
shows **Confirm on your Mac…** while `hubu-server` waits for the Touch ID
dialog. Each action has its own dialog and may wait up to 60 seconds.
**Deny**, **Revoke budget** and **Revoke spending target** take effect without
Touch ID.

On **Approvals**, each pending row has **Review**, **Approve** and **Deny**.
Approving reserves budget for the agent and clears the request from the pending
list. Pressing **Deny** in the Touch ID dialog also closes the request. The
agent can submit a new spend request if needed. If the dialog times out, the
request stays pending so you can approve it again when you’re at your Mac.

If another dialog is open, finish it on your Mac before retrying. If owner
confirmation is unavailable, check that `hubu-server` runs in your Mac’s
signed-in desktop session. After five denied or timed-out prompts in ten
minutes, wait for the retry time shown by the portal. If another client has
already resolved the spend request, the portal refreshes the pending list.

## Connection

`hubu ui` connects to Hubu the same way other CLI commands do: the selected
stack profile's active handoff, or `--url` with the legacy credential
environment variables or files. `hubu --help` lists the connection
precedence, and the [CLI administration reference](cli.md) covers the
equivalent terminal commands.

For the live Gongbu target view, the UI uses the profile's Gongbu endpoint and
caller token. Without a profile, pass both:

```bash
hubu ui --gongbu-endpoint http://127.0.0.1:GONGBU_PORT --gongbu-token-file /path/to/gongbu-caller-token
```

`--listen 127.0.0.1:PORT` changes the address. Port `0` picks a free port.
Only loopback addresses are accepted.

## Providers and secret references

Provider revisions and secret references are Gongbu configuration. Your stack
profile owns them in `providers.toml` and `credentials.toml`, and each render
freezes them into an immutable generation. The UI reads the active
generation's provider revisions after checking the manifest digest. It shows
each revision's provider, model, workload, config version, selectable and
execution state, pack identity, and the `service`/`account` names of its
secret reference.

The UI never reads or shows secret values or adapter transport settings. To
add or remove a provider, edit the profile and run `hubu stack render`, then
activate the new generation. See
[managing a local stack](operations/managing-a-stack.md). Adding and removing
providers from the UI is planned.

## Security model

The Hubu server refuses browser-origin requests on purpose, and that does not
change. The browser talks only to the `hubu ui` process, which works like
this:

- It listens only on loopback and answers only requests whose `Host` is its
  own loopback address, which blocks DNS rebinding.
- It grants a session only through the one-time login link printed to the
  terminal. The session cookie is `HttpOnly` and `SameSite=Strict`.
- API calls must carry a custom header and, when the browser sends them, a
  same-origin `Origin` and `Sec-Fetch-Site`. Writes must be JSON. Other sites
  therefore cannot send requests on your behalf.
- It holds the Hubu owner bearer and relays an allow-list of admin routes.
  The server enforces owner presence. Settlement/release routes accept only
  the human reconciliation envelope and reject executor finalization fields.
- Pages are served with a strict Content Security Policy, and server data is
  rendered as text.

## Human reconciliation in Decisions

The outstanding list and count read the current owner-scoped expired-claim queue,
including legacy claims without captured lifecycle events. Historical outcomes
remain immutable: a reconciliation-entry event still says when reconciliation
became necessary. Its **Current reconciliation** column reads the underlying
claim today. The **Current work** filter distinguishes outstanding and resolved
reconciliation events; Load more continues through history pages. Existing
agent/category/time filters apply to history; the outstanding list is labeled as
all agents and independent of those historical filters.

Details show billing/settlement versus non-billing/release, resolution time,
provider reference, supporting evidence, recorded settlement amount/cost basis,
resolver and current budget balances. Missing evidence is explicitly unavailable.
A legacy or quote-based receipt is not presented as independently confirmed
vendor cost. The recorded amount and human evidence remain visible separately.

To resolve an expired claim, choose **Resolve reconciliation**, enter the provider
reference and evidence, then choose vendor billed or vendor did not bill. Billed
cases also require the exact actual cost in major currency units (for example,
`0.13512` USD means $0.13512), provider request ID, existing artifact/receipt
reference, and price/model snapshot JSON with `provider`, `model` and matching
`currency`. Do not copy a reserved estimate as evidence of an actual charge or
invent an artifact reference. Review the claim, outcome, amount and resulting
budget change before confirming. The accounting charge rounds up once to six
decimal places; a confirmed overrun can make the remaining balance negative.

After you review the evidence, **Confirm reconciliation** opens the Touch ID
dialog. The server validates ownership, lease expiry, receipt and evidence,
and atomically settles or releases the hold after confirmation. Active claims cannot be reconciled. Duplicate clicks
are disabled, an identical retry is idempotent, and conflicting stale submissions
show errors. Rejected submissions preserve entered evidence and require a fresh
review. Success refreshes current details, outstanding counts and budget data.
Reconciliation changes accounting; it does not recover images or rerun work.

`GET /governance/records/{record_id}/reconciliation` is a bearer-authenticated,
owner-derived current detail for reconciliation events. It returns `claim` and
optional `receipt`; unlike the immutable redacted governance feed, this explicit
human detail includes the existing claim evidence and receipt references. It
accepts no snapshot or filters and does not rewrite historical facts. Missing or
foreign records return 404. UI overview/outstanding counts use the current claim
queue, while governance snapshot summaries retain their as-of semantics.

Focused validation uses isolated fixtures only:

```sh
node --test scripts/test-ui-reconciliation.mjs
cargo test -p hubu-cli ui::tests --locked
cargo test -p hubu-api owner_presence --locked
cargo test -p hubu-api reconciliation --locked
```

The reconciliation browser test intercepts HTTP requests. The owner-presence
browser test starts both binaries on loopback with a temporary database and
the fixture provider. It adds a 31.5-second delay at the relay to test the
browser and relay wait. Install Playwright in a temporary directory and set
`HUBU_UI_CHROME` to an installed Chrome executable or install Playwright
Chromium before running the tests:

```sh
ui_test_deps=$(mktemp -d)
npm install --prefix "$ui_test_deps" playwright@1.65.1
cargo build --locked -p hubu-cli -p hubu-api --bins
NODE_PATH="$ui_test_deps/node_modules" node scripts/test-ui-reconciliation-browser.cjs
NODE_PATH="$ui_test_deps/node_modules" node scripts/test-ui-owner-presence-browser.cjs
```
