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
| Overview | Three groups: people and agents; the spend lifecycle (pending approvals, automatic denials, unresolved reconciliations, recent decisions); and policies and budgets (spend against limits, budgets near their limit, policies, spending targets). A non-zero approval or denial count is highlighted, and every tile opens the matching page, pre-filtered where it helps. |
| Approvals | Review pending spend requests, then approve or deny them. |
| Spend history | Authorization records filtered by agent and status, with details. Ledger transactions with their entries. |
| Decisions | Governance records filtered by agent, category and time, with the summary for the same filter. |
| Agents | Register an agent from a name and a version label, list agents, rename an agent from the pencil next to its name, view its name history. |
| Users & targets | List human users, register a human user, set or revoke advisory spending targets. |
| Policies | Read any revision of a policy (rules in plain language and the YAML source), compare a past revision with the current one, then switch to Edit to change the YAML, validate it and save it as a new revision. |
| Budgets | Create budgets, change a limit, revoke, view version history. |
| Providers & secrets | Read-only view grouped by platform: each platform's models, config versions, secret reference name and which targets Gongbu currently offers. |

The current view reloads itself every 15 seconds while the browser tab is
visible, keeping your scroll position. Auto-refresh pauses while a dialog is
open and, once you change a filter, type in a form, load more rows or switch
modes on a view, until you reload that view with **Refresh** or navigate away,
so it never discards work in progress. Turn it off with the **Auto-refresh**
checkbox; the choice is remembered in that browser.

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
- It holds the Hubu bearer and approval credentials itself and relays an
  allow-list of admin routes. Agent spend, claim, settlement and reconciliation
  routes are not reachable from the browser.
- Approving or denying spend and renaming an agent use the human approval
  capability. The UI asks for confirmation twice, and the process refuses
  these calls unless the confirmation header is present.
- Pages are served with a strict Content Security Policy, and server data is
  rendered as text, never as HTML.

`hubu ui` can use the approval capability because it reads the same approval
token file the CLI reads. Anyone who can run commands as your user can already
read that file. Keep the approval token file out of agent sandboxes, as you
would for `hubu spend approval approve`.
