const sharedLinks = {
  readme: ["README", "README.md"],
  api: ["Local HTTP API", "crates/hubu-api/src/lib.rs"],
  appSpend: ["Spend approval service", "crates/hubu-core/src/app/spend_approval.rs"],
  appClaims: ["Executor claim service", "crates/hubu-core/src/app/executor_claim.rs"],
  appBudgetUpdate: ["Budget update service", "crates/hubu-core/src/app/budget_update.rs"],
  cli: ["CLI", "crates/hubu-cli/src/main.rs"],
  stackProviderProfile: ["Supported provider profile source", "contracts/provider-profiles-v1.json"],
  stackProviderDoctor: ["Provider profile doctor and catalog", "crates/hubu-cli/src/stack/doctor.rs"],
  stackLifecycle: ["Local stack lifecycle", "crates/hubu-cli/src/stack/lifecycle.rs"],
  stackConfiguration: ["Outcome-oriented stack configuration", "crates/hubu-cli/src/stack.rs"],
  managedCredentialHandoff: ["Managed Gongbu credential handoff", "crates/gongbu-api/src/config/setup.rs"],
  localStack: ["Local stack quick start", "docs/local-stack.md"],
  localStackAcceptance: ["Local stack acceptance canary", "scripts/integration-local-stack-acceptance.sh"],
  operationKeySkill: ["Operation-key skill", "skills/generate-hubu-operation-key/SKILL.md"],
  operationKeyHelper: ["Operation-key helper", "skills/generate-hubu-operation-key/scripts/operation_keys.py"],
  common: ["Shared models", "crates/hubu-common/src/lib.rs"],
  user: ["User manager", "crates/hubu-core/src/user.rs"],
  registration: ["Registration manager", "crates/hubu-core/src/registration/manager.rs"],
  registrationModel: ["Registration model", "crates/hubu-core/src/registration/model.rs"],
  registrationProtocol: ["Agent registration deep dive", "docs/agent-registration.md"],
  policyEngine: ["Policy engine", "crates/hubu-core/src/policy/engine.rs"],
  policyModel: ["Policy model", "crates/hubu-core/src/policy/model.rs"],
  policyCondition: ["Policy conditions", "crates/hubu-core/src/policy/condition.rs"],
  spend: ["Spend manager", "crates/hubu-core/src/spend/manager.rs"],
  spendModel: ["Spend model", "crates/hubu-core/src/spend/model.rs"],
  spendExecutor: ["Spend executor contract", "docs/spend-executor-contract.md"],
  executionScope: ["Spend lifecycle", "docs/spend-lifecycle.md"],
  scopeModel: ["Execution scope model", "crates/hubu-common/src/execution_scope.rs"],
  budget: ["Budget manager", "crates/hubu-core/src/budget/manager.rs"],
  budgetModel: ["Budget model", "crates/hubu-core/src/budget/model.rs"],
  spendingTarget: ["Spending target model", "crates/hubu-core/src/spending_target.rs"],
  payment: ["Payment manager", "crates/hubu-wallet/src/payment.rs"],
  paymentAttempt: ["Payment attempt store", "crates/hubu-wallet/src/persistence.rs"],
  rail: ["Payment rail", "crates/hubu-wallet/src/rail.rs"],
  ledger: ["Ledger", "crates/hubu-wallet/src/ledger.rs"],
  storage: ["Core SQLite storage", "crates/hubu-core/src/storage.rs"],
  persistence: ["Governance persistence", "crates/hubu-core/src/persistence.rs"],
  telemetry: ["Telemetry", "crates/hubu-core/src/telemetry.rs"],
  releases: ["Release runbook", "docs/operations/releases.md"],
  releaseWorkflow: ["Release workflow", ".github/workflows/release.yml"],
  sourceInstaller: ["Source installer", "scripts/install-from-source.sh"],
  gongbuOverview: ["Gongbu execution plane", "docs/gongbu-execution.md"],
  gongbuServer: ["Gongbu server runbook", "docs/operations/gongbu-server.md"],
  gongbuServerConfig: ["Gongbu server configuration", "crates/gongbu-api/src/server.rs"],
  gongbuApplication: ["Gongbu composition", "crates/gongbu-api/src/application.rs"],
  gongbuWorkflow: ["Gongbu workflow", "crates/gongbu-api/src/workflow.rs"],
  gongbuTemporal: ["Gongbu Temporal activities", "crates/gongbu-api/src/temporal.rs"],
  gongbuExecution: ["Gongbu execution store", "crates/gongbu-api/src/execution/mod.rs"],
  gongbuArtifact: ["Gongbu artifact service", "crates/gongbu-api/src/artifact/mod.rs"],
  gongbuAttestation: ["Gongbu FLUX attestation", "crates/gongbu-api/src/attestation.rs"],
  gongbuProvider: ["Gongbu provider boundary", "crates/gongbu-api/src/provider/mod.rs"],
  gongbuPricing: ["Gongbu provider contract", "crates/gongbu-api/src/provider/contract.rs"],
  gongbuFlux: ["FLUX asynchronous adapter", "crates/gongbu-api/src/provider/flux2_api.rs"],
  gongbuSupportedProfiles: ["Supported profile production validator", "crates/gongbu-api/src/provider/supported_profiles.rs"],
  gongbuProviderConfig: ["Provider configuration", "docs/configuration/local-stack/v1/providers-toml.md"],
  managedFluxProfile: ["Managed FLUX profile runbook", "docs/operations/managed-flux-profile.md"],
  gongbuHubu: ["Gongbu Hubu client", "crates/gongbu-api/src/hubu/mod.rs"],
  unifiedMcp: ["Unified MCP router", "crates/hubu-unified-mcp/src/lib.rs"],
  unifiedGovernedExecution: ["Composite governed execution", "crates/hubu-unified-mcp/src/governed_execution.rs"],
  unifiedHubuCatalog: ["Unified Hubu tool catalog", "crates/hubu-unified-mcp/src/hubu/catalog.rs"],
  unifiedHubuRouting: ["Unified Hubu request routing", "crates/hubu-unified-mcp/src/hubu/routing.rs"],
  unifiedOperationRegistry: ["Unified operation registry", "crates/hubu-unified-mcp/src/operation_registry.rs"],
  unifiedResumeOperation: ["Unified resume workflow", "crates/hubu-unified-mcp/src/resume_operation.rs"],
  unifiedOperationWorker: ["Durable operation worker", "crates/hubu-unified-mcp/src/operation_worker.rs"],
  unifiedGongbuCatalog: ["Unified Gongbu tool catalog", "crates/hubu-unified-mcp/src/gongbu/catalog.rs"],
  unifiedGongbuFixture: ["Gongbu tool golden fixture", "crates/hubu-unified-mcp/tests/fixtures/gongbu-tool-definitions-v2.json"],
  unifiedMcpStdio: ["Unified MCP stdio lifecycle", "crates/hubu-unified-mcp/src/stdio.rs"],
  unifiedMcpNotifications: ["Unified MCP catalog transitions", "crates/hubu-unified-mcp/src/notification.rs"],
  unifiedMcpContract: ["Unified MCP contract", "docs/unified-mcp.md"],
  gongbuConfig: ["Gongbu server example", "examples/gongbu/gongbu.server.json"],
};

