# HUB-96 unified MCP migration canary

Date: 2026-08-18 (America/Los_Angeles)

Decision: **NO-GO for standalone MCP deprecation.**

The packaged unified surface passed the runnable clean-install, catalog,
representative workflow, isolation, compatibility, recovery-on-refresh,
redaction, and rollback cases below. Deprecation remains blocked because the
contract-required list-change notification is not implemented, the complete
32-tool behavior parity matrix is not yet present, and the cumulative canary
gate requires two immutable releases and 14 elapsed calendar days. The
standalone `hubu-mcp-server` and `gongbu-mcp` binaries remain supported and were
not deprecated or removed.

## Artifact and build provenance

| Field | Recorded value |
| --- | --- |
| Required base/source | `f0aad7277493d69ce2fb7ed93b71f25898c2de05` |
| Source subject | `HUB-95: Document unified MCP migration and architecture (#152)` |
| Product stamp | `0.1.0-hub96` |
| Target | `aarch64-apple-darwin` |
| Archive | `hubu-0.1.0-hub96-aarch64-apple-darwin.tar.gz` |
| Archive SHA-256 | `34832a06a3db7ac866b871942f917bbf25135fcd578d78a4a7975eabe897fd64` |
| Compiler used for the local canary artifact | `rustc 1.94.1 (e408947bf 2026-03-25)` |
| Build profile | Cargo `dev`; no release-mode build |
| Provider execution | disabled; deterministic loopback fixtures only |

All six production binaries were built with the same product/source stamps.
`scripts/package-release-archive.sh` produced the archive, and
`scripts/verify-release-archive.sh` verified its manifest, provenance,
checksums, binary version surfaces, clean unified configuration, and packaged
rollback surfaces. No runtime source, dependency, lockfile, or public contract
changed between the recorded base and the packaged binaries; HUB-96 changes
only tests, fixtures, verification scripts, and this report.

The reusable invocation is:

```sh
scripts/run-unified-mcp-migration-canary.sh \
  ARCHIVE_PATH \
  0.1.0-hub96 \
  f0aad7277493d69ce2fb7ed93b71f25898c2de05
```

The runner extracts a fresh archive, invokes only its packaged
`hubu-unified-mcp` as the MCP surface, and runs the stamped no-spend matrix. It
does not build release artifacts and is not enabled in ordinary PR CI.

## Case evidence

| Case | Result | Exact observation |
| --- | --- | --- |
| Archive integrity and provenance | PASS | Six production binaries, manifest, provenance, dependency/license files, and both checksum layers verified. All six `--version` outputs matched product `0.1.0-hub96`, source `f0aad7277493d69ce2fb7ed93b71f25898c2de05`, and executor contract `hubu-spend-executor-v4.2`. |
| Clean unified-only client configuration | PASS | Packaged `hubu init codex --dry-run` emitted one `[mcp_servers.hubu]` entry pointing to the packaged `hubu-unified-mcp`, included separate Hubu and Gongbu credential-file variables, and emitted no `[mcp_servers.gongbu]` entry. |
| Unified initialization and full discovery | PASS | Packaged server reported `serverInfo.name = hubu-unified-mcp`; both fixture backends were `available`; `tools/list` matched the independent HUB-88 fixture exactly: 33 names, with no missing or extra tool. |
| Hubu governance | PASS | `hubu_list_budgets` preserved the structured Hubu result and `hubu_authorize_spend` preserved the no-human-approval authorization result plus trusted operation/task metadata. No billable operation occurred. |
| Gongbu execution and artifacts | PASS | `gongbu_create_execution`, `gongbu_get_execution`, `gongbu_list_artifacts`, and `gongbu_get_artifact` passed through packaged stdio. Artifact content was the deterministic PNG header fixture; unsafe storage metadata was removed and token-shaped metadata was redacted. |
| Governed end-to-end workflow | PASS | Hubu authorization returned the fixture spend token, which was supplied to Gongbu execution; the resulting execution and artifact were then read through the same packaged unified process. |
| Partial availability | PASS | Hubu-only, Gongbu-only, Hubu-unavailable, and Gongbu-unavailable cases preserved the healthy backend and rejected only unavailable routes. Gongbu execution remained fail-closed without Hubu. |
| Version mismatch | PASS | Representative Hubu product-version and Gongbu API-schema mismatches reported `incompatible`, blocked the affected routes, and preserved the compatible backend. Unit coverage separately exercises every compatibility dimension. |
| Backend transport stop/recovery | PASS with contract gap | Disabling each backend transport changed it to `unavailable`; restoring it returned it to `available` on bounded explicit capability refresh while the other backend remained independent. The required unsolicited `notifications/tools/list_changed` signal is absent; see HUB-106 and the failed gate below. |
| Secret redaction and credential isolation | PASS | Distinct Hubu and Gongbu canary credentials appeared only in the owning fixture's Authorization header. Secret-bearing backend failures were sanitized, and the test process asserted neither secret appeared in MCP output or stderr. |
| Migration refusal and success | PASS | Migration without replacement Gongbu settings failed without modifying the saved config. Supplying the distinct Gongbu endpoint/token file atomically replaced only the Hubu/Gongbu table families and preserved an unrelated MCP server. |
| Documented rollback | PASS | The verifier restored the exact backed-up two-entry configuration, then initialized and listed the packaged standalone catalogs: 30 Hubu tools and exactly four Gongbu tools. Backend databases or artifacts were not copied or merged. |
| Provider spend safety | PASS | No provider credential, provider endpoint, live provider call, or spend acknowledgement was configured. |

