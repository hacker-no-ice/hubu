# Executor Conformance Suite

Hubu publishes a black-box conformance suite for the
[`hubu-spend-executor-v4.3` contract](spend-executor-contract.md). Any executor
can run it to check its understanding of Hubu's lifecycle: Gongbu, the
first-party reference executor, or an external executor that implements
authorize → claim → settle/release without running Gongbu.

The suite has three parts:

- [`fixtures/hubu-executor-conformance-v4.3.json`](../fixtures/hubu-executor-conformance-v4.3.json):
  the versioned fixture corpus. It holds the expected v4.3 requests,
  responses, error codes, and retry decisions.
- [`scripts/hubu-executor-conformance.py`](../scripts/hubu-executor-conformance.py):
  a runner that needs only the Python standard library. It replays the corpus
  against the public HTTP API of a real `hubu-server`.
- [`scripts/hubu-executor-conformance-reference-plugin.py`](../scripts/hubu-executor-conformance-reference-plugin.py):
  the smallest external executor that speaks the plugin protocol.

CI runs [`scripts/integration-executor-conformance.sh`](../scripts/integration-executor-conformance.sh).
It runs the corpus once with the built-in executor side and once through the
external plugin.

## Hubu contract assertions and Gongbu-owned tests

This suite validates the **Hubu control plane** only. It never calls a
provider, stores an artifact, or runs Gongbu recovery logic. Its provider
request IDs, pricing snapshots, and artifact references are deterministic
fixture strings. They prove only that Hubu stores and replays the values an
executor reports.

| Concern | Owner | Where it is tested |
| --- | --- | --- |
| Authorization, holds, claims, leases, settlement, release, reconciliation, ledger postings, budget charges | Hubu | This suite (scenarios S01–S10) |
| Gongbu's mapping of Hubu responses to retry/recovery activity classes | Gongbu | `hubu_conformance_retry_decisions_map_to_gongbu_activity_classes` in [`crates/gongbu-api/src/hubu/mod.rs`](../crates/gongbu-api/src/hubu/mod.rs), driven by the corpus `retry_decisions` table |
| Gongbu workflow against a real Hubu, including dropped claim/settlement responses | Gongbu | [`crates/gongbu-api/tests/hubu_executor_e2e.rs`](../crates/gongbu-api/tests/hubu_executor_e2e.rs) via [`scripts/integration-hubu-gongbu-executor.sh`](../scripts/integration-hubu-gongbu-executor.sh); it starts Hubu with the corpus `server_profile` |
| Live provider calls, provider adapters, artifacts, and provider-side recovery | Gongbu | Gongbu's opt-in live-provider tests ([Gongbu execution](gongbu-execution.md)); never run by ordinary CI |

## Running the suite

Against an isolated, runner-managed server. The runner owns the process, so it
can kill and restart it for fault injection:

```bash
cargo build --locked --bin hubu-server
python3 scripts/hubu-executor-conformance.py --server-bin target/debug/hubu-server
```

Against any Hubu base URL (a dedicated, disposable instance):

```bash
python3 scripts/hubu-executor-conformance.py --base-url http://127.0.0.1:8787 --auth-token "$HUBU_AUTH_TOKEN" --reconciliation-token "$HUBU_RECONCILIATION_TOKEN" --restart-command ./restart-my-hubu.sh --run-id "$(date +%s)"
```

An attached target must meet these requirements:

- It uses the corpus `server_profile.lease_config_yaml` through
  `HUBU_LEASE_CONFIG`. The runner checks the published lease profiles before
  any scenario runs.
- It has distinct normal-bearer and human-reconciliation credentials.
- It accepts the corpus policy. The runner installs it as the owner's default
  policy, so never point the suite at a Hubu that holds real governance state.

Without `--restart-command`, S09 is reported as `SKIP` and the run fails unless
`--allow-skips` is passed. `--run-id` keeps agent names unique when you reuse a
target. `--transcript FILE` saves every observed request and response as
evidence.

### Pluggable executor side

Agent-side (`authorize`), observer (budgets, ledger, workflows, reconciliation
queue) and human-reconciliation steps are always sent by the runner. Every
**executor** step goes through the executor side: resolve, claim, claim
inspection, settle, release, and the executor's rejected attempts to
reconcile.

With `--executor-command CMD`, the runner starts `CMD` and talks to it over
stdin/stdout, one JSON object per line
(`hubu-executor-conformance-plugin-v1`):

1. The runner sends
   `{"type":"hello","protocol":"hubu-executor-conformance-plugin-v1","base_url":…,"executor_contract":"hubu-spend-executor-v4.3"}`.
   The executor answers `{"type":"ready","executor":"<name>"}`.
2. For each executor step, the runner sends
   `{"type":"invoke","scenario","step","operation","method","path","body","query","reconciliation_capability"}`.
   The executor performs the operation with its **own** normal Hubu bearer.
   It answers with Hubu's observed `{"status":…,"body":…}`, or
   `{"transport_error":"…"}` if it saw no response.
   The executor must reply within 30 seconds, or the runner kills it and
   fails the run.
3. When `reconciliation_capability` is `"executor_bearer"`, the executor sends
   its own normal bearer in `X-Hubu-Reconciliation-Capability`. Hubu must
   reject this probe. Executors never receive the human capability.
4. At the end, the runner sends `{"type":"shutdown"}`.