const components = {
  top: {
    title: "Major Components",
    kind: "Overview",
    viewBox: "0 0 1440 900",
    copy:
      "Agents use one default MCP surface. Its governed-execution tool can authorize, execute, observe, and deliver a normal auto-approved result in one bounded call; its router-owned resume workflow recovers approved primitive Hubu operations or continues stored governed intent by public handle. A versioned managed FLUX profile passes through source doctor/render and Gongbu production validation before its sanitized catalog is exposed. Hubu and Gongbu retain separate credentials, storage, provider work, artifacts, and failure domains.",
    responsibilities: [
      "Humans register, attach user-level policies, optionally set advisory spending targets, create agent budgets, approve or deny pending spend, review protected actions, and reconcile uncertain expired claims.",
      "Agents discover Hubu governance plus Gongbu's operator-approved execution targets, exact pricing, runtime image options, execution/artifact primitives, and one router-owned `hubu_submit_governed_execution` composite through `hubu-unified-mcp`; trusted client metadata supplies operation and optional task identity outside model-authored arguments.",
      "For guarded local dogfooding, a repository Codex skill allocates one key only after human authorization, binds it to the exact canonical unified-MCP call, and persists recovery state outside the Hubu server; the router reads that key privately while trusted callId remains the operation identity.",
      "Outcome-oriented initialization offers sandbox, local-stack, and Hubu-only modes. Sandbox and local-stack coordinate the complete Hubu, Gongbu, and Temporal ecosystem; Hubu-only deliberately omits the execution plane and exposes governance without reporting absent backends as failures.",
      "The CLI derives private managed credential locations, starts the final Hubu once, and, when the selected mode includes Gongbu, waits for protected readiness, invokes Gongbu's credential handoff, and only then starts Gongbu; server-bound CLI calls consume the active profile's endpoint and capability paths as one authenticated client context.",
      "The supported FLUX profile freezes provider `flux`, adapter `flux2_api`, model `flux-2-pro`, one-image PNG/JPEG capability, the certified 1k/2k/4k dimensions and reviewed USD pricing, zero generation retries, no fallback, polling, artifact delivery, and durable recovery as one versioned contract.",
      "CLI catalog and doctor report configured, credential-reference-present, production-validated, and live-qualified as independent facts. These checks inspect only local source, validators, Keychain item existence, and local service state; readiness never calls BFL and remains not live-qualified until a separate qualification records evidence.",
      "For that exact successful tuple, Gongbu durably counts poll/fetch transport entries and exposes one bodyless read-only attestation that revalidates the normalized artifact and returns only safe cardinalities and canonical projection hashes; it never moves credential comparison into Hubu or the MCP router.",
      "Stack startup is principal-neutral: registration after startup needs no render, activation, stop, or restart, and each new Gongbu execution persists the account and agent resolved from Hubu authorization.",
      "The clean-environment canary proves one launcher-owned final Hubu process, private credential materialization and reuse without leaks, then starts the real Gongbu, worker, and managed Temporal processes. Release binaries expose the deterministic fixture only behind Gongbu's explicit sandbox provider mode.",
      "Local HTTP callers reach the API with the Hubu bearer token before protected routes resolve user authority.",
      "Hubu resolves typed provider, executor, capability, and billing identities against its trusted catalog before policy evaluation and binds the canonical scope to authorization.",
      "The recommended initial-user install pins an exact release tag and full source commit, then builds and verifies four production binaries under one product version and provenance identity.",
      "Gongbu exclusively claims, preserves exact integer vendor cost and its frozen pricing snapshot, then Hubu charges budget cents with checked ceiling or releases active authorized spend; uncertainty and overruns return to a human decision.",
      "Exact token replay is resolved against Gongbu's persisted immutable request before Hubu, and settlement or release uses the persisted execution agent.",
      "The API handles local HTTP concerns and delegates spend approval, payment, and executor claim lifecycle orchestration to core app services.",
      "Gongbu owns its process, database, Temporal workflow state, vendor credentials, provider adapters, model calls, artifacts, and recovery. Asynchronous generation submits once, checkpoints a safe provider operation in Gongbu SQLite, and resumes read-only polling without moving that state into Hubu.",
      "A mixed Gemini+FLUX catalog keeps provider targets, opaque Keychain coordinates, execution attempts, and execution-bound artifacts distinct inside Gongbu, while Hubu and Gongbu continue to own separate databases and failure domains.",
      "Separate Hubu and Gongbu SQLite records preserve exact receipt amount, scale, currency, frozen pricing evidence, conservative budget charge, overrun, claims, and replay identity alongside their independently owned governance and execution state.",
      "The unified MCP surface is the only agent-facing surface; its composite reuses Hubu authorization and the durable Gongbu worker, while public-handle resume replays primitive Hubu intent without Gongbu and binds execution only for governed work. Neither creates a second state machine or collapses independently configured backend clients.",
    ],
    links: [sharedLinks.readme, sharedLinks.api, sharedLinks.appSpend, sharedLinks.appClaims, sharedLinks.cli, sharedLinks.stackConfiguration, sharedLinks.stackProviderProfile, sharedLinks.stackProviderDoctor, sharedLinks.managedFluxProfile, sharedLinks.localStack, sharedLinks.localStackAcceptance, sharedLinks.gongbuOverview, sharedLinks.gongbuApplication, sharedLinks.gongbuAttestation, sharedLinks.gongbuSupportedProfiles, sharedLinks.unifiedMcp, sharedLinks.unifiedMcpContract, sharedLinks.releases, sharedLinks.releaseWorkflow, sharedLinks.spendExecutor, sharedLinks.executionScope, sharedLinks.scopeModel],
    zones: [
      { label: "Hubu control-plane process + owned state", x: 650, y: 48, w: 730, h: 366, labelX: 682, labelY: 84 },
      { label: "Gongbu execution plane — sandbox + local-stack modes", x: 650, y: 484, w: 730, h: 366, labelX: 682, labelY: 520 },
    ],
    nodes: [
      { id: "human", label: "Human owner", sub: "setup + decisions", x: 42, y: 108, w: 212, h: 100, tone: "human" },
      { id: "agent", label: "AI agent", sub: "one MCP connection", x: 42, y: 380, w: 212, h: 100, tone: "agent" },
      { id: "cli", label: "Hubu CLI", sub: "outcome init + provider catalog", x: 340, y: 110, w: 230, h: 96, tone: "surface" },
      { id: "mcp", label: "Unified MCP", sub: "Hubu-only or governed execution", x: 340, y: 382, w: 230, h: 96, tone: "surface", path: "crates/hubu-unified-mcp/src/lib.rs" },
      { id: "release", label: "Versioned install", sub: "exact tag + full SHA", x: 340, y: 708, w: 230, h: 96, tone: "surface" },
      { id: "api", label: "Hubu HTTP API", sub: "Hubu bearer", x: 706, y: 132, w: 226, h: 104, tone: "core" },
      { id: "app", label: "Governance services", sub: "policy + budgets + claims", x: 1010, y: 132, w: 250, h: 104, tone: "core", path: "crates/hubu-core/src/app/mod.rs" },
      { id: "ledger", label: "Hubu SQLite", sub: "governance + ledger", x: 1010, y: 286, w: 250, h: 96, tone: "data", path: "crates/hubu-core/src/storage.rs" },
      { id: "gongbu", label: "Gongbu HTTP API", sub: "execution + safe catalogs + attestation", x: 706, y: 570, w: 226, h: 104, tone: "executor", path: "crates/gongbu-api/src/http/mod.rs" },
      { id: "workflow", label: "Provider execution", sub: "isolated attempt + resume poll", x: 1010, y: 548, w: 250, h: 104, tone: "executor", path: "crates/gongbu-api/src/workflow.rs" },
      { id: "gongbuData", label: "Gongbu state", sub: "SQLite checkpoints + artifacts", x: 1010, y: 716, w: 250, h: 96, tone: "data", path: "crates/gongbu-api/src/application.rs" },
    ],
    edges: [
      ["human", "cli", "operate"],
      ["agent", "mcp", "initialize + tools"],
      ["cli", "api", "Hubu credential"],
      ["mcp", "api", "Hubu tools + authorize", { fromSide: "right", toSide: "left", waypoints: [{ x: 610, y: 430 }, { x: 610, y: 184 }], labelSegment: 1, labelDx: 34 }],
      ["mcp", "gongbu", "sandbox/local: discover/select + execute", { fromSide: "right", toSide: "left", waypoints: [{ x: 610, y: 430 }, { x: 610, y: 622 }], labelSegment: 1, labelDx: 42 }],
      ["api", "app", "govern"],
      ["app", "ledger", "persist"],
      ["gongbu", "workflow", "execute"],
      ["workflow", "gongbuData", "checkpoint + artifacts"],
      ["gongbu", "api", "resolve attribution + finalize", { fromSide: "top", toSide: "bottom", waypoints: [{ x: 819, y: 458 }, { x: 819, y: 430 }], labelSegment: 1, labelDx: 150 }],
    ],
  },
  release: {
    title: "Immutable Releases",
    kind: "Component",
    copy:
      "The release workflow turns one exact main commit into an immutable tag, validates the recommended native source installation on both supported macOS architectures, and publishes secondary archives from the same four-binary workspace build.",
    responsibilities: [
      "Publishes only after an explicit operator dispatch: a commit-addressed canary, a versioned candidate, or a stable SemVer release for an exact main revision.",
      "Runs formatting, Clippy, workspace tests, the core integration flow, and source-installer contract tests before publication.",
      "Exercises the exact-tag, full-commit source installer natively on Intel and Apple silicon; one locked build produces only hubu, hubu-server, hubu-unified-mcp, and gongbu-server with non-development version and provenance metadata.",
      "Stages and verifies all four binaries before installing them into the chosen prefix, without relying on Apple signing or notarization.",
      "Preserves separate Hubu and Gongbu runtime boundaries while sharing one product version and source provenance identity.",
      "Keeps target archives as secondary convenience artifacts with licenses, notices, lockfile, manifest, provenance, and SHA-256 checksums, then smoke-tests their download, startup, unified MCP initialization, and version surfaces.",
      "Keeps the Hubu product version separate from the hubu-spend-executor-v4.3 contract identifier so consumers can negotiate compatibility explicitly.",
    ],
    links: [sharedLinks.sourceInstaller, sharedLinks.releaseWorkflow, sharedLinks.releases, sharedLinks.common, sharedLinks.api, sharedLinks.cli, sharedLinks.unifiedMcp, sharedLinks.gongbuApplication],
    nodes: [
      { id: "source", label: "Exact main commit", sub: "40-character SHA", x: 62, y: 224, w: 210, h: 92, tone: "data" },
      { id: "checks", label: "Release gates", sub: "fmt + lint + tests", x: 352, y: 224, w: 210, h: 92, tone: "core" },
      { id: "matrix", label: "Installer validation", sub: "Intel + Apple silicon", x: 642, y: 224, w: 210, h: 92, tone: "core" },
      { id: "published", label: "GitHub Release", sub: "exact tag + archives", x: 928, y: 112, w: 210, h: 92, tone: "data" },
      { id: "smoke", label: "Archive smoke", sub: "download + start", x: 928, y: 356, w: 210, h: 92, tone: "agent" },
      { id: "consumer", label: "Initial-user install", sub: "tag + SHA source build", x: 642, y: 510, w: 210, h: 92, tone: "executor" },
    ],
    edges: [
      ["source", "checks", "checkout", { labelDy: -44 }],
      ["checks", "matrix", "gate"],
      ["matrix", "published", "publish once", { fromSide: "top", toSide: "left", waypoints: [{ x: 747, y: 170 }, { x: 900, y: 170 }, { x: 900, y: 158 }], labelSegment: 1 }],
      ["published", "smoke", "download"],
      ["matrix", "consumer", "validated source path"],
    ],
  },
  api: {
    title: "Local HTTP API",
    kind: "Component",
    copy:
      "The local server is a small TCP HTTP API. It authenticates protected local requests with a bearer token, owns the shared process state, exposes JSON routes, resolves public IDs, and leaves spend approval, payment, and claim state transitions to core app services.",
    responsibilities: [
      "Frames each request at CRLF-CRLF, validates Content-Length, reads exactly the declared body, and bounds header size, body size, and socket read time.",
      "Keeps health and guidance public while requiring a local bearer token for protected routes plus distinct human capabilities for approval and reconciliation mutations.",
      "Uses the local token and current user context for protected workflow authority, while refusing to treat executor possession of that token as human approval or reconciliation authority.",
      "Exposes owner-scoped approval lookup and resolve routes; approve and deny are idempotent, while conflicting resolutions are rejected.",
      "Hydrates state from the configured SQLite path and reconciles expired budget holds at startup.",
      "Delegates authorize/payment to `SpendApprovalService` and claim, lookup, queue selection, settle/release, and reconciliation to `ExecutorClaimService` so both workflows are testable without HTTP.",
      "Bridges wallet payment authorization and durable external executor claims through shared spend and budget state.",
      "Uses one stable platform operation key as the agent-scoped workflow identity, with immutable authorization revisions for safe scope correction after terminal denial.",
      "Returns immutable attempt audit and structured retry guidance, while SQLite atomically admits corrected revisions and rejects unsafe changed scope with conflict status.",
      "Uses SQLite as the finalization authority so exact receipt, conservative budget charge, overrun, claim, token, hold, and balance commit atomically, settle serializes against release, and identical executor or human reconciliation retries return stored state.",
      "Writes managed structured events through one bounded JSONL sink, rotates four 10 MiB generations, keeps launcher stderr in a distinct per-start capture, suppresses successful liveness, version, and explicitly marked protected-readiness request noise, and retains unmarked reads plus failed probe diagnostics.",
    ],
    links: [sharedLinks.api, sharedLinks.appSpend, sharedLinks.appClaims, sharedLinks.spendExecutor, sharedLinks.persistence, sharedLinks.telemetry],
    nodes: [
      { id: "routes", label: "HTTP framing + routes", sub: "bounded GET/POST JSON", x: 72, y: 92, w: 220, h: 90, tone: "agent" },
      { id: "auth", label: "Local auth", sub: "bearer + owner caps", x: 410, y: 76, w: 220, h: 92, tone: "core" },
      { id: "state", label: "ServerState", sub: "shared managers", x: 410, y: 250, w: 220, h: 96, tone: "core" },
      { id: "app", label: "App services", sub: "approval + claims", x: 410, y: 432, w: 220, h: 92, tone: "core", path: "crates/hubu-core/src/app/mod.rs" },
      { id: "registration", label: "Registration", sub: "agent records", x: 805, y: 48, w: 190, h: 84, tone: "core" },
      { id: "governance", label: "Governance DB", sub: "attempts/outcomes/holds", x: 804, y: 180, w: 196, h: 84, tone: "data" },
      { id: "wallet", label: "Wallet", sub: "payment + ledger", x: 808, y: 310, w: 188, h: 84, tone: "wallet" },
      { id: "telemetry", label: "Telemetry", sub: "JSON events", x: 804, y: 464, w: 196, h: 86, tone: "data" },
    ],
    edges: [
      ["routes", "auth", "protect"],
      ["auth", "state", "dispatch"],
      ["state", "registration", "mutate"],
      ["state", "app", "authorize/claims"],
      ["app", "governance", "persist"],
      ["app", "wallet", "execute"],
      ["app", "telemetry", "log"],
    ],
  },
  app: {
    title: "App Services",
    kind: "Component",
    copy:
      "The core app layer coordinates managers and repositories for use cases that need more than one domain object. Spend approval and executor claim lifecycle services can be tested directly without exercising HTTP routes.",
    responsibilities: [
      "Atomically admits an immutable spend-attempt revision before evaluation; only all-denied, side-effect-free history permits corrected scope.",
      "Evaluates a spend request against the selected policy and records allow or deny as final while preserving needs_approval as a durable pending decision.",
      "Resolves pending spend exactly once: approval issues the scoped token and budget hold, while denial creates neither.",
      "Evaluates budget availability at the request's captured instant and reserves exactly one effectively active agent budget for an allowed spend decision.",
      "Persists the spend auth token and frozen budget hold after the budget accepts the request.",
      "Submits wallet payments, persists payment attempts, marks successful tokens used, and settles, releases, or keeps the hold frozen according to the failed-payment retry policy.",
      "Creates and looks up executor claims, derives the reconciliation queue, and coordinates exact-receipt executor or human finalization with checked ceiling, immutable replay, and overrun accounting through one atomic repository boundary.",
      "Returns domain-shaped approval, rejection, payment, and claim state while the API owns authentication, public IDs, and JSON response shape.",
    ],
    links: [sharedLinks.appSpend, sharedLinks.appClaims, sharedLinks.spend, sharedLinks.budget, sharedLinks.persistence, sharedLinks.payment, sharedLinks.paymentAttempt],
    nodes: [
      { id: "input", label: "Use-case input", sub: "internal IDs + policy", x: 78, y: 112, w: 230, h: 92, tone: "core" },
      { id: "approval", label: "Spend approval", sub: "wait + resolve + pay", x: 410, y: 72, w: 224, h: 92, tone: "core", path: "crates/hubu-core/src/app/spend_approval.rs" },
      { id: "claims", label: "Executor claims", sub: "claim + reconcile", x: 410, y: 282, w: 224, h: 92, tone: "core", path: "crates/hubu-core/src/app/executor_claim.rs" },
      { id: "managers", label: "Domain managers", sub: "spend + budget", x: 410, y: 492, w: 224, h: 92, tone: "core", path: "crates/hubu-core/src/spend/manager.rs" },
      { id: "persist", label: "Governance store", sub: "exact cost + budget charge", x: 798, y: 188, w: 238, h: 98, tone: "data", path: "crates/hubu-core/src/persistence.rs" },
      { id: "payment", label: "Payment submit", sub: "wallet boundary", x: 798, y: 444, w: 238, h: 98, tone: "wallet", path: "crates/hubu-wallet/src/payment.rs" },
    ],
    edges: [
      ["input", "approval", "authorize"],
      ["input", "claims", "claim/finalize"],
      ["approval", "managers", "evaluate/reserve", { fromSide: "left", toSide: "left", waypoints: [{ x: 360, y: 118 }, { x: 360, y: 538 }], labelSegment: 1, labelDx: -80 }],
      ["claims", "managers", "read/apply"],
      ["approval", "persist", "save"],
      ["claims", "persist", "atomic transition", { labelDx: 8, labelDy: 50 }],
      ["approval", "payment", "execute", { labelDx: 50, labelDy: 80 }],
    ],
  },
  registration: {
    title: "Registration",
    kind: "Component",
    viewBox: "0 0 1200 700",
    copy:
      "Registration has two paths: humans create the owner user context that Hubu selects as active, while agents prepare structured identity and version payloads against that owner. The server validates fingerprints before creating or reusing agent records.",
    responsibilities: [
      "Creates human owner users from a small username, display name, and optional email request, then selects that user as the active owner.",
      "Publishes compact agent registration guidance so agents can build envelopes for the current Hubu user context.",
      "Accepts simple agent requests or full envelopes, resolves the owner public id, and rejects mismatched fingerprints.",
      "Creates or reuses agent identity, version, and account records, plus a fresh session per agent registration.",
      "Registration after stack startup changes Hubu state only; it does not rerender, activate, stop, restart, or bind Gongbu startup to that agent.",
    ],
    links: [sharedLinks.user, sharedLinks.registration, sharedLinks.registrationModel, sharedLinks.registrationProtocol, sharedLinks.common],
    zones: [
      { label: "Human registration path", x: 44, y: 46, w: 1090, h: 178 },
      { label: "Agent registration path", x: 44, y: 286, w: 1090, h: 350 },
    ],
    nodes: [
      { id: "humanFields", label: "Human fields", sub: "username + display", x: 84, y: 112, w: 218, h: 88, tone: "human", path: "crates/hubu-cli/src/main.rs" },
      { id: "userManager", label: "User manager", sub: "create + select", x: 436, y: 112, w: 220, h: 88, tone: "core", path: "crates/hubu-core/src/user.rs" },
      { id: "ownerContext", label: "Owner context", sub: "usr_ public id", x: 806, y: 112, w: 230, h: 88, tone: "data", path: "crates/hubu-core/src/user.rs" },
      { id: "guidance", label: "Guidance", sub: ".well-known JSON", x: 84, y: 352, w: 218, h: 88, tone: "agent", path: "docs/agent-registration.md" },
      { id: "review", label: "Human review", sub: "name + version", x: 84, y: 512, w: 218, h: 88, tone: "human", path: "docs/agent-registration.md" },
      { id: "envelope", label: "Envelope", sub: "identity + version", x: 436, y: 430, w: 230, h: 98, tone: "core", path: "docs/agent-registration.md" },
      { id: "fingerprints", label: "Fingerprint check", sub: "canonical SHA-256", x: 806, y: 352, w: 240, h: 92, tone: "core", path: "crates/hubu-api/src/lib.rs" },
      { id: "records", label: "Agent records", sub: "identity/version/account", x: 806, y: 512, w: 250, h: 96, tone: "data", path: "crates/hubu-core/src/registration/manager.rs" },
    ],
    edges: [
      ["humanFields", "userManager", "POST /init"],
      ["userManager", "ownerContext", "create + select"],
      ["guidance", "envelope", "client fills"],
      ["review", "envelope", "approves"],
      ["ownerContext", "envelope", "owner pub_id", { labelDx: -78, labelDy: 4, labelT: 0.58 }],
      ["envelope", "fingerprints", "canonicalize"],
      ["fingerprints", "records", "create/reuse"],
    ],
  },
  policy: {
    title: "Policy Resources & Engine",
    kind: "Component",
    copy:
      "Hubu reconciles owner-scoped policy resources into immutable canonical revisions, assigns them by user default or agent override, and evaluates the selected current revision with deterministic deny-first precedence.",
    responsibilities: [
      "Gives each policy a stable opaque pol_ public id, immutable declarative key, mutable display name, and atomic current-revision pointer.",
      "Canonicalizes and hashes immutable revisions; identical apply is a no-op and optional revision/hash compare-and-set rejects stale writes.",
      "Stores assignments as separate references and migrates embedded legacy assignments without changing their effective content.",
      "Records actor, source, timestamp, old/new hashes, and affected assignments for every mutation.",
      "Validates policy shape before condition evaluation.",
      "Resolves provider, executor, capability, and billing-merchant selectors against a versioned trusted catalog; unknown or ambiguous combinations fail closed.",
      "Evaluates typed condition trees over amount, currency, agent, provider, executor, capability, billing merchant, legacy merchant, and category fields.",
      "Merges matched effects as deny > needs_approval > allow > default.",
    ],
    links: [sharedLinks.persistence, sharedLinks.api, sharedLinks.cli, sharedLinks.unifiedMcp, sharedLinks.executionScope, sharedLinks.scopeModel, sharedLinks.policyEngine, sharedLinks.policyModel, sharedLinks.policyCondition, ["Policy doc", "docs/policy-engine.md"]],
    nodes: [
      { id: "apply", label: "Declarative apply", sub: "validate + CAS", x: 54, y: 80, w: 210, h: 90, tone: "human", path: "crates/hubu-api/src/lib.rs" },
      { id: "resource", label: "Policy resource", sub: "pol_ id + key + name", x: 390, y: 80, w: 218, h: 90, tone: "core", path: "crates/hubu-core/src/persistence.rs" },
      { id: "revisions", label: "Immutable revisions", sub: "number + SHA-256", x: 742, y: 80, w: 228, h: 90, tone: "data", path: "crates/hubu-core/src/persistence.rs" },
      { id: "assignments", label: "Assignments", sub: "default / agent override", x: 742, y: 238, w: 228, h: 90, tone: "data", path: "crates/hubu-core/src/persistence.rs" },
      { id: "request", label: "Scope selector", sub: "provider/executor/capability/billing", x: 54, y: 412, w: 240, h: 90, tone: "agent", path: "crates/hubu-common/src/execution_scope.rs" },
      { id: "validate", label: "Resolve + evaluate", sub: "canonical scope + typed rules", x: 390, y: 412, w: 218, h: 90, tone: "core" },
      { id: "decision", label: "Decision trace", sub: "allow/approval/deny", x: 742, y: 412, w: 228, h: 90, tone: "wallet" },
    ],
    edges: [
      ["apply", "resource", "reconcile"],
      ["resource", "revisions", "append / point"],
      ["resource", "assignments", "reference"],
      ["assignments", "validate", "select current"],
      ["request", "validate", "input"],
      ["validate", "decision", "trace + precedence", { labelDy: -50 }],
    ],
  },
  budget: {
    title: "Budgets & Spending Targets",
    kind: "Component",
    copy:
      "Agent budgets are stable logical allocations whose hard limit lives in an immutable, auditable current version. SQLite stores only active or revoked administrative state; scheduled, expired, exhausted, and effective active availability are derived at one instant. User spending targets remain separate advisory records.",
    responsibilities: [
      "Creates single or finite recurring logical budgets owned by exactly one agent, with immutable currency and half-open period properties.",
      "Creates immutable revision 1 records with effective time, actor, source, optional reason, canonical request fingerprint, and a same-budget current-version pointer.",
      "Appends total-limit changes as one immutable direct successor under BEGIN IMMEDIATE, checks the requested edge for exact replay before stale-head rejection, and compare-and-sets the current pointer with the logical balance in the same transaction.",
      "Applies the repository-authoritative current snapshot to the in-memory manager only after commit; historical exact retries return their stable successor while never rewinding a later head.",
      "Keeps consumed and frozen usage on one logical balance, derives remaining from the current version limit, and attributes every hold to both the logical budget and authorizing version.",
      "Derives availability with fixed precedence revoked, scheduled, expired, exhausted, active; half-open periods are eligible at their start and unavailable at their end.",
      "Allows reservations only while effectively active, while existing version-attributed holds may settle, release, expire, or reconcile after exhaustion, expiry, or revocation without rewriting administrative state.",
          "Treats every non-revoked budget as overlap-blocking for the same agent and currency, allows revocation with outstanding holds, and appends total-cap updates under the same stable logical budget without resetting consumed or frozen usage.",
      "Projects public bgt_ and bgv_ identities through strict GET and POST /budgets/{budget_id}/versions routes; history reports the mutable logical snapshot once and immutable versions in ascending revision order.",
      "Persists user spending targets separately and compares them with the maximum concurrent allocation of overlapping agent budgets.",
      "Returns structured advisory warnings without blocking budget creation or spend.",
      "Keys authorization, claim, and finalization by agent and platform operation key while returning stored state for identical retries.",
      "Stores monotonic immutable authorization attempts and append-only outcomes so exact historical replay and corrected-denial audit survive restart.",
      "Binds the complete canonical provider, executor, capability, and billing-merchant scope through the immutable decision referenced by the authorization token.",
      "Uses one global authorization start window, snapshots the selected Hubu lease profile, and moves executor work from frozen to exclusively claimed for that profile's claim TTL.",
      "Enforces unique agent-scoped operation ownership and finalizes exact receipt, conservative cent charge, claim, token, hold, and budget balance in one immediate SQLite transaction while leaving expired or executor-overrun claims frozen for reconciliation.",
      "Lists expired claimed leases requiring reconciliation for the owning user and requires a server-verified human capability before recording exact cost, frozen pricing snapshot, provider evidence, outcome, actor, and timestamp.",
      "Normal executor settlement ceiling-rounds final exact cost once, consumes no more than the authorized maximum, and returns the unused remainder; after the claim lease expires, a human-confirmed billed overrun consumes the full conservative charge and records the overrun; release returns the full hold.",
      "A future shared allocation would be an explicit budget pool with agent membership, not a task-scoped branch in the MVP budget model.",
    ],
    links: [sharedLinks.budget, sharedLinks.budgetModel, sharedLinks.spendingTarget, sharedLinks.appBudgetUpdate, sharedLinks.appSpend, sharedLinks.appClaims, sharedLinks.spendExecutor, sharedLinks.persistence, ["Budget DTOs", "crates/hubu-core/src/budget/dto.rs"]],
    nodes: [
      { id: "create", label: "Create / update / inspect", sub: "hard + version history", x: 76, y: 76, w: 206, h: 92, tone: "human" },
      { id: "periods", label: "Logical budget", sub: "stable id + CAS head", x: 420, y: 76, w: 210, h: 92, tone: "core" },
      { id: "advisory", label: "Target advisory", sub: "max concurrent allocation", x: 780, y: 76, w: 230, h: 92, tone: "human", path: "crates/hubu-core/src/spending_target.rs" },
      { id: "agentSpend", label: "App service", sub: "authorize operation", x: 76, y: 248, w: 206, h: 92, tone: "core", path: "crates/hubu-core/src/app/spend_approval.rs" },
      { id: "reserve", label: "Reserve hold", sub: "effective active + version", x: 420, y: 248, w: 210, h: 92, tone: "core" },
      { id: "payment", label: "Hubu payment", sub: "success/failure", x: 76, y: 414, w: 206, h: 92, tone: "wallet" },
      { id: "executor", label: "Claim service", sub: "same operation + lease", x: 76, y: 548, w: 238, h: 92, tone: "executor", path: "crates/hubu-core/src/app/executor_claim.rs" },
      { id: "settle", label: "Settle/release", sub: "ceil cents + remainder", x: 420, y: 480, w: 238, h: 92, tone: "core" },
      { id: "store", label: "Governance store", sub: "append + pointer + balance", x: 780, y: 282, w: 230, h: 96, tone: "data" },
      { id: "reconcile", label: "Human reconciliation", sub: "evidence + overrun", x: 780, y: 500, w: 230, h: 96, tone: "human", path: "crates/hubu-core/src/app/executor_claim.rs" },
    ],
    edges: [
      ["create", "periods", "expand"],
      ["periods", "advisory", "compare"],
      ["advisory", "store", "warn"],
      ["periods", "store", "v1 / append + CAS"],
      ["agentSpend", "reserve", "authorize"],
      ["reserve", "store", "freeze on version"],
      ["payment", "settle", "payment", { labelDx: 18, labelDy: -18, labelT: 0.56 }],
      ["reserve", "executor", "claim lease", { labelDx: -36, labelDy: -46 }],
      ["executor", "settle", "receipt", { labelDx: 8, labelDy: 24, labelT: 0.56 }],
      ["executor", "reconcile", "lease expires", { labelDx: 10, labelDy: 50 }],
      ["reconcile", "settle", "billed / not billed", { labelDy: -75 }],
      ["reconcile", "store", "audit receipt"],
      ["settle", "store", "one transaction"],
      ["periods", "reserve", "derive availability"],
    ],
  },
  payment: {
    title: "Payment Manager",
    kind: "Component",
    copy:
      "The wallet boundary receives an app-service-built payment request after allowed spend. It checks request shape and idempotency, validates the spend token through a trait boundary, executes the selected rail, records only successful money movement, and marks tokens used only after ledger success.",
    responsibilities: [
      "Rejects malformed amounts, empty idempotency keys, and conflicting idempotency-key replays.",
      "Returns the original response for an identical idempotency replay without revalidating, rerunning the rail, or writing another ledger transaction.",
      "Validates token, owner, amount, agent, account, complete canonical execution scope, legacy merchant, and currency before rail execution, then resolves task ID and reason from the stored authorization snapshot.",
      "Persists canonical scope JSON with payment attempts so replay remains exact after restart while legacy rows migrate as nullable scope.",
      "Records successful payments in the immutable double-entry ledger, then marks the spend token used.",
      "Returns failed rail responses without ledger writes or token use; the app service persists attempts and decides whether to release holds or keep them frozen for retry.",
    ],
    links: [sharedLinks.payment, sharedLinks.paymentAttempt, sharedLinks.rail, sharedLinks.ledger, ["Spend lifecycle", "docs/spend-lifecycle.md"]],
    nodes: [
      { id: "request", label: "App payment request", sub: "idempotency + token", x: 70, y: 96, w: 238, h: 92, tone: "core", path: "crates/hubu-core/src/app/spend_approval.rs" },
      { id: "idempotency", label: "Idempotency state", sub: "cache + hydrated", x: 70, y: 278, w: 238, h: 92, tone: "data", path: "crates/hubu-wallet/src/persistence.rs" },
      { id: "auth", label: "Spend auth", sub: "scope validation", x: 420, y: 168, w: 220, h: 92, tone: "core" },
      { id: "rail", label: "PaymentRail", sub: "mock fiat/stablecoin", x: 780, y: 168, w: 230, h: 92, tone: "wallet" },
      { id: "ledger", label: "Ledger write", sub: "success only", x: 780, y: 368, w: 230, h: 92, tone: "data" },
      { id: "token", label: "Mark token used", sub: "after ledger", x: 420, y: 368, w: 220, h: 92, tone: "core" },
      { id: "response", label: "PaymentResponse", sub: "succeeded/failed", x: 420, y: 548, w: 220, h: 92, tone: "wallet" },
      { id: "attempts", label: "Attempt store", sub: "retry/restart state", x: 780, y: 548, w: 230, h: 92, tone: "data", path: "crates/hubu-wallet/src/persistence.rs" },
    ],
    edges: [
      ["request", "idempotency", "shape/key"],
      ["idempotency", "auth", "fresh"],
      ["idempotency", "response", "replay", { labelDx: -72, labelDy: 18, labelT: 0.62 }],
      ["auth", "rail", "execute"],
      ["rail", "ledger", "success"],
      ["ledger", "token", "ledger id"],
      ["token", "response", "used", { labelDx: 120, labelDy: -6 }],
      ["rail", "response", "failed", { fromSide: "right", toSide: "right", waypoints: [{ x: 1060, y: 214 }, { x: 1060, y: 500 }, { x: 700, y: 500 }, { x: 700, y: 594 }], labelSegment: 2 }],
      ["response", "attempts", "persist"],
    ],
  },
  gongbu: {
    title: "Gongbu Execution Plane",
    kind: "Runtime component",
    viewBox: "0 0 1340 900",
    copy:
      "Gongbu starts without an execution principal. Its installation caller authenticates the service, while each new Hubu authorization supplies the account and agent snapshot. At startup, Gongbu production-validates any versioned managed provider binding against its exact target, frozen pricing, capability, delivery, polling, and recovery contract. For asynchronous work, Gongbu submits once, checkpoints safe provider-operation evidence in its own SQLite database, and resumes read-only polling without moving credentials or provider payloads into Temporal or Hubu.",
    responsibilities: [
      "Projects active operator-configured targets as opaque stable target IDs with safe provider/model labels, compact Hubu authorization scopes, runtime image-size choices, and exact pricing components; it never returns credentials, endpoints, headers, adapter settings, or configuration revisions.",
      "Accepts one opaque Hubu spend-auth token ID plus execution intent and either a discovered target ID or the legacy explicit tuple through canonical HTTP v2; callers cannot override account, operation identity, money, scope, task metadata, endpoint, or credentials.",
      "Production-validates the managed FLUX binding as provider `flux`, adapter `flux2_api`, pinned non-preview model `flux-2-pro`, exactly one image, PNG/JPEG, certified 1k/2k/4k dimensions, frozen USD prices of 3/1, 45/10, and 75/10 minor units per image, zero generation retries, no fallback, and the fixed polling, artifact-delivery, and durable-recovery policies.",
      "Exposes an authenticated, sanitized provider catalog whose readiness keeps configured, credential-reference-present, production-validated, and live-qualified independent. Catalog reads never resolve secret bytes or call BFL, and the shipped contract remains explicitly not live-qualified pending separate evidence.",
      "For the exact successful HUB-172 tuple only, exposes a bodyless authenticated attestation that revalidates one normalized artifact, scans fixed Gongbu-owned projections with the currently registered key, and returns only versioned safe facts and hashes; it never reads Hubu storage or calls the provider.",
      "Rejects unknown or mixed target selectors, incomplete FLUX pricing profiles, unmatched image-size selectors, and arbitrary or conflicting FLUX dimensions before Hubu resolution, persistence, provider-attempt creation, or provider network activity.",
      "Authenticates one installation/service caller with no account or agent claim; that caller may access known execution IDs and artifacts across the owner's agents, without an owner-wide browse API or strong multi-user/per-agent isolation.",
      "Translates deprecated HTTP v1 only at admission: its two historical token aliases must be equal, never mean decision ID, and cannot broaden any resolved authority.",
      "For new work, first derives and freezes the provider target, normalized input, selector-qualified catalog price, and provider-specific billable dimensions; it then resolves Hubu's authorization snapshot read-only, takes its account and agent as authoritative, exact-matches scope and price, and persists the immutable snapshot before scheduling.",
      "For replay, checks the persisted token and immutable request locally before Hubu resolution, so a claimed or settled token remains replayable without reopening authorization.",
      "Owns workload types for execution classification and target resolution while accepting Hubu's independently selected lease profile and claim deadline.",
      "Accepts only schema-v2 pricing catalogs and freezes the complete exact rational snapshot plus normalized image-size selectors; the pinned non-preview flux-2-pro profile additionally freezes 1k as 1024×1024, 2k as 1920×1088 landscape, or 4k as 2048×2048 before Hubu resolution, and the retired flat pricing shape is rejected.",
      "Persists an immutable execution before scheduling and creates exactly one ProviderAttempt only after preflight and Hubu claim but before provider transmission; replay, worker restart, and activity recovery reuse that attempt and one Hubu financial mutation.",
      "Uses a Temporal patch to preserve deterministic old histories while new asynchronous histories run `submit_provider` followed by `poll_provider_operation`; synchronous adapters retain their existing one-activity behavior.",
      "Sends the FLUX generation POST only from `submit_provider`. A successful submit atomically checkpoints only the safe request ID, operation ID, validated BFL polling host, and original absolute deadline before long polling.",
      "Durably increments poll and artifact-fetch counters before the corresponding transport boundary; failure to record prevents the call, and restart preserves the conservative cumulative evidence.",
      "If transmission may have succeeded but interruption occurs before the checkpoint commits, retains compact safe reconciliation evidence and neither resubmits nor releases; a proven pre-transmission failure remains releasable.",
      "After the checkpoint, `poll_provider_operation` reconstructs status GETs for the same operation and resumes under the original deadline. Restart never sends another generation POST or grants a fresh deadline.",
      "Carries only execution ID and phase enum through Temporal. Credentials, raw provider bodies, complete polling URLs, signed artifact URLs, and storage paths remain outside workflow payloads and the operation checkpoint.",
      "Claims the Hubu authorization before provider work and validates the claim again immediately before the call.",
      "Derives the version-1 canonical execution scope from the agent-selected target in the operator-approved provider catalog and exact-matches it across the Hubu trust boundary.",
      "Resolves Gongbu-held credentials and invokes exactly the agent-selected target from the operator-approved catalog without arbitrary routing or fallback.",
      "A simultaneous Gemini+FLUX catalog preserves separate target revisions and opaque Keychain coordinates; each execution freezes only its selected target and creates its own provider attempt and execution-bound artifact records, so one provider's failure cannot route into the other.",
      "Stores normalized artifacts under the Gongbu artifact root and persists metadata in the Gongbu database, never in Hubu storage.",
      "Sends final exact cost and the full frozen snapshot to Hubu, which recomputes budget cents with checked ceiling and settles only within the normal authorization; ambiguous or over-limit outcomes retain exact attempt evidence and stay in reconciliation instead of causing blind provider resubmission.",
      "Migrates legacy minor-unit attempts and receipts to scale-2 exact values without changing execution, provider-request, pricing-snapshot, or settlement identity; Hubu migrates its own database independently.",
      "Keeps the Hubu and Gongbu processes, databases, credentials, provider work, artifacts, backend interfaces, and failure domains separate despite shared source and release identity.",
    ],
    links: [sharedLinks.gongbuOverview, sharedLinks.gongbuServer, sharedLinks.gongbuProviderConfig, sharedLinks.managedFluxProfile, sharedLinks.stackProviderProfile, sharedLinks.gongbuServerConfig, sharedLinks.gongbuApplication, sharedLinks.gongbuWorkflow, sharedLinks.gongbuTemporal, sharedLinks.gongbuExecution, sharedLinks.gongbuArtifact, sharedLinks.gongbuAttestation, sharedLinks.gongbuProvider, sharedLinks.gongbuPricing, sharedLinks.gongbuSupportedProfiles, sharedLinks.gongbuFlux, sharedLinks.gongbuHubu, sharedLinks.unifiedMcp, sharedLinks.gongbuConfig, sharedLinks.spendExecutor, sharedLinks.executionScope, sharedLinks.api],
    zones: [
      { label: "Gongbu process + owned state", x: 300, y: 44, w: 700, h: 810 },
      { label: "Provider boundary", x: 1025, y: 44, w: 270, h: 410 },
      { label: "Hubu control plane", x: 1025, y: 570, w: 270, h: 284 },
    ],
    nodes: [
      { id: "agent", label: "Agent client", sub: "unified MCP / HTTP", x: 45, y: 125, w: 205, h: 92, tone: "agent", path: "crates/hubu-unified-mcp/src/gongbu/mod.rs" },
      { id: "profile", label: "Managed profile source", sub: "FLUX contract + Gemini target", x: 45, y: 730, w: 205, h: 92, tone: "data", path: "contracts/provider-profiles-v1.json" },
      { id: "gongbuApi", label: "Execution + catalog API", sub: "freeze request; sanitize catalog", x: 330, y: 104, w: 220, h: 92, tone: "executor", path: "crates/gongbu-api/src/http/mod.rs" },
      { id: "workflow", label: "Durable workflow", sub: "preflight → claim → phase → settle", x: 690, y: 104, w: 250, h: 92, tone: "executor", path: "crates/gongbu-api/src/workflow.rs" },
      { id: "executionDb", label: "Gongbu SQLite", sub: "one attempt + safe operation", x: 330, y: 340, w: 240, h: 100, tone: "data", path: "crates/gongbu-api/src/execution/mod.rs" },
      { id: "temporal", label: "Temporal state", sub: "execution ID + phase only", x: 690, y: 250, w: 250, h: 92, tone: "data", path: "crates/gongbu-api/src/temporal.rs" },
      { id: "submit", label: "Submit provider", sub: "submit_provider · POST once", x: 660, y: 420, w: 280, h: 88, tone: "executor", path: "crates/gongbu-api/src/temporal.rs" },
      { id: "poll", label: "Poll existing", sub: "poll_provider_operation · GET", x: 660, y: 550, w: 280, h: 88, tone: "executor", path: "crates/gongbu-api/src/temporal.rs" },
      { id: "artifacts", label: "Artifact store", sub: "normalized bytes", x: 330, y: 570, w: 240, h: 96, tone: "data", path: "crates/gongbu-api/src/artifact/mod.rs" },
      { id: "credentials", label: "Keychain secrets", sub: "resolved inside activities", x: 660, y: 720, w: 280, h: 82, tone: "data", path: "crates/gongbu-api/src/config/secrets.rs" },
      { id: "validator", label: "Profile production validator", sub: "exact target + price + policies", x: 330, y: 740, w: 240, h: 82, tone: "core", path: "crates/gongbu-api/src/provider/supported_profiles.rs" },
      { id: "vendor", label: "Provider", sub: "external model/API", x: 1060, y: 135, w: 200, h: 130, tone: "vendor" },
      { id: "hubu", label: "Hubu trust boundary", sub: "resolve → claim → finalize", x: 1060, y: 660, w: 200, h: 100, tone: "core", path: "crates/hubu-api/src/lib.rs" },
    ],
    edges: [
      ["agent", "gongbuApi", "discover/select + execute", { labelDy: -65 }],
      ["profile", "validator", "rendered bindings", { fromSide: "bottom", toSide: "bottom", waypoints: [{ x: 147, y: 850 }, { x: 450, y: 850 }], labelSegment: 1 }],
      ["validator", "gongbuApi", "validated catalog", { fromSide: "left", toSide: "left", waypoints: [{ x: 280, y: 781 }, { x: 280, y: 150 }], labelSegment: 1, labelDx: -72 }],
      ["gongbuApi", "hubu", "after freeze: attribution", { fromSide: "right", toSide: "top", waypoints: [{ x: 600, y: 150 }, { x: 600, y: 620 }, { x: 1160, y: 620 }], labelSegment: 2, labelDy: -12 }],
      ["gongbuApi", "executionDb", "frozen replay / persist"],
      ["gongbuApi", "workflow", "schedule frozen request"],
      ["workflow", "temporal", "patch + phase"],
      ["workflow", "executionDb", "load durable state", { labelDx: -36, labelDy: 26 }],
      ["workflow", "hubu", "finalize / ceil-settle / reconcile", { fromSide: "right", toSide: "left", waypoints: [{ x: 975, y: 150 }, { x: 975, y: 710 }], labelSegment: 1, labelDx: 120 }],
      ["workflow", "submit", "new history", { fromSide: "left", toSide: "left", waypoints: [{ x: 630, y: 150 }, { x: 630, y: 464 }], labelSegment: 1, labelDx: -68 }],
      ["workflow", "poll", "checkpoint exists", { fromSide: "right", toSide: "right", waypoints: [{ x: 970, y: 150 }, { x: 970, y: 594 }], labelSegment: 1, labelDx: 78 }],
      ["credentials", "submit", "resolve secret", { fromSide: "top", toSide: "bottom", waypoints: [{ x: 960, y: 680 }, { x: 960, y: 530 }, { x: 800, y: 530 }], labelSegment: 1 }],
      ["credentials", "poll", "resolve secret"],
      ["submit", "vendor", "one generation POST", { fromSide: "right", toSide: "left", waypoints: [{ x: 995, y: 464 }, { x: 995, y: 200 }], labelSegment: 1 }],
      ["vendor", "submit", "safe operation ID", { fromSide: "bottom", toSide: "right", waypoints: [{ x: 1160, y: 320 }, { x: 1010, y: 320 }, { x: 1010, y: 464 }], labelSegment: 1 }],
      ["submit", "executionDb", "checkpoint ID + host + deadline", { labelDx: -30, labelDy: 34 }],
      ["executionDb", "poll", "load safe operation", { labelDx: 12, labelDy: -28 }],
      ["poll", "vendor", "status GET only", { fromSide: "right", toSide: "left", waypoints: [{ x: 1010, y: 594 }, { x: 1010, y: 230 }], labelSegment: 1 }],
      ["vendor", "poll", "state / result", { fromSide: "bottom", toSide: "right", waypoints: [{ x: 1180, y: 340 }, { x: 1030, y: 340 }, { x: 1030, y: 594 }], labelSegment: 1 }],
      ["poll", "artifacts", "normalized bytes", { labelDx: -12, labelDy: 34 }],
      ["artifacts", "executionDb", "metadata"],
    ],
  },
  ledger: {
    title: "SQLite Ledger",
    kind: "Component",
    copy:
      "The ledger is the audit record for successful money movement. It stores balanced double-entry transactions and uses SQLite triggers to block mutation.",
    responsibilities: [
      "Creates user wallet cash and agent spend expense accounts.",
      "Requires at least two entries, positive amounts, matching owner scope, and balanced debits/credits.",
      "Prevents updates and deletes for ledger transactions and entries.",
    ],
    links: [sharedLinks.ledger, sharedLinks.payment, ["Wallet persistence", "crates/hubu-wallet/src/persistence.rs"]],
    nodes: [
      { id: "accounts", label: "Accounts", sub: "wallet + expense", x: 90, y: 126, w: 220, h: 92, tone: "wallet" },
      { id: "draft", label: "Entry drafts", sub: "debit + credit", x: 448, y: 126, w: 210, h: 92, tone: "core" },
      { id: "validate", label: "Validate", sub: "owner + balance", x: 804, y: 126, w: 210, h: 92, tone: "core" },
      { id: "tx", label: "Transaction", sub: "external ref", x: 448, y: 366, w: 210, h: 92, tone: "data" },
      { id: "triggers", label: "Immutability", sub: "no update/delete", x: 804, y: 366, w: 210, h: 92, tone: "data" },
    ],
    edges: [
      ["accounts", "draft", "select"],
      ["draft", "validate", "check"],
      ["validate", "tx", "insert"],
      ["tx", "triggers", "protect"],
    ],
  },
  cli: {
    title: "Hubu CLI",
    kind: "Interface",
    viewBox: "0 0 1280 760",
    copy:
      "The CLI is the human developer surface and local-stack launcher. For the managed FLUX profile it turns one small operator selection into an exact versioned contract, reports independent non-network readiness facts, renders immutable provider catalogs, and invokes Gongbu's production validator before activation. It stages updates for explicit activation, reconciles only launcher-owned services in dependency order, configures Codex MCP discovery, and preserves backend ownership boundaries.",
    responsibilities: [
      "Supports profile init, a sanitized provider catalog, doctor, render, generation listing, explicit activation and source-matched rollback, dependency-aware start, component status and logs, and graceful reverse-order whole-stack stop alongside the existing administration commands.",
      "Expands the versioned FLUX contract into the exact `flux`/`flux2_api`/`flux-2-pro` target, one-image PNG/JPEG capability, and 1024×1024, 1920×1088, and 2048×2048 presets frozen at 3/1, 45/10, and 75/10 USD minor units per image, with fixed retry, fallback, poll, artifact, and recovery policies.",
      "Requires only the operator's opaque Keychain credential alias plus explicit maximum-spend and live-spend acknowledgement; mixed Gemini+FLUX source keeps separate credential coordinates and target revisions.",
      "Reports configured, credential-reference-present, production-validated, and live-qualified separately. Doctor and catalog may inspect Keychain item existence without reading it, but never call BFL; the shipped profile remains not live-qualified until a later qualification supplies evidence.",
      "Keeps operator TOML authoritative, stages validated updates without replacing the active manifest, reports redacted affected-component plans, and requires whole-stack stop/activate/start rather than selective restart or repair.",
      "Persists redacted process ownership metadata, validates the recorded start identity before every signal, and never signals external, compatible unowned, or client-owned MCP processes.",
      "Starts the final Hubu once, lets Hubu create its private capabilities, invokes the Gongbu-owned protected handoff, and starts Gongbu only after that succeeds; rollback touches only new child processes and preserves credential state for safe retry.",
      "Renders principal-neutral Gongbu schema v3 without account, agent, or caller-account fields; agent registration after startup requires no lifecycle action.",
      "Ships a one-command acceptance canary that proves no temporary Hubu process or credential leak, then verifies governed deterministic execution, Temporal workflow discovery, artifact retrieval, restart persistence, and graceful shutdown without billable provider spend.",
      "Writes a managed Codex config block that lets agents in other projects discover Hubu MCP tools without reading the Hubu repo.",
      "Builds canonical registration envelopes with the current owner context and fingerprints from server guidance.",
      "Resolves the selected profile's authenticated client handoff lazily and atomically for server-bound commands, while explicit `--url` or the absence of an active profile preserves manual environment/file credential resolution; approval and reconciliation capabilities are sent only on their human mutations.",
    ],
    links: [sharedLinks.cli, sharedLinks.stackProviderProfile, sharedLinks.stackProviderDoctor, sharedLinks.stackLifecycle, sharedLinks.managedCredentialHandoff, sharedLinks.gongbuSupportedProfiles, sharedLinks.managedFluxProfile, sharedLinks.localStack, sharedLinks.localStackAcceptance, sharedLinks.api, sharedLinks.registrationProtocol],
    nodes: [
      { id: "commands", label: "Commands", sub: "init/admin/stack/catalog", x: 35, y: 95, w: 220, h: 92, tone: "human" },
      { id: "profile", label: "Supported profile source", sub: "FLUX contract + credential alias", x: 315, y: 70, w: 240, h: 92, tone: "data", path: "contracts/provider-profiles-v1.json" },
      { id: "doctor", label: "Source doctor", sub: "four independent readiness facts", x: 315, y: 220, w: 240, h: 92, tone: "core", path: "crates/hubu-cli/src/stack/doctor.rs" },
      { id: "render", label: "Immutable render", sub: "target + price + policies", x: 315, y: 370, w: 240, h: 92, tone: "core", path: "crates/hubu-cli/src/stack.rs" },
      { id: "catalog", label: "Sanitized catalogs", sub: "CLI + MCP; no BFL call", x: 35, y: 515, w: 240, h: 92, tone: "surface" },
      { id: "launcher", label: "Lifecycle launcher", sub: "identity + ordering", x: 625, y: 70, w: 230, h: 92, tone: "core", path: "crates/hubu-cli/src/stack/lifecycle.rs" },
      { id: "validator", label: "Gongbu production validator", sub: "exact supported contract", x: 625, y: 370, w: 260, h: 92, tone: "core", path: "crates/gongbu-api/src/provider/supported_profiles.rs" },
      { id: "managedHubu", label: "Final managed Hubu", sub: "creates capabilities once", x: 965, y: 70, w: 250, h: 92, tone: "core", path: "crates/hubu-api/src/lib.rs" },
      { id: "credentialHandoff", label: "Private credential state", sub: "Gongbu-owned handoff", x: 965, y: 220, w: 250, h: 92, tone: "data", path: "crates/gongbu-api/src/config/setup.rs" },
      { id: "managedGongbu", label: "Managed Gongbu", sub: "validated catalog + execution", x: 965, y: 370, w: 250, h: 92, tone: "executor", path: "crates/gongbu-api/src/server.rs" },
      { id: "handoff", label: "Client handoff", sub: "CLI + client-owned MCP", x: 625, y: 535, w: 240, h: 92, tone: "agent" },
    ],
    edges: [
      ["commands", "profile", "configure"],
      ["profile", "doctor", "inspect locally"],
      ["profile", "render", "expand exact contract", { fromSide: "right", toSide: "right", waypoints: [{ x: 585, y: 116 }, { x: 585, y: 416 }], labelSegment: 1, labelDx: 72 }],
      ["doctor", "catalog", "readiness facts", { fromSide: "left", toSide: "right", waypoints: [{ x: 300, y: 266 }, { x: 300, y: 561 }], labelSegment: 1, labelDx: 70 }],
      ["render", "validator", "frozen catalogs"],
      ["validator", "catalog", "production validated", { fromSide: "bottom", toSide: "right", waypoints: [{ x: 755, y: 490 }, { x: 300, y: 490 }, { x: 300, y: 561 }], labelSegment: 1, labelDy: -12 }],
      ["render", "launcher", "active generation", { fromSide: "top", toSide: "bottom", waypoints: [{ x: 435, y: 340 }, { x: 740, y: 340 }], labelSegment: 1, labelDy: -10 }],
      ["launcher", "managedHubu", "start final process"],
      ["managedHubu", "credentialHandoff", "create + verify"],
      ["credentialHandoff", "validator", "bootstrap before serve"],
      ["validator", "managedGongbu", "start only if valid"],
      ["profile", "handoff", "verified post-start refs", { fromSide: "left", toSide: "bottom", waypoints: [{ x: 290, y: 116 }, { x: 290, y: 680 }, { x: 745, y: 680 }], labelSegment: 2 }],
      ["handoff", "commands", "endpoint + capabilities", { fromSide: "bottom", toSide: "left", waypoints: [{ x: 745, y: 700 }, { x: 10, y: 700 }, { x: 10, y: 141 }], labelSegment: 1, labelDy: -12 }],
    ],
  },
  mcp: {
    title: "Unified MCP Surface",
    kind: "Interface",
    viewBox: "0 0 1280 760",
    copy:
      "The agent harness launches one default stdio server. The router offers bounded governed submission plus public-handle resume: primitive resume is Hubu-only, governed resume can continue stored Gongbu intent, and completed operations replay from the local registry. It also routes Gongbu's authenticated, sanitized managed-provider catalog and exact guarded-FLUX attestation without exposing credential coordinates or invoking a provider. Governance, provider execution, backend storage, credentials, artifacts, and failures remain with their owners.",
    responsibilities: [
      "The unified server implements initialize, ping, tools/list, tools/call, startup validation, machine-readable capability snapshots, redacted backend-state errors, serialized list-changed notifications, and bounded monitor shutdown over JSON-RPC stdio.",
      "Starts independent jittered 30-second backend probes only after the initialize/initialized handshake, shares each backend's outage deadline with request refreshes, wakes the monitor when forced recovery shortens a deadline, reuses fresh snapshots for routine calls, and emits exactly one payload-free tools/list_changed event per effective callable-catalog transition.",
      "Configures separate Hubu and Gongbu endpoints, bearer credentials, bounded HTTP clients, and independently probed versioned adapter boundaries without cross-domain Cargo dependencies.",
      "Uses one installation-scoped Gongbu bearer without an account or agent claim; Hubu authorization remains the only source of new-execution attribution.",
      "Coalesces concurrent monitor and request refreshes with independent per-backend single-flight gates whose bookkeeping locks are released before network I/O.",
      "Publishes the accepted gongbu_* catalog, target-discovery, execution, artifact, and read-only attestation primitives, local hubu_operation_status, and router-owned hubu_submit_governed_execution and hubu_resume_operation with stable schemas, private continuation binding, public-handle correlation, and recursive redaction.",
      "Routes `gongbu_get_provider_catalog` as an argument-free read to Gongbu's fixed relative endpoint. Its output carries exact target, preset dimensions, currency, frozen rational prices, policies, and independent configured, credential-reference-present, production-validated, and live-qualified facts; neither MCP readiness nor catalog retrieval calls BFL.",
      "Routes `gongbu_list_execution_targets` as a sanitized read of the active operator-configured targets, safe authorization scopes, runtime image-size options, and exact pricing components without credential or endpoint data.",
      "Routes `gongbu_get_redaction_attestation` as a strict execution-ID-only read; validates bounded counts, money, digest shapes, absence booleans, and the versioned contract before returning Gongbu's safe projection.",
      "For the normal auto-approved path, authorizes with Hubu, binds the immutable execution intent to the same normalized operation, wakes the existing durable worker, and gives the full workflow—including artifact work where possible—a 45-second production response target without implementing another state machine.",
      "Returns approval_required immediately without Gongbu or provider work, persists the bounded immutable intent, synchronizes MCP- or CLI-submitted decisions into status, and requires explicit idempotent public-handle resume before an approval can start execution.",
      "On success, delivers only PNG/JPEG artifacts within an 8 MiB aggregate raw-byte cap (about 10.7 MiB base64) and reports a router envelope alongside overlapping nullable Gongbu execution, provider, and non-provider intervals; it never adds the views together or labels router polling as provider-only time.",
      "Forwards only fixed relative Gongbu API routes and rejects caller attempts to override accounts, endpoints, credentials, retry controls, or artifact storage paths before network access.",
      "Fails closed on unknown or mismatched product, source-commit, executor-contract, MCP, and Gongbu schema versions while preserving healthy unrelated backend capabilities.",
      "Keeps compatible Gongbu target discovery, execution reads, and artifact capabilities available during degraded readiness, but blocks governed execution admission unless both required backend boundaries are safe.",
      "Lists and routes exactly the 31 contract-approved Hubu tools with stable schemas, annotations, validation, trusted metadata, response shapes, and application errors.",
      "Uses fixed Hubu routes plus one strictly validated public budget-version path; the update strips budget_id from its POST body, and only update/history translate recursively redacted typed backend rejections into MCP isError results.",
      "Uses only the Hubu credential for ordinary routes, sends the separate approval capability only on protected approval resolution, and sends the separate reconciliation capability only on the two reconciliation mutations.",
      "Rejects unknown and out-of-map primitive calls before domain network access, never falls back across backends, never retries provider mutations, and limits cross-backend orchestration to the explicit governed-execution contract.",
      "Runs an adapter-owned durable worker that advances accepted, queued, dispatching, reconciling, succeeded, and failed states; it retries only exact idempotent Gongbu create replay and read-only status observation with bounded exponential backoff.",
      "Permits known-ID execution and artifact reads across the owner's agents through that installation caller, but promises neither owner-wide browsing nor strong multi-user/per-agent isolation.",
      "Is the only agent-facing surface written by `hubu init codex` and the only MCP server included in release packaging.",
      "Publishes a generic client approval profile so any harness can auto-approve reads, spend submission, governed submission, and idempotent handle resume but prompt before resolving a needs_approval decision.",
      "Uses Codex per-tool approval overrides as one rendering of that profile: the human first says approve or deny in chat, then the native resolver prompt confirms the call; cancel submits nothing and leaves Hubu pending.",
      "Annotates tools with read-only, destructive, idempotent, open-world, and Hubu approval hints.",
      "Keeps operation_key and task_id out of model-authored spend schemas, normalizes bounded Codex, Claude Code, or controlled Hubu metadata, and injects resolved identities into the HTTP request; trusted task_id remains visible as non-authoritative correlation in sanitized results.",
      "Persists one stable local installation identity and an immutable public operation_handle alongside a canonical tool-and-argument request hash and bounded immutable request intent. By default it allocates a private backend operation key; when the guarded preallocation store is configured, it instead requires exactly one active record for that canonical request, binds that record once, and fails closed before backend access on absence, mismatch, reuse, or store failure.",
      "Keeps trusted Codex callId as operation identity while atomically claiming exact-scope preallocated key material in the owner-only operator SQLite store. The claim binds one stable router registry, call identity, and request hash before backend access. No model-authored argument, MCP response, log, or evidence carries the key or store location, and exact redelivery or restart reuses the durable router binding without reallocating the helper record.",
      "Marks dispatch before Hubu mutation, stores monotonic results, approval status, and sanitized replay state separately from decision and continuation columns, synchronizes external decisions from authoritative Hubu reads, and retains private keys only inside trusted adapter state.",
      "Binds each allowed auth_token_id to exactly one canonical Gongbu create intent before backend access, temporarily persists the validated request for restart-safe replay until durable execution identity is recovered, then deletes the request while retaining Gongbu execution identity and lifecycle state; changed intent, spoofed protected controls, or mismatched returned identity fail closed.",
      "Accepts only the public operation handle through hubu_operation_status and hubu_resume_operation; pending stays approval_required, external approval becomes resume_required, denial is terminal, primitive resume recovers its scoped Hubu outcome, governed resume can only bind the already stored execution intent, and completed outcomes replay from sanitized registry state without backend access.",
      "Recursively removes operation_key fields and private-key text from Gongbu content, structured content, errors, failure messages, artifact metadata, and status projections; allowlisted admission diagnostics survive durable terminal projection and restart, while private operation identity is replaced only by the non-authoritative public handle.",
      "Returns the stable public handle with decision-aware guidance: approved pending work resumes by handle without the original call identity, while a definitive denial translates backend key-reuse guidance into a new harness call and logical operation for corrected work. Migrated v4 pending rows without stored intent require exact original-call backfill before resume or become terminal resume_intent_unavailable.",
      "Treats the registry as an independent billable-operation capability: missing or broken state hides and rejects new Hubu spend calls without stopping the router or affecting Hubu reads, gongbu_get_execution, or artifact access.",
      "Loads the local Hubu bearer and owner capability tokens, returns durable approval status, withholds authorization continuations from resolution responses, and protects approve-or-deny with both the narrow spend-approval client gate and server-verified approval capability without enabling the broader administrative gate.",
    ],
    links: [sharedLinks.unifiedMcp, sharedLinks.unifiedGovernedExecution, sharedLinks.unifiedResumeOperation, sharedLinks.unifiedMcpStdio, sharedLinks.unifiedMcpNotifications, sharedLinks.unifiedHubuCatalog, sharedLinks.unifiedHubuRouting, sharedLinks.unifiedOperationRegistry, sharedLinks.unifiedOperationWorker, sharedLinks.unifiedGongbuCatalog, sharedLinks.unifiedGongbuFixture, sharedLinks.unifiedMcpContract, sharedLinks.operationKeySkill, sharedLinks.operationKeyHelper, sharedLinks.managedFluxProfile, sharedLinks.gongbuSupportedProfiles, sharedLinks.api, sharedLinks.gongbuApplication],
    zones: [
      { label: "hubu-unified-mcp process", x: 286, y: 44, w: 596, h: 670 },
      { label: "Hubu process + failure domain", x: 940, y: 44, w: 292, h: 280 },
      { label: "Gongbu process + failure domain", x: 940, y: 434, w: 292, h: 280 },
    ],
    nodes: [
      { id: "agent", label: "Agent harness", sub: "one stdio connection", x: 30, y: 318, w: 210, h: 96, tone: "agent" },
      { id: "keyStore", label: "Scoped key store", sub: "operator-owned + private", x: 30, y: 566, w: 210, h: 96, tone: "data", path: "skills/generate-hubu-operation-key/scripts/operation_keys.py" },
      { id: "tools", label: "Static router", sub: "42 tools; revision 7 + safe catalogs + attestation", x: 330, y: 92, w: 200, h: 96, tone: "surface", path: "crates/hubu-unified-mcp/src/lib.rs" },
      { id: "notifications", label: "Catalog monitor", sub: "deduped list_changed", x: 330, y: 262, w: 200, h: 96, tone: "surface", path: "crates/hubu-unified-mcp/src/notification.rs" },
      { id: "operationWorker", label: "Durable worker", sub: "safe replay + observe", x: 330, y: 422, w: 200, h: 96, tone: "executor", path: "crates/hubu-unified-mcp/src/operation_worker.rs" },
      { id: "capability", label: "Capability snapshot", sub: "isolated health + compatibility", x: 330, y: 578, w: 200, h: 96, tone: "core", path: "crates/hubu-unified-mcp/src/capability.rs" },
      { id: "hubuClient", label: "Hubu client", sub: "Hubu endpoint + credential", x: 650, y: 170, w: 200, h: 96, tone: "core", path: "crates/hubu-unified-mcp/src/hubu/transport.rs" },
      { id: "operationRegistry", label: "Operation registry", sub: "scoped key + intent + replay", x: 650, y: 334, w: 200, h: 96, tone: "data", path: "crates/hubu-unified-mcp/src/operation_registry.rs" },
      { id: "gongbuClient", label: "Gongbu client", sub: "execution + safe attestation", x: 650, y: 486, w: 200, h: 96, tone: "executor", path: "crates/hubu-unified-mcp/src/gongbu/transport.rs" },
      { id: "approval", label: "Hubu HTTP API", sub: "governance + Hubu SQLite", x: 974, y: 138, w: 224, h: 104, tone: "human", path: "crates/hubu-api/src/lib.rs" },
      { id: "api", label: "Gongbu HTTP API", sub: "catalog + execution + attest", x: 974, y: 528, w: 224, h: 104, tone: "executor", path: "crates/gongbu-api/src/http/mod.rs" },
    ],
    edges: [
      ["agent", "tools", "submit / decide / resume", { labelDy: -54 }],
      ["notifications", "agent", "list_changed", { fromSide: "left", toSide: "right", labelDy: -18 }],
      ["agent", "capability", "status", { labelDy: 48 }],
      ["keyStore", "operationRegistry", "exact scope · atomic claim", { fromSide: "right", toSide: "left", waypoints: [{ x: 260, y: 614 }, { x: 260, y: 382 }], labelSegment: 2, labelDx: -35, labelDy: 26 }],
      ["tools", "hubuClient", "review + resolve + budget versions"],
      ["tools", "operationRegistry", "normalize + replay"],
      ["operationRegistry", "operationWorker", "", { fromSide: "left", toSide: "right" }],
      ["operationWorker", "gongbuClient", "safe create + GET"],
      ["operationRegistry", "hubuClient", "decision sync + private key"],
      ["tools", "gongbuClient", "catalog / reads / attest", { fromSide: "bottom", toSide: "top", waypoints: [{ x: 430, y: 350 }, { x: 750, y: 350 }], labelSegment: 1 }],
      ["capability", "hubuClient", "probe", { fromSide: "top", toSide: "bottom", waypoints: [{ x: 430, y: 410 }, { x: 750, y: 410 }], labelSegment: 1 }],
      ["capability", "gongbuClient", "probe"],
      ["capability", "notifications", "catalog diff"],
      ["hubuClient", "approval", "bounded HTTP"],
      ["gongbuClient", "api", "bounded HTTP"],
    ],
  },
  agent: {
    title: "Agent Spend Path",
    kind: "Flow",
    copy:
      "Agents never hold private backend operation keys. They submit authorization and execution intent once; an auto-allow proceeds immediately, while a pending decision is reviewed, resolved, synchronized, and explicitly resumed by its durable public handle.",
    responsibilities: [
      "Consumes registration guidance instead of guessing protocol fields from prose.",
      "Uses the unified MCP registry for harness spend calls. After an exact human gate, the repository skill may allocate one key-redacted record for the canonical call; the router consumes it privately while the agent keeps the same trusted call identity for replay and restart. Other direct diagnostic CLI flows may still use the skill's separate explicit-key registry.",
      "Lists operator-approved targets, chooses one opaque target ID and an advertised runtime image option, then calls hubu_submit_governed_execution with the returned compact authorization scope and target-bound structured execution intent; optional business task correlation remains trusted client metadata rather than a protected model argument.",
      "Uses the returned public operation handle for status and approved continuation; exact harness-call redelivery also recovers the same normalized operation, while a distinct call ID always allocates a different operation. A migrated pending row without stored intent must be backfilled by that exact redelivery before handle resume or becomes terminal.",
      "If a result is ambiguous, redelivers the exact call with the same harness identity and never submits a replacement spend call.",
      "If authorization is definitively denied, treats that operation as terminal; exact redelivery only recovers the denial, while corrected work is submitted as a new call and receives a new private operation key.",
      "On approval_required, receives an immediate response with no provider work, reads the immutable review, and asks the human to say approve or deny in chat before the native resolver prompt confirms the call.",
      "Treats a canceled native prompt as no submitted decision: Hubu remains pending and the agent never reports cancellation as a denial.",
      "Synchronizes decisions submitted through unified MCP or the CLI; approved work becomes resume_required and only hubu_resume_operation may advance the sticky needs_approval result, replaying stored Hubu intent and binding stored execution intent only for governed work, while original-call redelivery remains replay-only.",
      "Makes an authorization that expires before approved resume terminal and replacement-safe with create-new-operation guidance, without Gongbu or provider work; unrelated or ambiguous Hubu failures keep the same handle resumable.",
      "On in_progress because execution is nonterminal when the total internal budget expires, observes the same durable handle while the existing worker continues; it does not submit a replacement.",
      "On success, receives only eligible bounded PNG/JPEG artifacts plus server-observed timing whose execution wait is not misrepresented as provider-only time.",
    ],
    links: [sharedLinks.unifiedMcp, sharedLinks.cli, sharedLinks.spend, sharedLinks.registrationProtocol, sharedLinks.operationKeySkill, sharedLinks.operationKeyHelper],
    nodes: [
      { id: "register", label: "Register", sub: "identity/session", x: 60, y: 92, w: 220, h: 92, tone: "agent" },
      { id: "policy", label: "User policy", sub: "human-authored", x: 390, y: 92, w: 220, h: 92, tone: "human" },
      { id: "operation", label: "Operation registry", sub: "normalized call + public handle", x: 60, y: 336, w: 220, h: 92, tone: "data", path: "crates/hubu-unified-mcp/src/operation_registry.rs" },
      { id: "submit", label: "Governed submit", sub: "authorization + execution", x: 390, y: 336, w: 220, h: 92, tone: "agent" },
      { id: "decision", label: "Hubu decision", sub: "deny / approval / allow", x: 780, y: 200, w: 230, h: 92, tone: "core" },
      { id: "resume", label: "Handle resume", sub: "approved stored intent", x: 780, y: 338, w: 230, h: 92, tone: "surface" },
      { id: "result", label: "Gongbu result", sub: "handle / artifact + timing", x: 780, y: 476, w: 230, h: 92, tone: "executor" },
    ],
    edges: [
      ["register", "policy", "inherits"],
      ["register", "operation", "agent scope"],
      ["policy", "submit", "governs"],
      ["operation", "submit", "private identity"],
      ["submit", "decision", "Hubu first"],
      ["decision", "resume", "approve"],
      ["resume", "result", "explicit + idempotent"],
    ],
  },
  human: {
    title: "Human Owner Flow",
    kind: "Flow",
    copy:
      "Humans set the financial boundaries. The CLI and MCP adapter aim to keep review small while making identity, policy, advisory target, and hard budget state explicit.",
    responsibilities: [
      "Registers humans with separate username and display name fields.",
      "Reviews current owner context, agent name/version, and protected setup actions.",
      "Funds governance by creating a user-level policy and agent budget before agent spending, with an optional advisory spending target for aggregate allocations.",
      "Reviews every material field, says approve or deny in chat, then confirms the native MCP resolver prompt; cancel leaves the durable request pending rather than recording a denial.",
      "Resolves pending spend explicitly as approve or deny; repeated matching decisions are safe, conflicts are rejected, and resolution itself never invokes a provider.",
    ],
    links: [sharedLinks.cli, sharedLinks.unifiedMcp, sharedLinks.registrationProtocol, sharedLinks.budget],
    nodes: [
      { id: "user", label: "User", sub: "username + public id", x: 90, y: 126, w: 210, h: 92, tone: "human" },
      { id: "review", label: "Review", sub: "chat choice + MCP prompt", x: 430, y: 126, w: 220, h: 92, tone: "human" },
      { id: "policy", label: "Policy", sub: "rules", x: 800, y: 100, w: 210, h: 92, tone: "core" },
      { id: "budget", label: "Budget + target", sub: "hard + advisory", x: 800, y: 334, w: 210, h: 92, tone: "core" },
      { id: "audit", label: "Audit", sub: "ledger/list views", x: 430, y: 454, w: 220, h: 92, tone: "data" },
    ],
    edges: [
      ["user", "review", "approve / deny"],
      ["review", "policy", "attach"],
      ["review", "budget", "create", { fromSide: "right", toSide: "left", waypoints: [{ x: 700, y: 172 }, { x: 700, y: 380 }], labelSegment: 1 }],
      ["policy", "audit", "observe", { fromSide: "right", toSide: "right", waypoints: [{ x: 1050, y: 146 }, { x: 1050, y: 500 }], labelSegment: 1 }],
      ["budget", "audit", "observe", { fromSide: "bottom", toSide: "top", waypoints: [{ x: 905, y: 470 }, { x: 540, y: 470 }], labelSegment: 1 }],
    ],
  },
};

