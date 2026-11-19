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
browser, stop `hubu ui` and start it again.

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
- It holds the Hubu bearer, approval and reconciliation credentials itself and
  relays an allow-list of admin routes. Agent spend and claim creation remain
  unavailable. Settlement/release routes accept only a confirmed human
  reconciliation envelope, never executor finalization fields.
- Approving or denying spend and renaming an agent use the human approval
  capability. The UI asks for confirmation twice, and the process refuses
  these calls unless the confirmation header is present.
- Pages are served with a strict Content Security Policy, and server data is
  rendered as text, never as HTML.

`hubu ui` can use the approval capability because it reads the same approval
token file the CLI reads. Anyone who can run commands as your user can already
read that file. Keep the approval token file out of agent sandboxes, as you
would for `hubu spend approval approve`.


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

The separate human reconciliation capability stays in the UI process, as with
the CLI. It is attached only to confirmed reconciliation mutations. The server
still validates ownership, lease expiry, receipt and evidence, and atomically
settles/releases the hold. Active claims cannot be reconciled. Duplicate clicks
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
cargo test -p hubu-api reconciliation --locked
```

The browser fixture suite intercepts every HTTP request and never contacts a
running Hubu/Gongbu profile or provider. Install Playwright in a temporary folder,
then run (set `HUBU_UI_CHROME` to an installed Chrome executable, or install
Playwright Chromium):

```sh
ui_test_deps=$(mktemp -d)
npm install --prefix "$ui_test_deps" playwright@1.65.1
NODE_PATH="$ui_test_deps/node_modules" node scripts/test-ui-reconciliation-browser.cjs
```
