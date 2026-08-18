# Gongbu Carry-Forward Inventory

This inventory preserves work from the Hubu logo-demo image proxy stack that is
useful for a future Gongbu-style executor project.

The architectural decision is that Hubu should remain the spend control plane.
Gongbu, or any other executor that follows the Hubu spend executor contract,
should own vendor credentials, provider adapters, model calls, artifact
handling, and execution-specific retries.

## Keep In Hubu

These belong in Hubu because they define generic spend control-plane behavior:

- PR #43, `codex/logo-demo-spend-authorization`
  - Keep: spend authorization tokens, frozen budget holds, expiry/reconciliation.
  - Status: merged into `main`.

- `docs/spend-executor-contract.md`
  - Keep: the current `hubu-spend-executor-v4.1` single-spend claim, receipt,
    settlement, release, and reconciliation contract.
  - This remains the normative contract for one authorized provider call.

- `docs/multi-spend-mandate-protocol.md`
  - Retain as deferred design research only.
  - Hubu and Gongbu will not implement v5 for the current dogfood phase.
    Multi-provider work uses one independent v4 spend operation per potentially
    billable invocation.

## Carry Forward To Gongbu

These PRs contain execution-plane ideas that should move to the new project
rather than remain Hubu responsibilities.

- PR #44, `codex/logo-demo-image-proxy`
  - Useful for Gongbu: initial image-call endpoint shape, prompt intake, spend
    token handoff, and response shape for generated output references.
  - Hubu lesson: do not expose `/model-calls/image` from Hubu; expose only
    executor spend validation/settlement.

- PR #48, `codex/logo-demo-image-artifact`
  - Useful for Gongbu: artifact writing, output references, local output
    directory behavior.
  - Gongbu requirement: preflight artifact destinations before irreversible
    provider calls when possible.

- PR #51, `codex/logo-demo-provider-adapter`
  - Useful for Gongbu: provider adapter trait/boundary that separates generic
    image generation from vendor-specific HTTP logic.
  - Gongbu requirement: adapters should accept server-side config and never
    expose API keys to agents.

- PR #52, `codex/logo-demo-provider-selection`
  - Useful for Gongbu: explicit adapter selection and rejection of unwired
    provider/model pairs.
  - Gongbu requirement: no silent mock/demo fallback for configured real
    vendors.

- PR #54, `codex/logo-demo-http-json-adapter`
  - Useful for Gongbu: generic HTTP JSON adapter for vendor-like endpoints.
  - Gongbu requirement: parse endpoint URLs and enforce HTTPS or true loopback
    HTTP; reject userinfo smuggling such as `localhost@vendor.example`.

- PR #56, `codex/logo-demo-image-provider-timeout`
  - Useful for Gongbu: provider timeout configuration.
  - Gongbu requirement: apply whole-call deadlines, not just per-socket
    read/write timeouts.

- PR #57, `codex/logo-demo-provider-error-codes`
  - Useful for Gongbu: classifying provider failures into retryable,
    configuration, authentication, rate limit, and invalid response classes.

- PR #58, `codex/logo-demo-provider-idempotency-header`
  - Useful for Gongbu: forwarding a stable idempotency key to vendors when
    supported.
  - Gongbu requirement: derive vendor idempotency from the Hubu authorization
    and operation key without leaking secrets.

- PR #59, `codex/logo-demo-provider-retry-config`
  - Useful for Gongbu: bounded retry configuration for transient provider
    failures.
  - Gongbu requirement: retry only before settlement and only within the
    authorization window.

- PR #60, `codex/logo-demo-http-json-field-mapping`
  - Useful for Gongbu: request/response field mapping for generic vendor
    endpoints.
  - Gongbu requirement: keep mappings server-side or admin-configured; agents
    should not control credential-bearing fields.

- PR #63, `codex/logo-demo-gemini-image-adapter`
  - Useful for Gongbu: Gemini/Nano Banana `generateContent` adapter behavior,
    inline image extraction, and endpoint shape.
  - Gongbu requirement: use the Gemini image `v1beta` endpoint form documented
    in the later runbook update.

- PR #65, `codex/logo-demo-readme-gemini-runbook`
  - Useful for Gongbu: operator runbook for provisioning Gemini/Nano Banana
    credentials and configuring the executor without pasting secrets into chat.

- PR #66, `codex/logo-demo-gemini-e2e-test`
  - Useful for Gongbu: end-to-end mock Gemini flow for executor-level tests.

- PR #68, `codex/logo-demo-provider-error-redaction`
  - Useful for Gongbu: redacting provider API keys and endpoint query tokens
    from returned/logged errors.
  - Gongbu requirement: redact full secret values and parsed endpoint secret
    components.

- PR #69, `codex/logo-demo-output-dir-preflight`
  - Useful for Gongbu: output directory readiness checks before billable vendor
    calls.
  - Gongbu requirement: fail fast before the provider call when the configured
    artifact destination is missing, unwritable, or invalid.

## Hubu Contract Requirements Learned From The Stack

These lessons should stay expressed in Hubu's generic executor contract and
tests:

- Executor spend tokens must be scoped by amount, owner, agent/account,
  merchant, and task.
- Hubu must reject executor validation when the associated budget hold is not
  frozen.
- Hubu must provide explicit settle and release paths so executors can keep
  vendor work outside Hubu while still closing the spend state.
- Settlement should happen only after irreversible billable work succeeds.
- Release should happen only before irreversible billable work occurs.
- Expired or abandoned holds must return budget to the user.

## Likely Archive In Hubu

These PRs are valuable as exploration, but after the spend executor contract
lands they should likely be closed or archived rather than merged into Hubu:

- #44 and #48 through #69, except for any small generic spend-control fixes that
  have already moved into Hubu.

If needed, create a Gongbu bootstrap issue or project board from the "Carry
Forward To Gongbu" section before closing the Hubu PRs.