const fillByTone = {
  human: "var(--human)",
  agent: "var(--agent)",
  surface: "var(--surface)",
  core: "var(--core)",
  wallet: "var(--wallet)",
  data: "var(--data)",
  external: "var(--external)",
  executor: "var(--executor)",
  vendor: "var(--vendor)",
};

const sidebarHighlights = {
  top: [
    "Owners set budgets, policies, and approvals.",
    "One governed call can authorize, execute, and deliver an auto-approved result.",
    "A versioned FLUX profile is source-checked, rendered, production-validated, and exposed through sanitized CLI/MCP catalogs without calling BFL.",
    "Hubu authorizes spend; Gongbu executes provider work.",
    "Runtime, data, credential, and failure boundaries stay separate.",
  ],
  release: [
    "An exact version tag and full commit drive one locked four-binary build.",
    "Intel and Apple silicon runners validate the source installer before publication.",
    "Secondary archives retain checksums, manifests, and source provenance.",
  ],
  api: [
    "Bearer authentication protects local HTTP routes.",
    "Routes translate requests and delegate work to app services.",
    "Transport concerns stay outside domain orchestration.",
  ],
  app: [
    "Approval services persist pending decisions and resolve them once.",
    "Claim services coordinate executor settlement and release.",
    "State transitions are persisted atomically.",
  ],
  registration: [
    "Humans establish the active owner context.",
    "Agents submit structured identity and version envelopes.",
    "The server recomputes fingerprints before registration.",
  ],
  policy: [
    "Policies use immutable, hash-addressed revisions.",
    "Assignments select the current user or agent policy.",
    "Evaluation is deterministic and deny-first.",
  ],
  budget: [
    "Agent budgets are hard limits; owner targets are advisory.",
    "Authorization freezes one execution-scoped hold.",
    "Exact costs ceiling-round to cents; overruns wait for human reconciliation.",
  ],
  payment: [
    "Requests validate idempotency, spend token, and scope.",
    "Successful rail execution writes the immutable ledger.",
    "Identical retries return the stored result without paying twice.",
  ],
  gongbu: [
    "Gongbu runs as a separate execution-plane process.",
    "Startup selects no account or agent; Hubu authorization attributes each new execution.",
    "The supported FLUX contract freezes target, dimensions, pricing, policies, and non-live qualification state.",
    "Gemini and FLUX keep distinct credential coordinates, targets, attempts, and execution-bound artifacts.",
    "Persisted token replay is local before Hubu resolution.",
    "It owns workflows, credentials, provider calls, exact receipts, and artifacts.",
  ],
  ledger: [
    "Successful money movement is recorded double-entry.",
    "Every transaction must balance within one owner scope.",
    "Triggers prevent updates and deletes.",
  ],
  cli: [
    "Humans use the CLI for setup, administration, and local stack lifecycle.",
    "Doctor and catalog report four independent readiness facts without reading secrets or calling BFL.",
    "Render expands the exact profile and requires Gongbu production validation before activation.",
    "Humans select sandbox, local-stack, or Hubu-only outcomes before field-level configuration.",
    "The generated topology includes only components required by the selected outcome.",
    "Validated updates stage first and activate only while the owned stack is stopped.",
    "The launcher signals only processes whose recorded start identity still matches.",
    "The acceptance canary proves the real process lifecycle plus deterministic workflow and artifact recovery without billable provider spend.",
    "Agent registration after startup needs no render or restart.",
    "It configures agent-facing MCP access.",
    "It exposes policy, budget, spend, ledger, and health workflows.",
  ],
  mcp: [
    "The agent harness starts one default unified MCP process.",
    "The read-only Gongbu profile catalog exposes exact sanitized contract and readiness data without a provider call.",
    "One explicit composite coordinates Hubu authorization and the existing Gongbu worker.",
    "Approval-required calls return before provider work; nonterminal 45-second budget expiry returns a durable handle.",
    "Separate clients, credentials, probes, and failures preserve backend boundaries.",
    "The Gongbu caller authenticates the installation, not an execution principal.",
    "Its static catalog and routing preserve the versioned public MCP contract.",
  ],
  agent: [
    "The agent receives a public operation handle and never supplies the private backend operation key.",
    "Exact redelivery reuses one normalized operation; a different call ID is always a different operation.",
    "The normal auto-approved path can return artifact and timing in one call.",
    "Human approval and wait-budget expiry return durable continuation states.",
  ],
  human: [
    "The owner initializes and funds the control plane.",
    "Policies and budgets define agent authority.",
    "Pending spend returns for an explicit approve-or-deny review.",
  ],
};

