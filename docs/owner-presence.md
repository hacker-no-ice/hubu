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
| Agent | Agent or owner credential | spend, ledger and budget reads, `GET /owner/presence`, `POST /owner/lock`, `POST /spend/approval/prompt` |
| Owner bearer | Owner credential, no prompt | owner reads, `POST /budgets/revoke`, `POST /user/spending-target/revoke`, approval `decision: deny` |
| Owner action | Owner credential plus an owner window | `POST /budgets`, budget versions, `POST /policies`, spending target, agent rename, `POST /init`, approval `decision: approve`, reconciliation, `POST /owner/unlock` |
| Executor | Owner credential until v0.3.2 | claim, settle and release with an executor body |

A route without a declaration is an owner action. Tightening (deny, revoke,
lock) never prompts, so the owner can always stop spending at once.

## Owner windows

- When an owner action arrives with no window, `hubu-server` raises the
  dialog and holds the request until the owner answers or 60 seconds pass.
  If the owner confirms, it performs the action and returns
  `owner_window: { token, expires_at }` in the response.
- The window lasts 5 minutes from confirmation and does not slide. The client
  that opened it sends the token as `X-Hubu-Owner-Window` on later owner
  actions. The server keeps only a hash of the token, in memory.
- A window closes when it expires, on `POST /owner/lock`, and when the server
  stops. Each client (`X-Hubu-Client: portal` or `cli`) has at most one window,
  and a new window replaces the old one.
- Every spend approval prompts, even inside a window, so the owner always sees
  the amount.
- `X-Hubu-No-Prompt: 1` returns `owner_presence_required` instead of raising
  the dialog. Executors always send it.

The dialog reads `"hubu-server" is trying to <reason>.` Hubu builds the reason
from stored, validated data, for example
`set design-agent's budget to 20.00 USD (now 10.00 USD)`. Agent names are
reduced to `[A-Za-z0-9._-]`, at most 32 characters.

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
| `owner_presence_required` | 403 | No valid window, and the request sent `X-Hubu-No-Prompt` |
| `owner_presence_denied` | 403 | The owner refused, or the dialog failed |
| `owner_presence_timeout` | 403 | No answer within 60 seconds |
| `owner_presence_unavailable` | 403 | No presence provider can run, for example with no GUI session |
| `owner_prompt_in_progress` | 409 | Another dialog is on screen |
| `owner_prompt_rate_limited` | 429 | More than 5 prompts in 10 minutes |
| `approval_not_pending` | 409 | The approval was already resolved or expired |

Requests that name an unknown agent or budget are refused before any prompt.

## Records

Each prompt is recorded with its action, the exact dialog text, the outcome
(`confirmed`, `denied`, `timed_out`, `unavailable`, `rate_limited`,
`superseded`), the method and the client. So are owner actions performed inside
a window (`window`, with the record of the prompt that opened it) and tightening
writes (`not_required`). Read them with `GET /governance/owner-presence`
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