The `base_url` in `hello` is the runner's loopback **fault proxy**, not Hubu
itself. For a dropped-response step, the proxy forwards the request so Hubu
commits it. It then closes the executor's connection without writing a
response. The executor's own HTTP client therefore sees a real transport loss,
and it must answer with `transport_error`. The step fails if the executor
reports an HTTP response it could not have received, or if the request never
reached Hubu. Invoke messages don't say which steps drop the response.

A real executor maps each `operation` onto its own Hubu client and returns what
Hubu answered. `method`, `path`, and `body` give the canonical v4.3 request
the executor must send. HUB-206 wires its standalone external-executor example
into this protocol.

## Corpus format and determinism

- `schema` (`hubu-executor-conformance-v1`) versions the corpus format.
  `corpus_version` versions the fixture content. `protocol_version` names the
  Hubu contract under test. A breaking contract version gets a new corpus file,
  such as `…-v4.4.json`.
- `operations` maps each operation name to its public route and default actor.
- `retry_decisions` names every non-success outcome an executor must handle.
  For each one it records the observable response (`status`, `error_contains`,
  and JSON `match`), the required executor action, and Gongbu's activity class.
  A step that expects a rejection refers to one of these decisions instead of
  repeating the response.
- `definitions` holds deterministic inputs: typed scopes, exact costs, and
  complete frozen pricing snapshots. Operation keys are fixed strings such as
  `conformance:v4.3:s02:op-a`. They are agent-scoped, and the runner
  registers a fresh agent and budget for every scenario.
- Server-generated IDs and timestamps are **never** asserted literally. Steps
  `capture` them and assert them by relation: `same`, `different`, `before`,
  `not_after`, `non_null`.
- Fault injection is part of the fixture:
  - `"fault": "drop_response"` routes the request through the fault proxy.
    Hubu commits the request, but the proxy closes the caller's connection
    before any response, so the caller (including an external executor's HTTP
    client) sees only an ambiguous outcome.
  - `{"control": "restart_hubu"}` SIGKILLs and restarts `hubu-server` on the
    same database.
  - `{"control": "await_reconciliation_required"}` polls the public claim until
    Hubu itself reports lease expiry. The `conformance_short` lease profile
    (1-second claim TTL) makes this fast without an injected clock.

## Scenario map

| Scenario | Corpus ID | Key black-box assertions |
| --- | --- | --- |
| 1. Two independent provider operations under one agent budget | `S01` | Two allowed authorizations against the **same** budget produce distinct tokens, holds, decisions, and claims. Budget frozen 700 / remaining 300. Two claimed workflows share a task reference. No postings while claimed. |
| 2. Settle both with precise costs and immutable references | `S02` (includes `S01`) | Exact costs `3.500001` and `2.5001` USD charge 351 and 251 cents with 49-cent releases each. Snapshots, provider request IDs, and artifact references are returned unchanged. Changed provider reference, artifact reference, or an equal-but-rescaled cost is rejected. Exactly one `external_provider` posting per settlement, linked to its claim and workflow. Budget coverage has no unaccounted consumption. |
| 3. Ambiguous retry of authorization, claim, and finalization | `S03` | Each stage's first response is dropped. Retries return the same token, hold, claim, and settlement (`idempotent_replay`, same timestamps). One hold, one consumption, one posting, one workflow. |
| 4. Operation-key reuse with changed scope | `S04` | Changed amount, provider scope, or lease profile returns `409 create_new_operation`. Claims with a changed amount, scope, or operation key are rejected. No extra hold. Exact replay still recovers the original. Changed scope is still rejected after claim. |
| 5. Release before billable work | `S05` | Release returns the full hold once (replay keeps `finalized_at`). Settlement after release is rejected. Claim replay reports `released`. No posting, no receipt. |
| 6. Settlement response loss | `S06` | The claim shows `settled` after the lost response. Release cannot erase billed work. The identical retry returns the stored settlement. A changed-cost retry is rejected. One consumption and one posting. |
| 7. Lease expiry into reconciliation | `S07` | Hubu marks the expired claim `reconciliation_required` and keeps the hold `claimed`/frozen. Normal settle and release are rejected. The claim appears in the owner reconciliation queue. The workflow shows `reconciliation_required`. No posting. |
| 8. Human reconciliation, both outcomes | `S08` | Executor attempts with only the normal bearer (missing capability) or the bearer presented as the capability (invalid) are rejected for both outcomes, and the queue is unchanged. The human `vendor_billed` path settles with one posting. `vendor_did_not_bill` releases with no posting. Replays are stable. Changed evidence is rejected. |
| 9. Restart between stages | `S09` | SIGKILL/restart after authorization (including a denied-then-corrected revision), after claim, and after a lost settlement response. Every replay returns the same durable IDs and timestamps. Budget and posting stay consistent. |
| 10. Operation maximum and agent budget | `S10` | Authorization over the budget, over the remaining balance, and one cent past exhaustion is denied with no hold. A claim above the operation maximum is rejected. Settlement one cent over the maximum is rejected and leaves the hold claimed. Settlement at exactly the maximum succeeds. A denied key can be corrected within the remaining balance. |

The former scenario 11 (executor capability cannot call administrative routes)
belongs to HUB-32. Add it to this corpus when an executor-scoped credential
exists.

S09's corrected-retry restart step is a regression guard. Hubu previously
recreated a superseded `(agent_id, operation_key)` unique index on every
database open. After a denied authorization was corrected under the same
operation key, `hubu-server` then refused to restart.