let currentView = "top";

const svg = document.getElementById("architecture-canvas");
const title = document.getElementById("diagram-title");
const crumb = document.getElementById("diagram-crumb");
const detailsTitle = document.getElementById("details-title");
const detailsKind = document.getElementById("details-kind");
const detailsCopy = document.getElementById("details-copy");
const highlights = document.getElementById("highlights");
const responsibilities = document.getElementById("responsibilities");
const sourceLinks = document.getElementById("source-links");
const topButtons = [
  document.getElementById("top-view-button"),
  document.getElementById("details-back-button"),
];

topButtons.forEach((button) => button.addEventListener("click", () => showView("top")));

function showView(viewId) {
  currentView = viewId;
  const view = components[viewId];
  title.textContent = view.title;
  crumb.textContent = view.kind;
  detailsTitle.textContent = view.title;
  detailsKind.textContent = view.kind;
  detailsCopy.textContent = view.copy;
  renderList(highlights, sidebarHighlights[viewId]);
  renderList(responsibilities, view.responsibilities);
  renderSourceLinks(view.links);
  renderDiagram(view);
}

function renderList(list, items) {
  list.innerHTML = "";
  items.forEach((item) => {
    const li = document.createElement("li");
    li.textContent = item;
    list.appendChild(li);
  });
}

