# Use Hubu with Your Own Executor

Hubu controls spend; executors do the work. Gongbu is the first-party executor,
but it can't cover every provider or workload. Any service can act as the
executor for Hubu-authorized work. It needs only the normal Hubu bearer and the
[`hubu-spend-executor-v4.4`](spend-executor-contract.md) HTTP contract. This
guide follows one operation from the agent's MCP call to settlement.

```text
agent ──hubu_authorize_spend──▶ unified MCP ──▶ Hubu   (reserve the budget)
agent ──continuation token────▶ your executor
your executor ──resolve / claim / settle or release──▶ Hubu (HTTP, v4.4)
agent ──hubu_operation_status─▶ unified MCP ──reads──▶ Hubu authorization record
```

## 1. The agent authorizes through MCP

The agent calls `hubu_authorize_spend` with the account, amount, reason and
typed `execution_scope`. The harness supplies the operation identity in trusted
metadata, so the model never invents or sees an operation key. The result is
one of three decisions:

- **`allow`:** Hubu reserved a budget hold. The result carries the public
  `operation_handle` and the authorization continuation, `auth_token_id`.
- **`deny`:** this outcome is terminal. Nothing was reserved, and corrected work
  needs a new tool call.
- **`needs_approval`:** a human inspects and resolves the approval. The agent
  then calls `hubu_resume_operation` with the same `operation_handle`. Resuming
  a standalone authorization never starts Gongbu; it returns the continuation
  in `hubu_result.auth_token_id`.

The agent hands the continuation token to exactly one executor. It never passes
Hubu credentials or operation keys.

## 2. The executor claims before billable work

Your executor authenticates with its own normal Hubu bearer. It claims by the
token and finalizes by the `claim_id` that the claim returns:

1. Persist the token durably before doing anything else.
2. `POST /spend/executor/resolve` with `{"spend_auth_token_id": …}` reads the
   authoritative account, amount and typed scope. Check them against the work
   you priced.
3. `POST /spend/executor/claim` with the token plus `account_id`,
   `amount_cents` and `execution_scope`. Never send `operation_key` or
   `agent_id`; v4.4 rejects them. Persist the returned `claim_id`. A retry with
   the same token returns the same claim, even after the authorization
   expires, so a lost claim response is recovered by replaying the claim.

Do no irreversible or billable work until the claim succeeds.

## 3. Finalize: settle or release

- **Billable work happened:** `POST /spend/executor/settle` with
  `{"claim_id": …}` and a receipt. The receipt
  holds the exact vendor cost (`amount`, `scale`, `currency`), the provider
  request ID, the complete frozen pricing snapshot and an artifact reference.
  Hubu consumes the conservative budget charge, releases the remainder, and
  posts exactly one ledger transaction.
- **No billable work happened:** `POST /spend/executor/release` with
  `{"claim_id": …}`. Hubu returns the full hold and posts nothing.

Settle and release reject the token; they are identified by the `claim_id`
only. If a response is lost, replay the identical request. If you never saw
the claim response, replay the claim first to recover the `claim_id`. Never
change a receipt on retry, and never release work that may have been billed.

## 4. Ambiguous outcomes need a human

If a claim's lease expires before you finalize it, Hubu keeps the hold claimed
and normal settle and release are rejected. The same applies when the provider
outcome is unknown. A human checks provider billing and reconciles the claim
with the separate reconciliation capability: `vendor_billed` with a receipt, or
`vendor_did_not_bill`. Executors never hold that capability. See
[expired claims](spend-executor-contract.md#expired-claims).

## 5. The agent observes status by its handle

`hubu_operation_status` reads the operation's Hubu authorization record, so it reflects
your executor's progress without any callback to the router:

| Executor progress | `state` | `replacement_safe` |
| --- | --- | --- |
| not yet claimed | `authorized` | yes |
| claimed | `executing` | **no** |
| settled | `settled` (terminal) | no |
| released | `released` (terminal) | no |
| lease expired unsettled | `reconciliation_required` | **no** |
| Hubu unreachable | `unverified` | **no** |

Agents must not submit a replacement unless `replacement_safe` is true. Full
semantics are in [MCP tool consolidation](mcp-tool-consolidation.md#authority-approval-and-recovery).

## 6. Check your executor against the conformance suite

The [executor conformance suite](executor-conformance.md) replays the
versioned v4.4 corpus against a real `hubu-server`. Plug your executor in
through the JSON-lines plugin protocol:

```bash
python3 scripts/hubu-executor-conformance.py --server-bin target/debug/hubu-server --executor-command "your-executor-conformance-adapter"
```

The suite covers ambiguous responses, process restarts, lease expiry,
reconciliation and budget limits. It also refuses any executor request that
carries the retired operation key or agent.

The MCP-to-executor handoff in this guide is itself qualified end to end by
[`scripts/integration-standalone-mcp-executor.sh`](../scripts/integration-standalone-mcp-executor.sh).
That script runs a real `hubu-server` and `hubu-unified-mcp` with Gongbu
unconfigured.