After hardening the fixture's TCP half-close behavior, the exact packaged runner
completed three consecutive times with 9/9 tests passing in each run. Before
that harness-only adjustment, exploratory runs each reported 8/9 with transient
fixture transport failures in different otherwise-healthy probes; the packaged
archive was unchanged. Those exploratory runs are not counted as qualifying
canary releases.

## HUB-88 routing accounting

The independent oracle is
[`fixtures/unified-mcp-routing-v1.json`](../../fixtures/unified-mcp-routing-v1.json).
The packaged `tools/list` response was sorted and compared for exact equality,
and `hubu_unified_capabilities` was compared name-by-name for owner and
availability.

- Router (1): `hubu_unified_capabilities`.
- Gongbu (4): `gongbu_create_execution`, `gongbu_get_artifact`,
  `gongbu_get_execution`, `gongbu_list_artifacts`.
- Hubu (28): `hubu_add_policy`, `hubu_apply_policy`,
  `hubu_authorize_spend`, `hubu_client_approval_profile`,
  `hubu_create_budget`, `hubu_create_recurring_budget`, `hubu_export_policy`,
  `hubu_get_executor_claim`, `hubu_health`, `hubu_list_agents`,
  `hubu_list_budgets`, `hubu_list_claims_requiring_reconciliation`,
  `hubu_list_ledger`, `hubu_list_users`, `hubu_policy_diff`,
  `hubu_policy_history`, `hubu_reconcile_vendor_billed_claim`,
  `hubu_reconcile_vendor_did_not_bill_claim`, `hubu_register_agent`,
  `hubu_register_human`, `hubu_registration_guidance`,
  `hubu_replace_budget`, `hubu_revoke_budget`,
  `hubu_revoke_spending_target`, `hubu_set_spending_target`,
  `hubu_show_policy`, `hubu_show_spending_targets`, and `hubu_submit_spend`.

The standalone-only `hubu_get_spend_approval` and
`hubu_resolve_spend_approval` remain intentionally excluded by HUB-88 and are
tested as absent from the unified catalog.

## Failed deprecation gates and blockers

1. **Compatibility/failure gate: FAIL.** The server advertises
   `capabilities.tools.listChanged = true`, and the contract requires a
   `notifications/tools/list_changed` notification on catalog-affecting state
   transitions. The stdio loop currently emits only request responses; there is
   no notification implementation. [HUB-106](https://linear.app/hubu/issue/HUB-106/emit-unified-mcp-toolslist-changed-notifications-on-backend-state)
   is the bounded P1 blocker.
2. **Behavior parity gate: FAIL.** Static catalog, schema, annotation, and route
   parity is exact, and representative behavior passed. The contract still
   requires golden success plus representative error evidence for every one of
   the 32 mapped tools. [HUB-107](https://linear.app/hubu/issue/HUB-107/complete-32-tool-unified-mcp-golden-behavior-parity-matrix)
   owns that bounded matrix.
3. **Cumulative canary gate: FAIL.** This local package exercise is not an
   immutable published canary release. The contract requires at least two
   consecutive immutable canary releases, 14 completed calendar days, zero
   unresolved P0/P1 defects, migrated Hubu and Gongbu client workflows, and a
   verified rollback. The implementation and migration documentation were
   merged on 2026-08-18, so the elapsed-time requirement cannot yet be met.

Therefore HUB-97 must remain blocked. Re-run this archive-driven canary against
each immutable candidate after HUB-106 and HUB-107 close, retain the release
artifacts and results for at least 14 days, and issue a new explicit decision
only if every cumulative gate passes.

## Scope and architecture

This task does not change components, public interfaces, request ownership, or
storage boundaries, so the architecture visualizer does not need an update.
Hubu and Gongbu remain separate processes with separate credentials, state,
provider execution, artifacts, and failure domains.