function renderSourceLinks(links) {
  sourceLinks.innerHTML = "";
  links.forEach(([label, path]) => {
    const li = document.createElement("li");
    const anchor = document.createElement("a");
    anchor.href = `https://github.com/hacker-no-ice/hubu/blob/main/${path}`;
    anchor.target = "_blank";
    anchor.rel = "noreferrer";
    anchor.textContent = `${label} — ${path}`;
    li.appendChild(anchor);
    sourceLinks.appendChild(li);
  });
}

function renderDiagram(view) {
  svg.innerHTML = "";
  svg.setAttribute("viewBox", view.viewBox || "0 0 1200 700");
  addMarker();
  (view.zones || []).forEach(drawZone);
  const nodesById = Object.fromEntries(view.nodes.map((node) => [node.id, node]));
  view.edges.forEach(([from, to, label, options = {}], index) => {
    drawEdge(nodesById[from], nodesById[to], label, index, options);
  });
  view.nodes.forEach(drawNode);
}

function drawZone(zone) {
  const group = makeSvg("g", { class: "zone" });
  group.appendChild(makeSvg("rect", {
    class: "zone-fill",
    x: zone.x,
    y: zone.y,
    width: zone.w,
    height: zone.h,
    rx: "8",
  }));
  const text = makeSvg("text", {
    class: "zone-label",
    x: zone.labelX || zone.x + 18,
    y: zone.labelY || zone.y + 30,
  });
  text.textContent = zone.label;
  group.appendChild(text);
  svg.appendChild(group);
}

