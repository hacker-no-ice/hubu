# Owner presence

From v0.3.0, owner actions need the owner to be at the Mac. Holding a file is
never enough. `hubu-server` checks this itself: it raises the macOS
Touch ID dialog, and it never trusts a client that says the owner approved.
The design is in sections 4, 5 and 9 of the
[v0.3.0 tech spec](specs/v0.3.0-owner-agent-separation.md).

This page covers the server side. The CLI, admin portal and unified MCP
changes are documented with those components.

## Route classes

| Class | Credential | Examples |
| --- | --- | --- |
| Agent | Agent or owner credential | spend, ledger and budget reads, `GET /owner/presence`, `POST /spend/approval/prompt` |
| Owner bearer | Owner credential, no prompt | owner reads, `GET /governance/owner-presence`, `POST /budgets/revoke`, `POST /user/spending-target/revoke`, approval `decision: deny` |
| Owner action | Owner credential plus the owner's confirmation of that action | `POST /budgets`, budget versions, `POST /policies`, spending target, agent rename, `POST /init`, approval `decision: approve`, reconciliation |
| Executor | Owner credential until v0.3.2 | claim, settle and release with an executor body |

A route without a declaration is an owner action, and a request to a path that
does not exist is refused before any prompt. Tightening (deny, revoke) never
prompts, so the owner can always stop spending at once.

## One confirmation, one action

- When an owner action arrives, `hubu-server` raises the dialog and holds the
  request until the owner answers or 60 seconds pass. If the owner confirms,
  it performs that action.
- A confirmation authorizes only the action its dialog named. There is no
  owner window: the next owner action prompts again, and the server keeps no
  owner authority between requests.
- Every spend approval prompts, so the owner always sees the amount. Pressing
  Deny in an approval dialog closes the request as denied, whether the owner
  or an agent raised the dialog. A timeout leaves it pending.
- Dialog first: Hubu checks only what the dialog text needs (the amount, and
  that the named agent or budget exists). All other checks run in the handler
  after the owner confirms. A request that fails there returns its error and
  writes nothing.
- `X-Hubu-No-Prompt: 1` returns `owner_presence_required` instead of raising
  the dialog. Executors always send it.

The dialog reads `"hubu-server" is trying to <reason>.` Hubu builds the reason
from stored, validated data. A spend approval can read
`get your approval for design-agent to spend up to 0.014 USD on black-forest-labsflux/flux-3-image-klein`.
A budget change can read `set design-agent's budget to 20.00 USD (now 10.00 USD)`.
Agent names are reduced to `[A-Za-z0-9._-]`, at most 32 characters.

Approving many pending requests with one dialog is planned
([HUB-399](https://linear.app/hubu/issue/HUB-399)).

## Approving from chat

After the owner says yes in the conversation, an agent may call
`POST /spend/approval/prompt` with `approval_request_id`. Hubu raises the
approval dialog. If the owner confirms, the request is approved. If the owner
denies, it is closed. On a timeout, or when no provider can run, it stays
pending and can be raised again. Prompting a request that is no longer pending
returns `approval_not_pending`.

## Errors

| Code | Status | Meaning |
| --- | --- | --- |
| `owner_authority_required` | 403 | The agent credential called an owner route |
| `owner_presence_required` | 403 | An owner action sent `X-Hubu-No-Prompt` |
| `owner_presence_denied` | 403 | The owner refused, or the dialog failed |
| `owner_presence_timeout` | 403 | No answer within 60 seconds |
| `owner_presence_unavailable` | 403 | No presence provider can run, for example with no GUI session |
| `owner_prompt_in_progress` | 409 | Another dialog is on screen |
| `owner_prompt_rate_limited` | 429 | 5 refused or timed-out prompts in the last 10 minutes; confirmed prompts do not count |
| `approval_not_pending` | 409 | The approval was already resolved (approved or denied) |

Requests that name an unknown agent or budget are refused before any prompt.

## Records

Each prompt is recorded with its action, the exact dialog text, the outcome
(`confirmed`, `denied`, `timed_out`, `unavailable`, `rate_limited`,
`superseded`), the method and the client. So are tightening writes
(`not_required`). Read them with `GET /governance/owner-presence`
(owner credential, `limit` 1–200, newest first).

## Providers

| `HUBU_OWNER_PRESENCE_PROVIDER` | Behaviour |
| --- | --- |
| unset or `system` | macOS LocalAuthentication. `HUBU_OWNER_PRESENCE_POLICY=owner` (default) allows Touch ID or the login password; `biometrics` allows Touch ID only. Other platforms have no provider, so owner actions fail with `owner_presence_unavailable`. |
| `fixture` | Tests and proof kits only. Answers from `HUBU_OWNER_PRESENCE_FIXTURE`, a comma-separated script of `confirm`, `deny`, `timeout` and `unavailable`, where the last answer repeats (default `confirm`). The prompt rate limit is off unless `HUBU_OWNER_PRESENCE_FIXTURE_RATE_LIMIT=on`. `hubu-server` refuses to start with it when the database is inside a managed profile. |

`hubu-server` must run in the owner's GUI session for the dialog to appear.
`hubu stack start` already starts it that way.

## Retired capabilities

`hubu-server` no longer reads or creates the approval and reconciliation
capability files, and it ignores the `X-Hubu-Approval-Capability` and
`X-Hubu-Reconciliation-Capability` headers.