function addMarker() {
  const defs = makeSvg("defs");
  const marker = makeSvg("marker", {
    id: "arrow-tip",
    viewBox: "0 0 10 10",
    refX: "8",
    refY: "5",
    markerWidth: "7",
    markerHeight: "7",
    orient: "auto-start-reverse",
  });
  marker.appendChild(makeSvg("path", { d: "M 0 0 L 10 5 L 0 10 z", fill: "var(--line)" }));
  defs.appendChild(marker);
  svg.appendChild(defs);
}

function drawEdge(from, to, label, index, options = {}) {
  if (options.waypoints) {
    drawRoutedEdge(from, to, label, options);
    return;
  }
  const start = center(from);
  const end = center(to);
  const dx = end.x - start.x;
  const dy = end.y - start.y;
  const horizontal = Math.abs(dx) >= Math.abs(dy);
  const fromPoint = edgePoint(from, horizontal ? Math.sign(dx) : 0, horizontal ? 0 : Math.sign(dy));
  const toPoint = edgePoint(to, horizontal ? -Math.sign(dx) : 0, horizontal ? 0 : -Math.sign(dy));
  const offset = (index % 2 === 0 ? 1 : -1) * 10;
  const points = horizontal
    ? [
        fromPoint,
        { x: (fromPoint.x + toPoint.x) / 2 + offset, y: fromPoint.y },
        { x: (fromPoint.x + toPoint.x) / 2 + offset, y: toPoint.y },
        toPoint,
      ]
    : [
        fromPoint,
        { x: fromPoint.x, y: (fromPoint.y + toPoint.y) / 2 + offset },
        { x: toPoint.x, y: (fromPoint.y + toPoint.y) / 2 + offset },
        toPoint,
      ];
  drawPolylineEdge(points, label, {
    ...options,
    labelPoint: {
      x: (fromPoint.x + toPoint.x) / 2,
      y: (fromPoint.y + toPoint.y) / 2,
    },
  });
}

function drawRoutedEdge(from, to, label, options) {
  const points = [
    edgePointForSide(from, options.fromSide),
    ...options.waypoints,
    edgePointForSide(to, options.toSide),
  ];
  drawPolylineEdge(points, label, options);
}

function drawPolylineEdge(points, label, options = {}) {
  const path = makeSvg("path", {
    class: "arrow-line",
    d: points.map((point, pointIndex) => `${pointIndex === 0 ? "M" : "L"} ${point.x} ${point.y}`).join(" "),
    "marker-end": "url(#arrow-tip)",
  });
  svg.appendChild(path);

  const segmentIndex = options.labelSegment == null
    ? longestSegmentIndex(points)
    : Math.min(options.labelSegment, points.length - 2);
  const segmentStart = points[segmentIndex];
  const segmentEnd = points[segmentIndex + 1];
  const labelPoint = options.labelPoint || {
    x: (segmentStart.x + segmentEnd.x) / 2,
    y: (segmentStart.y + segmentEnd.y) / 2,
  };
  drawEdgeLabel(label, {
    x: labelPoint.x + (options.labelDx || 0),
    y: labelPoint.y - 8 + (options.labelDy || 0),
  });
}

function longestSegmentIndex(points) {
  let longestIndex = 0;
  let longestLength = -1;
  for (let index = 0; index < points.length - 1; index += 1) {
    const length = Math.abs(points[index + 1].x - points[index].x)
      + Math.abs(points[index + 1].y - points[index].y);
    if (length > longestLength) {
      longestLength = length;
      longestIndex = index;
    }
  }
  return longestIndex;
}

function drawEdgeLabel(label, point) {
  const labelWidth = Math.max(58, label.length * 8 + 18);
  svg.appendChild(makeSvg("rect", {
    class: "arrow-label-back",
    x: point.x - labelWidth / 2,
    y: point.y - 17,
    width: labelWidth,
    height: 23,
    rx: "4",
  }));
  const text = makeSvg("text", {
    class: "arrow-label",
    x: point.x,
    y: point.y,
    "text-anchor": "middle",
  });
  text.textContent = label;
  svg.appendChild(text);
}

function drawNode(node) {
  const drillable = Boolean(components[node.id]);
  const attributes = { class: nodeClass(node, drillable) };
  if (drillable) {
    attributes.tabindex = "0";
    attributes.role = "button";
    attributes["aria-label"] = `${node.label}. Select for subsystem details.`;
  }
  const group = makeSvg("g", attributes);
  group.dataset.nodeId = node.id;

  const angle = ((node.x + node.y) % 7) - 3;
  group.setAttribute("transform", `rotate(${angle} ${node.x + node.w / 2} ${node.y + node.h / 2})`);

  drawNodeShape(group, node);

  const label = makeSvg("text", {
    x: node.x + node.w / 2,
    y: node.y + node.h / 2 - 5,
    "text-anchor": "middle",
    "font-size": labelSize(node.label),
  });
  label.textContent = node.label;
  group.appendChild(label);

  const sub = makeSvg("text", {
    class: "subtext",
    x: node.x + node.w / 2,
    y: node.y + node.h / 2 + 24,
    "text-anchor": "middle",
    "font-size": subLabelSize(node.sub),
  });
  sub.textContent = node.sub;
  group.appendChild(sub);

  if (drillable) {
    group.addEventListener("click", () => drill(node.id));
    group.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        drill(node.id);
      }
    });
  }
  svg.appendChild(group);
}

function nodeClass(node, drillable) {
  return [
    "node",
    drillable ? "is-drillable" : "",
    isActorNode(node) ? "actor-node" : "",
    node.tone === "data" ? "storage-node" : "",
    node.tone === "vendor" ? "vendor-node" : "",
    ["external", "executor", "vendor"].includes(node.tone) ? "external-node" : "",
  ].filter(Boolean).join(" ");
}

function drawNodeShape(group, node) {
  if (isActorNode(node)) {
    drawActorShape(group, node);
    return;
  }

  if (node.tone === "vendor") {
    drawVendorShape(group, node);
    return;
  }

  if (node.tone === "data") {
    drawStorageShape(group, node);
    return;
  }

  group.appendChild(makeSvg("rect", {
    class: "node-fill",
    x: node.x,
    y: node.y,
    width: node.w,
    height: node.h,
    rx: "5",
    fill: fillByTone[node.tone],
  }));
  drawUnderline(group, node);
}

function drawActorShape(group, node) {
  const notch = Math.min(28, node.w * 0.15);
  const points = [
    [node.x + notch, node.y],
    [node.x + node.w - notch, node.y],
    [node.x + node.w, node.y + node.h / 2],
    [node.x + node.w - notch, node.y + node.h],
    [node.x + notch, node.y + node.h],
    [node.x, node.y + node.h / 2],
  ].map(([x, y]) => `${x},${y}`).join(" ");
  group.appendChild(makeSvg("polygon", {
    class: "node-fill",
    points,
    fill: fillByTone[node.tone],
  }));
  drawUnderline(group, node);
}

function drawStorageShape(group, node) {
  const capHeight = Math.min(24, node.h * 0.28);
  const bodyTop = node.y + capHeight / 2;
  group.appendChild(makeSvg("path", {
    class: "node-fill",
    d: [
      `M ${node.x} ${bodyTop}`,
      `Q ${node.x + node.w / 2} ${node.y - capHeight / 2} ${node.x + node.w} ${bodyTop}`,
      `L ${node.x + node.w} ${node.y + node.h - capHeight / 2}`,
      `Q ${node.x + node.w / 2} ${node.y + node.h + capHeight / 2} ${node.x} ${node.y + node.h - capHeight / 2}`,
      "Z",
    ].join(" "),
    fill: fillByTone[node.tone],
  }));
  group.appendChild(makeSvg("path", {
    class: "storage-cap",
    d: `M ${node.x} ${bodyTop} Q ${node.x + node.w / 2} ${node.y + capHeight * 1.35} ${node.x + node.w} ${bodyTop}`,
    fill: "none",
  }));
}

function drawVendorShape(group, node) {
  const x = node.x;
  const y = node.y;
  const w = node.w;
  const h = node.h;
  const cloudPath = [
    `M ${x + w * 0.21} ${y + h * 0.72}`,
    `C ${x + w * 0.07} ${y + h * 0.72}, ${x + w * 0.03} ${y + h * 0.52}, ${x + w * 0.17} ${y + h * 0.44}`,
    `C ${x + w * 0.18} ${y + h * 0.23}, ${x + w * 0.39} ${y + h * 0.17}, ${x + w * 0.49} ${y + h * 0.34}`,
    `C ${x + w * 0.61} ${y + h * 0.12}, ${x + w * 0.87} ${y + h * 0.23}, ${x + w * 0.82} ${y + h * 0.48}`,
    `C ${x + w * 0.98} ${y + h * 0.52}, ${x + w * 0.94} ${y + h * 0.75}, ${x + w * 0.78} ${y + h * 0.74}`,
    `L ${x + w * 0.21} ${y + h * 0.72}`,
    "Z",
  ].join(" ");
  group.appendChild(makeSvg("path", {
    class: "node-fill",
    d: cloudPath,
    fill: fillByTone[node.tone],
  }));
}

function drawUnderline(group, node) {
  group.appendChild(makeSvg("path", {
    d: roughUnderline(node.x + 18, node.y + node.h - 18, node.w - 36),
    fill: "none",
    stroke: "rgba(31, 41, 51, 0.35)",
    "stroke-width": "3",
    "stroke-linecap": "round",
  }));
}

function isActorNode(node) {
  return node.tone === "human" || node.tone === "agent";
}

function drill(nodeId) {
  if (components[nodeId]) {
    showView(nodeId);
  }
}

function center(node) {
  return { x: node.x + node.w / 2, y: node.y + node.h / 2 };
}

function edgePoint(node, sideX, sideY) {
  return {
    x: node.x + node.w / 2 + (node.w / 2) * sideX,
    y: node.y + node.h / 2 + (node.h / 2) * sideY,
  };
}

function edgePointForSide(node, side) {
  const sides = {
    top: [0, -1],
    right: [1, 0],
    bottom: [0, 1],
    left: [-1, 0],
  };
  const [sideX, sideY] = sides[side];
  return edgePoint(node, sideX, sideY);
}

function roughUnderline(x, y, width) {
  const middle = x + width / 2;
  return `M ${x} ${y} Q ${middle} ${y + 7}, ${x + width} ${y - 1}`;
}

function labelSize(label) {
  if (label.length > 20) return 19;
  if (label.length > 14) return 21;
  return 24;
}

function subLabelSize(label) {
  if (label.length > 30) return 11;
  if (label.length > 24) return 12;
  return 14;
}

function makeSvg(name, attrs = {}) {
  const element = document.createElementNS("http://www.w3.org/2000/svg", name);
  Object.entries(attrs).forEach(([key, value]) => element.setAttribute(key, value));
  return element;
}

showView("top");
