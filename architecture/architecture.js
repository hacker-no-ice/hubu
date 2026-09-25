const sharedLinks = {
  readme: ["README", "README.md"],
  api: ["Local HTTP API", "crates/hubu-api/src/lib.rs"],
  appSpend: ["Spend approval service", "crates/hubu-core/src/app/spend_approval.rs"],
  appClaims: ["Executor claim service", "crates/hubu-core/src/app/executor_claim.rs"],
  budgetCoordinator: ["Private budget coordinator", "crates/hubu-core/src/budget/coordinator.rs"],
  cli: ["CLI", "crates/hubu-cli/src/main.rs"],
  stackProviderContract: ["Provider contract source", "contracts/provider-contracts-v1.json"],
  stackProviderDoctor: ["Provider contract doctor and catalog", "crates/hubu-cli/src/stack/doctor.rs"],
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
  budgetState: ["Private budget state", "crates/hubu-core/src/budget/state.rs"],
  budgetModel: ["Budget model", "crates/hubu-core/src/budget/model.rs"],
  spendingTarget: ["Spending target model", "crates/hubu-core/src/spending_target.rs"],
  payment: ["Payment manager", "crates/hubu-wallet/src/payment.rs"],
  paymentAttempt: ["Payment attempt store", "crates/hubu-wallet/src/persistence.rs"],
  rail: ["Payment rail", "crates/hubu-wallet/src/rail.rs"],
  ledger: ["Ledger domain", "crates/hubu-ledger/src/domain.rs"],
  providerAccounting: ["Ledger facade", "crates/hubu-core/src/ledger.rs"],
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
  gongbuProviderContracts: ["Provider contract production validator", "crates/gongbu-api/src/provider/provider_contracts.rs"],
  gongbuProviderConfig: ["Provider configuration", "docs/configuration/local-stack/v1/providers-toml.md"],
  liveProviders: ["Live provider operations", "docs/operations/live-providers.md"],
  geminiProviderContract: ["Gemini provider contract runbook", "docs/operations/gemini-provider-contract.md"],
  fluxProviderContract: ["FLUX provider contract runbook", "docs/operations/flux-provider-contract.md"],
  gongbuHubu: ["Gongbu Hubu client", "crates/gongbu-api/src/hubu/mod.rs"],
  feedback: ["Offline feedback preparation", "crates/hubu-feedback/src/lib.rs"],
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

// Step-by-step walkthroughs of one spend request across the top-level diagram.
// Each step lights the listed nodes and [from, to] edges; keep them aligned
// with docs/spend-lifecycle.md and the top-level node and edge IDs.
function spendTraces() {
  const request = [
    {
      nodes: ["agent", "mcp"],
      edges: [["agent", "mcp"]],
      caption: "The agent calls the governed-execution tool on its single unified MCP connection.",
    },
    {
      nodes: ["mcp", "api"],
      edges: [["mcp", "api"]],
      caption: "The router asks Hubu to authorize the spend. The agent never holds payment keys or provider credentials.",
    },
    {
      nodes: ["api", "app"],
      edges: [["api", "app"]],
      caption: "Hubu resolves the trusted execution scope, then evaluates the owner's policy: allow, deny, or needs approval.",
    },
  ];
  const reserve = {
    nodes: ["app", "ledger"],
    edges: [["app", "ledger"]],
    caption: "Policy allows. Hubu freezes a budget hold for the authorized maximum and records the authorization in its own SQLite.",
  };
  const submit = {
    nodes: ["mcp", "gongbu"],
    edges: [["mcp", "gongbu"]],
    caption: "The router submits the authorized work to Gongbu, a separate process with its own credentials and storage.",
  };
  const resolve = {
    nodes: ["gongbu", "api"],
    edges: [["gongbu", "api"]],
    caption: "Gongbu resolves the authorization with Hubu and checks account, amount, and scope against its operator-approved target and price.",
  };
  const execute = {
    nodes: ["gongbu", "workflow"],
    edges: [["gongbu", "workflow"]],
    caption: "Gongbu's durable workflow claims the authorization and calls the provider with credentials only Gongbu holds.",
  };
  return {
    allow: {
      label: "Allowed",
      steps: [
        ...request,
        reserve,
        submit,
        resolve,
        execute,
        {
          nodes: ["workflow", "gongbuData"],
          edges: [["workflow", "gongbuData"]],
          caption: "The exact provider cost, frozen pricing snapshot, and artifacts are checkpointed in Gongbu's own state.",
        },
        {
          nodes: ["gongbu", "api", "app", "ledger"],
          edges: [["gongbu", "api"], ["api", "app"], ["app", "ledger"]],
          caption: "Gongbu settles with the exact receipt. Hubu charges the budget and releases the unused hold in one atomic commit.",
        },
        {
          nodes: ["agent", "mcp"],
          edges: [["agent", "mcp"]],
          caption: "The auto-approved result and its artifact return to the agent in the same call.",
        },
      ],
    },
    deny: {
      label: "Denied",
      steps: [
        ...request,
        {
          nodes: ["app", "ledger"],
          edges: [["app", "ledger"]],
          caption: "Policy denies. Nothing is reserved and no money moves; Hubu records the attempt and its outcome.",
        },
        {
          nodes: ["agent", "mcp", "api"],
          edges: [["mcp", "api"], ["agent", "mcp"]],
          caption: "The denial and its reason return to the agent. Gongbu and the provider are never contacted.",
        },
      ],
    },
    approval: {
      label: "Needs approval",
      steps: [
        ...request,
        {
          nodes: ["app", "ledger"],
          edges: [["app", "ledger"]],
          caption: "Policy needs approval. Hubu persists a pending decision on an immutable review snapshot.",
        },
        {
          nodes: ["agent", "mcp"],
          edges: [["agent", "mcp"]],
          caption: "The call returns before any provider work, with a durable public operation handle.",
        },
        {
          nodes: ["human", "cli", "api", "app"],
          edges: [["human", "cli"], ["cli", "api"]],
          caption: "The owner reviews the snapshot and approves; a denial would end the operation here. Approval alone never starts provider work.",
        },
        {
          nodes: ["agent", "mcp", "api", "app"],
          edges: [["agent", "mcp"], ["mcp", "api"], ["api", "app"]],
          caption: "The agent resumes by public handle before the authorization expires, continuing the same operation.",
        },
        {
          nodes: ["mcp", "gongbu", "workflow", "gongbuData", "api"],
          edges: [["mcp", "gongbu"], ["gongbu", "workflow"], ["workflow", "gongbuData"], ["gongbu", "api"]],
          caption: "From here the approved work runs as in the allowed path: Gongbu executes, then settles with Hubu.",
        },
      ],
    },
    reconcile: {
      label: "Reconciled",
      steps: [
        ...request,
        reserve,
        submit,
        resolve,
        execute,
        {
          nodes: ["workflow", "gongbuData"],
          edges: [["workflow", "gongbuData"]],
          caption: "Billing is ambiguous, or the confirmed cost exceeds the authorized maximum. Gongbu preserves the provider evidence.",
        },
        {
          nodes: ["gongbu", "api", "app", "ledger"],
          edges: [["gongbu", "api"], ["api", "app"], ["app", "ledger"]],
          caption: "Hubu keeps the hold claimed instead of releasing it. A timeout is never treated as a free outcome.",
        },
        {
          nodes: ["human", "cli", "api", "app"],
          edges: [["human", "cli"], ["cli", "api"]],
          caption: "The owner reconciles the operation from the evidence using a separate reconciliation capability.",
        },
      ],
    },
  };
}

const components = {
  top: {
    title: "Major Components",
    kind: "Overview",
    summary: "Agents request paid work through one MCP connection. Hubu decides whether money may be spent; Gongbu, a separate process, does the provider work. The two never share credentials, storage, or failures.",
    viewBox: "0 0 1440 900",
    copy:
      "Agents use one default MCP surface. Its governed-execution tool can authorize, execute, observe, and deliver a normal auto-approved result in one bounded call; its router-owned resume workflow recovers approved primitive Hubu operations or continues stored governed intent by public handle. Provider integrations are frozen in versioned provider contracts and selected publicly only by opaque target IDs, sharing one governance lifecycle across synchronous and asynchronous provider transports. Hubu and Gongbu retain separate credentials, storage, provider work, artifacts, and failure domains.",
    responsibilities: [
      ["Humans set the boundaries", "Owners register, attach policies, create agent budgets, approve or deny pending spend, and reconcile uncertain outcomes."],
      ["One agent surface", "Agents use only the unified MCP server; one governed call can authorize, execute, and return the result."],
      ["Hubu governs money", "Hubu resolves the trusted scope, evaluates policy, reserves budget, then settles or releases it. It holds no provider credentials."],
      ["Gongbu does the work", "Gongbu runs provider calls, retries, artifacts, and recovery in its own process, database, and credential store."],
      ["Separate failure domains", "Hubu and Gongbu share source and releases, but never a process, database, credential, or backend client."],
      ["Exact, replay-safe accounting", "Gongbu reports the exact cost and Hubu charges within the authorized maximum. Retries return stored state; overruns go to a human."],
      ["Provider contracts", "Each provider target is a frozen, versioned contract covering model, pricing, retries, and polling. Agents select it only by opaque ID."],
      ["Three stack modes", "Sandbox and local-stack run Hubu, Gongbu, and Temporal together; Hubu-only runs governance without the execution plane."],
    ],
    links: [sharedLinks.readme, sharedLinks.api, sharedLinks.appSpend, sharedLinks.appClaims, sharedLinks.cli, sharedLinks.stackConfiguration, sharedLinks.stackProviderContract, sharedLinks.stackProviderDoctor, sharedLinks.liveProviders, sharedLinks.localStack, sharedLinks.localStackAcceptance, sharedLinks.gongbuOverview, sharedLinks.gongbuApplication, sharedLinks.gongbuProviderContracts, sharedLinks.unifiedMcp, sharedLinks.unifiedMcpContract, sharedLinks.releases, sharedLinks.releaseWorkflow, sharedLinks.spendExecutor, sharedLinks.executionScope, sharedLinks.scopeModel],
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
      { id: "app", label: "Governance + budgets", sub: "BudgetManager + spend services", x: 1010, y: 132, w: 250, h: 104, tone: "core", path: "crates/hubu-core/src/app/mod.rs" },
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
      ["api", "app", "typed commands"],
      ["app", "ledger", "persist"],
      ["gongbu", "workflow", "execute"],
      ["workflow", "gongbuData", "checkpoint + artifacts"],
      ["gongbu", "api", "resolve attribution + finalize", { fromSide: "top", toSide: "bottom", waypoints: [{ x: 819, y: 458 }, { x: 819, y: 430 }], labelSegment: 1, labelDx: 150 }],
    ],
    traces: spendTraces(),
  },
  release: {
    title: "Immutable Releases",
    kind: "Component",
    summary: "Every release is built from one exact main commit and tag. The source installer is validated on Intel and Apple silicon before anything is published.",
    copy:
      "The release workflow turns one exact main commit into an immutable tag, validates the recommended native source installation on both supported macOS architectures, and publishes secondary archives from the same four-binary workspace build.",
    responsibilities: [
      ["Operator-triggered", "Publishes only on explicit dispatch: a canary, a release candidate, or a stable SemVer release of an exact main commit."],
      ["Gated", "Formatting, Clippy, workspace tests, the core integration flow, and installer tests must pass first."],
      ["Source install first", "The installer builds four binaries from an exact tag and full commit, validated on Intel and Apple silicon."],
      ["Not Apple-signed", "Binaries are staged and verified before install, but are not Developer ID-signed or notarized."],
      ["Archives are secondary", "Archives include licenses, lockfile, provenance, and SHA-256 checksums, and are smoke-tested after download."],
      ["Versions stay separate", "The product version is distinct from the executor contract version, so compatibility is negotiated explicitly."],
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
    summary: "A small local HTTP server that authenticates requests with a bearer token and routes them. Approval, payment, and claim logic lives in core app services, not here.",
    copy:
      "The local server is a small TCP HTTP API. It authenticates protected local requests with a bearer token, owns the shared process state, exposes JSON routes, resolves public IDs, and leaves spend approval, payment, and claim state transitions to core app services.",
    responsibilities: [
      ["Bounded HTTP", "Strictly frames each request and caps header size, body size, and read time."],
      ["Authentication", "Health and guidance are public. Other routes need the bearer token; approval and reconciliation also need separate human capabilities."],
      ["Executors are not humans", "Holding the bearer token never grants approval or reconciliation authority."],
      ["Thin transport", "Parses public IDs and checks ownership, then delegates to `SpendApprovalService`, `ExecutorClaimService`, and `BudgetManager`."],
      ["Idempotent approvals", "Approve and deny are safe to repeat; a conflicting resolution is rejected."],
      ["Atomic finalization", "Receipt, ledger posting, budget charge, claim, and hold commit in one SQLite transaction; identical retries return stored state."],
      ["Startup recovery", "Loads state from SQLite and reconciles expired budget holds on start."],
      ["Bounded logs", "Writes structured JSONL events with fixed-size rotation and skips successful health-probe noise."],
    ],
    links: [sharedLinks.api, sharedLinks.budget, sharedLinks.budgetCoordinator, sharedLinks.appSpend, sharedLinks.appClaims, sharedLinks.spendExecutor, sharedLinks.persistence, sharedLinks.telemetry],
    nodes: [
      { id: "routes", label: "HTTP framing + routes", sub: "bounded GET/POST JSON", x: 72, y: 92, w: 220, h: 90, tone: "agent" },
      { id: "auth", label: "Local auth", sub: "bearer + owner caps", x: 410, y: 76, w: 220, h: 92, tone: "core" },
      { id: "state", label: "ServerState", sub: "shared managers", x: 410, y: 250, w: 220, h: 96, tone: "core" },
      { id: "app", label: "App services", sub: "approval + claims", x: 410, y: 432, w: 220, h: 92, tone: "core", path: "crates/hubu-core/src/app/mod.rs" },
      { id: "registration", label: "Registration", sub: "agent records", x: 805, y: 48, w: 190, h: 84, tone: "core" },
      { id: "governance", label: "Governance DB", sub: "attempts/outcomes/holds", x: 804, y: 180, w: 196, h: 84, tone: "data" },
      { id: "wallet", label: "Wallet", sub: "payment execution", x: 808, y: 310, w: 188, h: 84, tone: "wallet" },
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
    title: "Core Governance",
    kind: "Component",
    summary: "The core use-case layer. It runs spend approvals and executor claims across managers, while BudgetManager owns budget changes, all testable without HTTP.",
    copy:
      "BudgetManager owns budget administration through its private coordinator. The core app layer separately coordinates spend approval and executor claim lifecycles across managers and repositories. These use cases can be tested without HTTP routes.",
    responsibilities: [
      ["Spend approval", "Records each attempt, evaluates policy, and stores allow or deny as final; needs_approval stays pending."],
      ["Resolve once", "Approval issues the scoped token and budget hold; denial creates neither."],
      ["Budget reservation", "Reserves exactly one active agent budget at the request's captured time, then persists the token and hold."],
      ["Payments", "Submits wallet payments, records attempts, and settles, releases, or keeps the hold according to the retry policy."],
      ["Executor claims", "Creates claims, lists work needing reconciliation, and finalizes exact receipts atomically, capped at the authorized maximum."],
      ["Budget administration", "Create, update, and revoke go straight to `BudgetManager`."],
      ["No HTTP concerns", "Returns domain results; the API owns authentication, public IDs, and JSON shape."],
    ],
    links: [sharedLinks.appSpend, sharedLinks.appClaims, sharedLinks.spend, sharedLinks.budget, sharedLinks.budgetCoordinator, sharedLinks.persistence, sharedLinks.payment, sharedLinks.paymentAttempt],
    nodes: [
      { id: "input", label: "Use-case input", sub: "internal IDs + policy", x: 78, y: 112, w: 230, h: 92, tone: "core" },
      { id: "approval", label: "Spend approval", sub: "wait + resolve + pay", x: 410, y: 72, w: 224, h: 92, tone: "core", path: "crates/hubu-core/src/app/spend_approval.rs" },
      { id: "claims", label: "Executor claims", sub: "claim + reconcile", x: 410, y: 282, w: 224, h: 92, tone: "core", path: "crates/hubu-core/src/app/executor_claim.rs" },
      { id: "budget", label: "BudgetManager", sub: "admin + hold accounting", x: 410, y: 492, w: 224, h: 92, tone: "core", path: "crates/hubu-core/src/budget/manager.rs" },
      { id: "persist", label: "Governance store", sub: "exact cost + budget charge", x: 798, y: 188, w: 238, h: 98, tone: "data", path: "crates/hubu-core/src/persistence.rs" },
      { id: "payment", label: "Payment submit", sub: "wallet boundary", x: 798, y: 444, w: 238, h: 98, tone: "wallet", path: "crates/hubu-wallet/src/payment.rs" },
    ],
    edges: [
      ["input", "approval", "authorize"],
      ["input", "claims", "claim/finalize"],
      ["approval", "budget", "evaluate/reserve", { fromSide: "left", toSide: "left", waypoints: [{ x: 360, y: 118 }, { x: 360, y: 538 }], labelSegment: 1, labelDx: -80 }],
      ["claims", "budget", "read/apply"],
      ["approval", "persist", "save"],
      ["claims", "persist", "atomic transition", { labelDx: 8, labelDy: 50 }],
      ["approval", "payment", "execute", { labelDx: 50, labelDy: 80 }],
    ],
  },
  registration: {
    title: "Registration",
    kind: "Component",
    summary: "Humans set up the owner; agents register against that owner with structured identity and version payloads. The server recomputes fingerprints before creating or reusing an agent record.",
    viewBox: "0 0 1200 700",
    copy:
      "Registration has two paths: humans create the owner user context that Hubu selects as active, while agents prepare structured identity and version payloads against that owner. The server validates fingerprints before creating or reusing agent records.",
    responsibilities: [
      ["Human owner", "Created from a username, display name, and optional email, then selected as the active owner."],
      ["Guidance for agents", "Hubu publishes compact guidance so agents build registration envelopes instead of guessing fields."],
      ["Fingerprint check", "The server recomputes fingerprints and rejects mismatches before creating anything."],
      ["Records", "Creates or reuses agent identity, version, and account records, with a fresh session per registration."],
      ["No restarts", "Registering after the stack starts changes only Hubu state; Gongbu needs no rerender or restart."],
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
    summary: "Owner policies become immutable, versioned revisions, assigned per user by default or per agent as an override. Evaluation is deterministic, and deny rules always win.",
    copy:
      "Hubu reconciles owner-scoped policy resources into immutable canonical revisions, assigns them by user default or agent override, and evaluates the selected current revision with deterministic deny-first precedence.",
    responsibilities: [
      ["Stable resources", "Each policy has an opaque `pol_` ID, a fixed key, a display name, and a pointer to its current revision."],
      ["Immutable revisions", "Revisions are canonicalized and hashed. Re-applying the same policy is a no-op, and stale writes are rejected."],
      ["Assignments", "A user default applies unless an agent-level override is assigned."],
      ["Audit trail", "Every change records actor, source, time, old and new hashes, and affected assignments."],
      ["Trusted scope", "Provider, executor, capability, and merchant selectors resolve against a versioned catalog; unknown combinations fail closed."],
      ["Deterministic evaluation", "Typed conditions cover amount, currency, agent, provider, and more. Effects merge as deny > needs_approval > allow > default."],
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
    summary: "Each agent budget is a hard spending limit with an auditable version history. Owner spending targets are separate and advisory only.",
    copy:
      "Agent budgets are stable logical allocations whose hard limit lives in an immutable, auditable current version. SQLite stores only active or revoked administrative state; scheduled, expired, exhausted, and effective active availability are derived at one instant. User spending targets remain separate advisory records.",
    responsibilities: [
      ["One owner of budgets", "`BudgetManager` is the only public way to create, update, revoke, or query budgets."],
      ["Commit, then publish", "Changes commit to SQLite before in-memory state updates, so a failed change leaves memory untouched."],
      ["Hard limits per agent", "Each budget belongs to one agent with a fixed currency and period. Overlapping budgets for the same agent and currency are blocked."],
      ["Versioned limits", "Limit changes append immutable versions. Usage carries over, and every hold records the version that authorized it."],
      ["Availability", "Status is derived at request time: revoked, scheduled, expired, exhausted, or active. Only active budgets accept new holds."],
      ["Settlement", "Charges the exact cost rounded up to cents, capped at the hold, and returns the rest. Release returns the full hold."],
      ["Overruns need a human", "Ambiguous or over-limit outcomes stay frozen until a human with the reconciliation capability records the real cost."],
      ["Replay-safe", "Authorizations, claims, and settlements are keyed by agent and operation; identical retries return stored state."],
      ["Advisory targets", "Owner spending targets are separate records. Exceeding one produces a warning, never a block."],
    ],
    links: [["Budget boundary and lock order", "docs/budget-architecture.md"], sharedLinks.budget, sharedLinks.budgetState, sharedLinks.budgetModel, sharedLinks.spendingTarget, sharedLinks.budgetCoordinator, sharedLinks.appSpend, sharedLinks.appClaims, sharedLinks.spendExecutor, sharedLinks.persistence, ["Budget DTOs", "crates/hubu-core/src/budget/dto.rs"]],
    nodes: [
      { id: "create", label: "Admin commands", sub: "create / update / revoke", x: 36, y: 76, w: 200, h: 92, tone: "human" },
      { id: "periods", label: "BudgetManager", sub: "sole public facade", x: 295, y: 76, w: 210, h: 92, tone: "core", path: "crates/hubu-core/src/budget/manager.rs" },
      { id: "coordinator", label: "Private coordinator", sub: "commit → publish state", x: 565, y: 76, w: 225, h: 92, tone: "core", path: "crates/hubu-core/src/budget/coordinator.rs" },
      { id: "advisory", label: "Target advisory", sub: "max concurrent allocation", x: 845, y: 76, w: 225, h: 92, tone: "human", path: "crates/hubu-core/src/spending_target.rs" },
      { id: "agentSpend", label: "App service", sub: "authorize operation", x: 76, y: 248, w: 206, h: 92, tone: "core", path: "crates/hubu-core/src/app/spend_approval.rs" },
      { id: "reserve", label: "Reserve hold", sub: "effective active + version", x: 420, y: 248, w: 210, h: 92, tone: "core", path: "crates/hubu-core/src/budget/state.rs" },
      { id: "payment", label: "Hubu payment", sub: "success/failure", x: 76, y: 414, w: 206, h: 92, tone: "wallet" },
      { id: "executor", label: "Claim service", sub: "same operation + lease", x: 76, y: 548, w: 238, h: 92, tone: "executor", path: "crates/hubu-core/src/app/executor_claim.rs" },
      { id: "settle", label: "Settle/release", sub: "ceil cents + remainder", x: 420, y: 480, w: 238, h: 92, tone: "core" },
      { id: "store", label: "Governance store", sub: "append + pointer + balance", x: 780, y: 282, w: 230, h: 96, tone: "data" },
      { id: "reconcile", label: "Human reconciliation", sub: "evidence + overrun", x: 780, y: 500, w: 230, h: 96, tone: "human", path: "crates/hubu-core/src/app/executor_claim.rs" },
    ],
    edges: [
      ["create", "periods", "typed inputs"],
      ["periods", "coordinator", "owns"],
      ["create", "advisory", "advisory projection", { fromSide: "top", toSide: "top", waypoints: [{ x: 136, y: 28 }, { x: 957, y: 28 }], labelSegment: 1 }],
      ["coordinator", "store", "serialized transaction"],
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
    summary: "After a spend is allowed, the wallet validates the request and spend token, runs the payment rail, and records only successful money movement. Identical retries never pay twice.",
    copy:
      "The wallet boundary receives an app-service-built payment request after allowed spend. It checks request shape and idempotency, validates the spend token through a trait boundary, executes the selected rail, records only successful money movement, and marks tokens used only after ledger success.",
    responsibilities: [
      ["Input checks", "Rejects malformed amounts, empty idempotency keys, and conflicting replays."],
      ["Idempotent", "An identical replay returns the original response without paying or writing to the ledger again."],
      ["Token validation", "Checks token, owner, amount, agent, account, scope, and currency before running the rail."],
      ["Ledger on success only", "Successful payments are written to the double-entry ledger, then the token is marked used."],
      ["Failures move no money", "A failed payment skips the ledger write and token use; the app service records the attempt and releases the hold or keeps it for retry."],
      ["Restart-safe", "Payment attempts store their full scope, so replay stays exact after a restart."],
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
    summary: "Gongbu runs authorized provider work in its own process, with its own database and credentials. Each Hubu authorization attributes the work, and asynchronous jobs checkpoint so they resume safely.",
    viewBox: "0 0 1340 900",
    copy:
      "Gongbu starts without an execution principal. Its installation caller authenticates the service, while each new Hubu authorization supplies the account and agent snapshot. At startup, Gongbu production-validates any versioned managed provider binding against its exact target, frozen pricing, capability, delivery, polling, and recovery contract. For asynchronous work, Gongbu submits once, checkpoints safe provider-operation evidence in its own SQLite database, and resumes read-only polling without moving credentials or provider payloads into Temporal or Hubu.",
    responsibilities: [
      ["Safe target catalog", "Lists operator-approved targets by opaque ID with labels, pricing, and output options, never credentials or endpoints."],
      ["Narrow requests", "Callers send a Hubu authorization and a target ID; they cannot override account, money, scope, endpoint, or credentials."],
      ["Hubu decides attribution", "Gongbu starts with no account or agent. Each Hubu authorization supplies them, and scope and price must match exactly."],
      ["Validated contracts", "Each managed provider binding is checked against its frozen contract at startup; unknown targets or prices are rejected early."],
      ["Exactly one provider attempt", "Created after the Hubu claim and before transmission; restarts and retries reuse it."],
      ["Durable async work", "Asynchronous jobs are submitted once, checkpointed, and resumed by polling, never resubmitted or given a fresh deadline."],
      ["Uncertain means reconcile", "If a submission may have gone through, Gongbu keeps the evidence and neither resubmits nor releases the budget."],
      ["Secrets stay out of workflows", "Temporal carries only execution ID and phase; credentials, raw provider bodies, and signed URLs never enter it."],
      ["Own storage", "Artifacts and their metadata live in Gongbu's artifact root and database, never in Hubu."],
      ["Exact settlement", "Reports the exact cost and frozen price to Hubu, which settles within the authorization."],
      ["Degrades gracefully", "If Temporal or Hubu is unhealthy, new work is refused while reads and recovery continue."],
      ["Installation-level caller", "One service caller can read known executions across the owner's agents; there is no per-agent isolation."],
    ],
    links: [sharedLinks.gongbuOverview, sharedLinks.gongbuServer, sharedLinks.liveProviders, sharedLinks.gongbuProviderConfig, sharedLinks.fluxProviderContract, sharedLinks.stackProviderContract, sharedLinks.gongbuServerConfig, sharedLinks.gongbuApplication, sharedLinks.gongbuWorkflow, sharedLinks.gongbuTemporal, sharedLinks.gongbuExecution, sharedLinks.gongbuArtifact, sharedLinks.gongbuAttestation, sharedLinks.gongbuProvider, sharedLinks.gongbuPricing, sharedLinks.gongbuProviderContracts, sharedLinks.gongbuFlux, sharedLinks.gongbuHubu, sharedLinks.unifiedMcp, sharedLinks.gongbuConfig, sharedLinks.spendExecutor, sharedLinks.executionScope, sharedLinks.api],
    zones: [
      { label: "Gongbu process + owned state", x: 300, y: 44, w: 700, h: 810 },
      { label: "Provider boundary", x: 1025, y: 44, w: 270, h: 410 },
      { label: "Hubu control plane", x: 1025, y: 570, w: 270, h: 284 },
    ],
    nodes: [
      { id: "agent", label: "Agent client", sub: "unified MCP / HTTP", x: 45, y: 125, w: 205, h: 92, tone: "agent", path: "crates/hubu-unified-mcp/src/gongbu/mod.rs" },
      { id: "contract", label: "Provider contract source", sub: "Gemini + FLUX contracts", x: 45, y: 730, w: 205, h: 92, tone: "data", path: "contracts/provider-contracts-v1.json" },
      { id: "gongbuApi", label: "Execution + catalog API", sub: "freeze request; sanitize catalog", x: 330, y: 104, w: 220, h: 92, tone: "executor", path: "crates/gongbu-api/src/http/mod.rs" },
      { id: "workflow", label: "Durable workflow", sub: "preflight → claim → phase → settle", x: 690, y: 104, w: 250, h: 92, tone: "executor", path: "crates/gongbu-api/src/workflow.rs" },
      { id: "executionDb", label: "Gongbu SQLite", sub: "one attempt + safe operation", x: 330, y: 340, w: 240, h: 100, tone: "data", path: "crates/gongbu-api/src/execution/mod.rs" },
      { id: "temporal", label: "Temporal state", sub: "execution ID + phase only", x: 690, y: 250, w: 250, h: 92, tone: "data", path: "crates/gongbu-api/src/temporal.rs" },
      { id: "submit", label: "Submit provider", sub: "submit_provider · POST once", x: 660, y: 420, w: 280, h: 88, tone: "executor", path: "crates/gongbu-api/src/temporal.rs" },
      { id: "poll", label: "Poll existing", sub: "poll_provider_operation · GET", x: 660, y: 550, w: 280, h: 88, tone: "executor", path: "crates/gongbu-api/src/temporal.rs" },
      { id: "artifacts", label: "Artifact store", sub: "normalized bytes", x: 330, y: 570, w: 240, h: 96, tone: "data", path: "crates/gongbu-api/src/artifact/mod.rs" },
      { id: "credentials", label: "Keychain secrets", sub: "resolved inside activities", x: 660, y: 720, w: 280, h: 82, tone: "data", path: "crates/gongbu-api/src/config/secrets.rs" },
      { id: "validator", label: "Provider contract validator", sub: "exact target + price + policies", x: 330, y: 740, w: 240, h: 82, tone: "core", path: "crates/gongbu-api/src/provider/provider_contracts.rs" },
      { id: "vendor", label: "Provider", sub: "external model/API", x: 1060, y: 135, w: 200, h: 130, tone: "vendor" },
      { id: "hubu", label: "Hubu trust boundary", sub: "resolve → claim → finalize", x: 1060, y: 660, w: 200, h: 100, tone: "core", path: "crates/hubu-api/src/lib.rs" },
    ],
    edges: [
      ["agent", "gongbuApi", "discover/select + execute", { labelDy: -65 }],
      ["contract", "validator", "rendered bindings", { fromSide: "bottom", toSide: "bottom", waypoints: [{ x: 147, y: 850 }, { x: 450, y: 850 }], labelSegment: 1 }],
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
    title: "Hubu Accounting",
    kind: "Component",
    summary: "One double-entry ledger records wallet payments, provider expenses, and adjustments exactly. Budgets are linked for context but not required.",
    copy:
      "The first-class Hubu ledger owns one canonical transaction and exact-entry model for wallet payments, external-provider expenses and adjustments. Budgets are optional linked control context; governed spends retain agent and budget evidence.",
    responsibilities: [
      ["Accounts", "Wallet cash, agent spend expense, and externally billed clearing accounts."],
      ["Balanced postings", "Every transaction has at least two entries, one owner scope, and balanced debits and credits."],
      ["One ledger", "Wallet and provider postings share the same immutable tables, keeping exact precision with cent views for compatibility."],
      ["Clear ownership", "`LedgerService` owns corrections and reads; settlement changes the ledger and budget atomically."],
      ["History", "HTTP, CLI, and MCP read one owner snapshot with agent, account, and budget filters and cursor paging."],
    ],
    links: [sharedLinks.ledger, sharedLinks.providerAccounting, sharedLinks.payment, ["Ledger accounting", "docs/ledger-accounting.md"], ["History contract", "docs/ledger-history.md"], ["Read projections", "crates/hubu-api/src/history.rs"], ["Wallet persistence", "crates/hubu-wallet/src/persistence.rs"]],
    nodes: [
      { id: "providerJournal", label: "Canonical ledger", sub: "wallet + provider + adjustments", x: 70, y: 580, w: 290, h: 92, tone: "data", path: "crates/hubu-ledger/src/domain.rs" },
      { id: "providerBudget", label: "Atomic settlement", sub: "receipt + budget + posting", x: 420, y: 580, w: 260, h: 92, tone: "core", path: "crates/hubu-core/src/persistence.rs" },
      { id: "correction", label: "Ledger facade", sub: "corrections + agent/budget query", x: 790, y: 580, w: 290, h: 92, tone: "core", path: "crates/hubu-core/src/ledger.rs" },
      { id: "accounts", label: "Accounts", sub: "cash + expense + clearing", x: 90, y: 126, w: 220, h: 92, tone: "wallet" },
      { id: "draft", label: "Entry drafts", sub: "debit + credit", x: 448, y: 126, w: 210, h: 92, tone: "core" },
      { id: "validate", label: "Validate", sub: "owner + balance", x: 804, y: 126, w: 210, h: 92, tone: "core" },
      { id: "history", label: "History API / CLI / MCP", sub: "owner + filters + safe evidence", x: 70, y: 366, w: 290, h: 92, tone: "core", path: "crates/hubu-api/src/history.rs" },
      { id: "tx", label: "Transaction", sub: "external ref", x: 448, y: 366, w: 210, h: 92, tone: "data" },
      { id: "triggers", label: "Immutability", sub: "no update/delete", x: 804, y: 366, w: 210, h: 92, tone: "data" },
    ],
    edges: [
      ["accounts", "draft", "select"],
      ["draft", "validate", "check"],
      ["validate", "tx", "insert"],
      ["tx", "triggers", "protect"],
      ["providerBudget", "providerJournal", "commit"],
      ["correction", "providerBudget", "adjust"],
      ["providerJournal", "triggers", "immutable"],
      ["tx", "providerJournal", "same store"],
      ["providerJournal", "history", "read snapshot"],
    ],
  },
  cli: {
    title: "Hubu CLI",
    kind: "Interface",
    summary: "The human's tool for setup, administration, and running the local stack. It validates provider contracts before activation and only manages services it launched.",
    viewBox: "0 0 1280 760",
    copy:
      "The CLI is the human developer surface and local-stack launcher. For the Gemini Lite, Gemini non-Lite, and FLUX provider contracts it renders one explicitly versioned composite catalog, reports independent non-network readiness facts, and invokes Gongbu's production validator before activation. It stages updates for explicit activation, reconciles only launcher-owned services in dependency order, configures Codex MCP discovery, and preserves backend ownership boundaries.",
    responsibilities: [
      ["Stack lifecycle", "`init`, `doctor`, `render`, `activate`, `rollback`, `start`, `status`, `logs`, and `stop`, in dependency order."],
      ["Configuration safety", "Operator TOML is authoritative. Updates are staged and activated only while the stack is stopped."],
      ["Readiness facts", "Reports configured, credential present, validated, and live-qualified separately, without reading secrets or calling providers."],
      ["Live-spend guardrails", "Live provider use requires an explicit spending maximum and acknowledgement."],
      ["Safe process control", "Only signals processes it started, after checking their recorded identity."],
      ["Credential handoff", "Starts Hubu first, hands credentials to Gongbu through a protected step, then starts Gongbu."],
      ["Agent setup", "Writes the Codex MCP configuration and builds registration envelopes from server guidance."],
      ["Acceptance canary", "One command verifies the full stack end to end without billable provider spend."],
    ],
    links: [sharedLinks.feedback, sharedLinks.cli, sharedLinks.stackProviderContract, sharedLinks.stackProviderDoctor, sharedLinks.stackLifecycle, sharedLinks.managedCredentialHandoff, sharedLinks.gongbuProviderContracts, sharedLinks.liveProviders, sharedLinks.fluxProviderContract, sharedLinks.localStack, sharedLinks.localStackAcceptance, sharedLinks.api, sharedLinks.registrationProtocol],
    nodes: [
      { id: "commands", label: "Commands", sub: "init/admin/stack/feedback", x: 35, y: 95, w: 220, h: 92, tone: "human" },
      { id: "contract", label: "Provider contract source", sub: "FLUX contract + credential alias", x: 315, y: 70, w: 240, h: 92, tone: "data", path: "contracts/provider-contracts-v1.json" },
      { id: "doctor", label: "Source doctor", sub: "four independent readiness facts", x: 315, y: 220, w: 240, h: 92, tone: "core", path: "crates/hubu-cli/src/stack/doctor.rs" },
      { id: "render", label: "Immutable render", sub: "target + price + policies", x: 315, y: 370, w: 240, h: 92, tone: "core", path: "crates/hubu-cli/src/stack.rs" },
      { id: "catalog", label: "Sanitized catalogs", sub: "CLI + MCP; no BFL call", x: 35, y: 515, w: 240, h: 92, tone: "surface" },
      { id: "launcher", label: "Lifecycle launcher", sub: "identity + ordering", x: 625, y: 70, w: 230, h: 92, tone: "core", path: "crates/hubu-cli/src/stack/lifecycle.rs" },
      { id: "validator", label: "Gongbu production validator", sub: "exact provider contract", x: 625, y: 370, w: 260, h: 92, tone: "core", path: "crates/gongbu-api/src/provider/provider_contracts.rs" },
      { id: "managedHubu", label: "Final managed Hubu", sub: "creates capabilities once", x: 965, y: 70, w: 250, h: 92, tone: "core", path: "crates/hubu-api/src/lib.rs" },
      { id: "credentialHandoff", label: "Private credential state", sub: "Gongbu-owned handoff", x: 965, y: 220, w: 250, h: 92, tone: "data", path: "crates/gongbu-api/src/config/setup.rs" },
      { id: "managedGongbu", label: "Managed Gongbu", sub: "validated catalog + execution", x: 965, y: 370, w: 250, h: 92, tone: "executor", path: "crates/gongbu-api/src/server.rs" },
      { id: "handoff", label: "Client handoff", sub: "CLI + client-owned MCP", x: 625, y: 535, w: 240, h: 92, tone: "agent" },
    ],
    edges: [
      ["commands", "contract", "configure"],
      ["contract", "doctor", "inspect locally"],
      ["contract", "render", "expand exact contract", { fromSide: "right", toSide: "right", waypoints: [{ x: 585, y: 116 }, { x: 585, y: 416 }], labelSegment: 1, labelDx: 72 }],
      ["doctor", "catalog", "readiness facts", { fromSide: "left", toSide: "right", waypoints: [{ x: 300, y: 266 }, { x: 300, y: 561 }], labelSegment: 1, labelDx: 70 }],
      ["render", "validator", "frozen catalogs"],
      ["validator", "catalog", "production validated", { fromSide: "bottom", toSide: "right", waypoints: [{ x: 755, y: 490 }, { x: 300, y: 490 }, { x: 300, y: 561 }], labelSegment: 1, labelDy: -12 }],
      ["render", "launcher", "active generation", { fromSide: "top", toSide: "bottom", waypoints: [{ x: 435, y: 340 }, { x: 740, y: 340 }], labelSegment: 1, labelDy: -10 }],
      ["launcher", "managedHubu", "start final process"],
      ["managedHubu", "credentialHandoff", "create + verify"],
      ["credentialHandoff", "validator", "bootstrap before serve"],
      ["validator", "managedGongbu", "start only if valid"],
      ["contract", "handoff", "verified post-start refs", { fromSide: "left", toSide: "bottom", waypoints: [{ x: 290, y: 116 }, { x: 290, y: 680 }, { x: 745, y: 680 }], labelSegment: 2 }],
      ["handoff", "commands", "endpoint + capabilities", { fromSide: "bottom", toSide: "left", waypoints: [{ x: 745, y: 700 }, { x: 10, y: 700 }, { x: 10, y: 141 }], labelSegment: 1, labelDy: -12 }],
    ],
  },
  mcp: {
    title: "Unified MCP Surface",
    kind: "Interface",
    summary: "The single server agents connect to. It routes governance calls to Hubu and execution calls to Gongbu, and lets paused or approved operations resume by a public handle.",
    viewBox: "0 0 1280 760",
    copy:
      "The agent harness launches one default stdio server. The router offers bounded governed submission plus public-handle resume: primitive resume is Hubu-only, governed resume can continue stored Gongbu intent, and completed operations replay from the local registry. Agents discover selectable execution targets and pricing; provider-contract diagnostics and guarded-FLUX attestation remain authenticated operator HTTP endpoints. Governance, provider execution, backend storage, credentials, artifacts, and failures remain with their owners.",
    responsibilities: [
      ["The only agent surface", "Written by `hubu init codex`, it is the only MCP server in releases and speaks JSON-RPC over stdio."],
      ["Separate backends", "Hubu and Gongbu have separate endpoints, credentials, clients, and health probes, with no fallback between them."],
      ["Live tool catalog", "Probes backends every 30 seconds and notifies clients once when the callable tool set changes."],
      ["Governed execution", "One call authorizes with Hubu, starts Gongbu work, and aims to return the result within 45 seconds."],
      ["Approvals pause", "`approval_required` returns before any provider work; execution starts only after an explicit resume."],
      ["Operation handles", "Each operation gets a public handle for status and resume. Private operation keys never reach the model, logs, or responses."],
      ["Trusted identity", "Operation and task IDs come from client metadata, not model-written arguments."],
      ["Durable worker", "Advances operations through their states, retrying only idempotent Gongbu creates and read-only status checks."],
      ["Fails closed", "Rejects unknown tools, attempts to override accounts, endpoints, or credentials, and mismatched backend versions."],
      ["Human gates", "Approval and reconciliation capabilities are sent only on those mutations, and tools carry approval annotations."],
      ["Result delivery", "Returns PNG or JPEG artifacts up to 8 MiB, with timing that never labels waiting as provider time."],
    ],
    links: [sharedLinks.feedback, sharedLinks.unifiedMcp, sharedLinks.unifiedGovernedExecution, sharedLinks.unifiedResumeOperation, sharedLinks.unifiedMcpStdio, sharedLinks.unifiedMcpNotifications, sharedLinks.unifiedHubuCatalog, sharedLinks.unifiedHubuRouting, sharedLinks.unifiedOperationRegistry, sharedLinks.unifiedOperationWorker, sharedLinks.unifiedGongbuCatalog, sharedLinks.unifiedGongbuFixture, sharedLinks.unifiedMcpContract, sharedLinks.operationKeySkill, sharedLinks.operationKeyHelper, sharedLinks.liveProviders, sharedLinks.fluxProviderContract, sharedLinks.gongbuProviderContracts, sharedLinks.api, sharedLinks.gongbuApplication],
    zones: [
      { label: "hubu-unified-mcp process", x: 286, y: 44, w: 596, h: 670 },
      { label: "Hubu process + failure domain", x: 940, y: 44, w: 292, h: 280 },
      { label: "Gongbu process + failure domain", x: 940, y: 434, w: 292, h: 280 },
    ],
    nodes: [
      { id: "agent", label: "Agent harness", sub: "one stdio connection", x: 30, y: 318, w: 210, h: 96, tone: "agent" },
      { id: "keyStore", label: "Scoped key store", sub: "operator-owned + private", x: 30, y: 566, w: 210, h: 96, tone: "data", path: "skills/generate-hubu-operation-key/scripts/operation_keys.py" },
      { id: "tools", label: "Static router", sub: "39 tools; revision 12 + target discovery", x: 330, y: 92, w: 200, h: 96, tone: "surface", path: "crates/hubu-unified-mcp/src/lib.rs" },
      { id: "notifications", label: "Catalog monitor", sub: "deduped list_changed", x: 330, y: 262, w: 200, h: 96, tone: "surface", path: "crates/hubu-unified-mcp/src/notification.rs" },
      { id: "operationWorker", label: "Durable worker", sub: "safe replay + observe", x: 330, y: 422, w: 200, h: 96, tone: "executor", path: "crates/hubu-unified-mcp/src/operation_worker.rs" },
      { id: "capability", label: "Capability snapshot", sub: "isolated health + compatibility", x: 330, y: 578, w: 200, h: 96, tone: "core", path: "crates/hubu-unified-mcp/src/capability.rs" },
      { id: "hubuClient", label: "Hubu client", sub: "Hubu endpoint + credential", x: 650, y: 170, w: 200, h: 96, tone: "core", path: "crates/hubu-unified-mcp/src/hubu/transport.rs" },
      { id: "operationRegistry", label: "Operation store", sub: "scoped key + intent + replay", x: 650, y: 334, w: 200, h: 96, tone: "data", path: "crates/hubu-unified-mcp/src/operation_registry.rs" },
      { id: "gongbuClient", label: "Gongbu client", sub: "execution + artifacts", x: 650, y: 486, w: 200, h: 96, tone: "executor", path: "crates/hubu-unified-mcp/src/gongbu/transport.rs" },
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
    summary: "Agents submit a spend and execution request once and never see private backend keys. Auto-approved work proceeds right away; anything needing review pauses and resumes by handle.",
    copy:
      "Agents never hold private backend operation keys. They submit authorization and execution intent once; an auto-allow proceeds immediately, while a pending decision is reviewed, resolved, synchronized, and explicitly resumed by its durable public handle.",
    responsibilities: [
      ["Follows guidance", "Registers from Hubu's guidance object instead of guessing fields."],
      ["Picks a target", "Lists approved targets, chooses one by ID, and submits one governed call with the returned scope."],
      ["Uses the handle", "Tracks status and resumes approved work by public handle; it never sees private operation keys."],
      ["Never double-spends", "On an ambiguous result it redelivers the same call and never submits a replacement."],
      ["Denial is final", "Corrected work goes in as a new call and a new operation."],
      ["Asks the human", "On `approval_required`, it shows the review and asks the human to approve or deny. A canceled prompt is not a denial."],
      ["Expiry ends the operation", "If authorization expires before resume, no provider work runs and a new operation is needed."],
      ["Long jobs keep running", "If the wait budget runs out, it keeps watching the same handle while the worker continues."],
    ],
    links: [sharedLinks.feedback, sharedLinks.unifiedMcp, sharedLinks.cli, sharedLinks.spend, sharedLinks.registrationProtocol, sharedLinks.operationKeySkill, sharedLinks.operationKeyHelper],
    nodes: [
      { id: "register", label: "Register", sub: "identity/session", x: 60, y: 92, w: 220, h: 92, tone: "agent" },
      { id: "policy", label: "User policy", sub: "human-authored", x: 390, y: 92, w: 220, h: 92, tone: "human" },
      { id: "operation", label: "Operation store", sub: "normalized call + public handle", x: 60, y: 336, w: 220, h: 92, tone: "data", path: "crates/hubu-unified-mcp/src/operation_registry.rs" },
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
    summary: "Humans set the financial boundaries: identity, policies, budgets, and approvals. The tools aim to keep each review small and explicit.",
    copy:
      "Humans set the financial boundaries. The CLI and MCP adapter aim to keep review small while making identity, policy, advisory target, and hard budget state explicit.",
    responsibilities: [
      ["Identity", "Registers with a username and a separate display name."],
      ["Funding", "Creates a policy and an agent budget before any spending; spending targets are optional and advisory."],
      ["Review", "Checks owner context, agent name and version, and every material field before protected actions."],
      ["Decide explicitly", "Says approve or deny in chat, then confirms the prompt; canceling leaves the request pending."],
      ["Safe decisions", "Repeating a decision is harmless, conflicting decisions are rejected, and approval never calls a provider by itself."],
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
    "Versioned provider contracts are source-checked, rendered, production-validated, and exposed through sanitized CLI/MCP catalogs without calling the provider.",
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
    "The FLUX provider contract freezes target, dimensions, pricing, policies, and non-live qualification state.",
    "Gemini Lite and non-Lite keep distinct targets while sharing an allowed Google credential; FLUX remains credential-isolated, and every execution keeps distinct attempts and artifacts.",
    "Persisted token replay is local before Hubu resolution.",
    "It owns workflows, credentials, provider calls, exact receipts, and artifacts.",
  ],
  ledger: [
    "Wallet movement + external expenses is recorded double-entry.",
    "Every transaction must balance within one owner scope.",
    "Triggers prevent updates and deletes.",
  ],
  cli: [
    "Checks the product version with `hubu version`; no backend needs to be running.",
    "Manages the local stack: create and validate configuration (`stack init`, `stack doctor`), then start, stop, and check status of the servers.",
    "Runs human admin operations: register user and agent identities, draft, apply, and update policies, and create and list budgets.",
  ],
  mcp: [
    "The agent harness starts one default unified MCP process.",
    "The read-only Gongbu provider contract catalog exposes exact sanitized contract and readiness data without a provider call.",
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

const TRACE_STEP_MS = 2600;
const defaultDocumentTitle = document.title;

let currentView = null;
let focusedNodeId = null;
let layers = null;
let labelPlacement = null;
const trace = { id: null, step: 0, timer: null };

const svg = document.getElementById("architecture-canvas");
const title = document.getElementById("diagram-title");
const crumb = document.getElementById("diagram-crumb");
const detailsTitle = document.getElementById("details-title");
const detailsKind = document.getElementById("details-kind");
const detailsPanel = document.getElementById("details-panel");
const detailsSummary = document.getElementById("details-summary");
const detailsCopy = document.getElementById("details-copy");
const responsibilitiesCount = document.getElementById("responsibilities-count");
const linksCount = document.getElementById("links-count");
const detailSections = document.querySelectorAll(".detail-section");
const highlights = document.getElementById("highlights");
const responsibilities = document.getElementById("responsibilities");
const sourceLinks = document.getElementById("source-links");
const topButtons = [
  document.getElementById("top-view-button"),
  document.getElementById("details-back-button"),
];

const traceBar = document.getElementById("trace-bar");
const traceOutcomes = document.getElementById("trace-outcomes");
const tracePlayer = document.getElementById("trace-player");
const traceCaption = document.getElementById("trace-caption");
const traceCount = document.getElementById("trace-count");
const tracePrev = document.getElementById("trace-prev");
const traceNext = document.getElementById("trace-next");
const tracePlay = document.getElementById("trace-play");
const traceExit = document.getElementById("trace-exit");

topButtons.forEach((button) => button.addEventListener("click", () => navigateTo("top")));
tracePrev.addEventListener("click", () => stepTrace(-1));
traceNext.addEventListener("click", () => stepTrace(1));
tracePlay.addEventListener("click", togglePlay);
traceExit.addEventListener("click", stopTrace);
window.addEventListener("popstate", syncViewFromLocation);
window.addEventListener("hashchange", syncViewFromLocation);
document.addEventListener("keydown", (event) => {
  if (!trace.id || event.target.closest?.("input, textarea, select")) return;
  if (event.key === "ArrowRight") stepTrace(1);
  else if (event.key === "ArrowLeft") stepTrace(-1);
  else if (event.key === "Escape") stopTrace();
  else return;
  event.preventDefault();
});

// Views are addressable as #<view-id> so drill-downs can be linked and the
// browser Back button returns to the previous level.
function viewIdFromLocation() {
  let viewId = "";
  try {
    viewId = decodeURIComponent(window.location.hash.slice(1));
  } catch {
    viewId = "";
  }
  return Object.hasOwn(components, viewId) ? viewId : "top";
}

function syncViewFromLocation() {
  const viewId = viewIdFromLocation();
  if (viewId !== currentView) showView(viewId);
}

function navigateTo(viewId) {
  if (viewId === currentView) return;
  if (viewId === "top") {
    history.pushState(null, "", window.location.pathname + window.location.search);
  } else {
    history.pushState(null, "", `#${encodeURIComponent(viewId)}`);
  }
  showView(viewId);
}

function showView(viewId) {
  currentView = viewId;
  focusedNodeId = null;
  resetTrace();
  const view = components[viewId];
  document.title = viewId === "top" ? defaultDocumentTitle : `${view.title} · ${defaultDocumentTitle}`;
  title.textContent = view.title;
  crumb.textContent = view.kind;
  detailsTitle.textContent = view.title;
  detailsKind.textContent = view.kind;
  setInlineText(detailsSummary, view.summary);
  setInlineText(detailsCopy, view.copy);
  renderList(highlights, sidebarHighlights[viewId]);
  renderList(responsibilities, view.responsibilities);
  responsibilitiesCount.textContent = `(${view.responsibilities.length})`;
  renderSourceLinks(view.links);
  linksCount.textContent = `(${view.links.length})`;
  detailsPanel.scrollTop = 0;
  renderDiagram(view);
  renderTraceControls(view);
}

function renderTraceControls(view) {
  traceOutcomes.innerHTML = "";
  traceBar.hidden = !view.traces;
  if (!view.traces) return;
  Object.entries(view.traces).forEach(([traceId, { label }]) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "trace-outcome";
    button.dataset.traceId = traceId;
    button.textContent = label;
    button.setAttribute("aria-pressed", "false");
    button.addEventListener("click", () => startTrace(traceId));
    traceOutcomes.appendChild(button);
  });
  updateTraceControls();
}

function activeTrace() {
  return trace.id ? components[currentView].traces[trace.id] : null;
}

function startTrace(traceId) {
  resetTrace();
  trace.id = traceId;
  trace.step = 0;
  updateTraceControls();
  updateEmphasis();
  startPlaying();
}

function stepTrace(delta) {
  const active = activeTrace();
  if (!active) return;
  pausePlaying();
  trace.step = Math.min(Math.max(trace.step + delta, 0), active.steps.length - 1);
  updateTraceControls();
  updateEmphasis();
}

function stopTrace() {
  resetTrace();
  updateTraceControls();
  updateEmphasis();
}

function resetTrace() {
  pausePlaying();
  trace.id = null;
  trace.step = 0;
}

function togglePlay() {
  if (trace.timer) {
    pausePlaying();
    updateTraceControls();
    return;
  }
  const active = activeTrace();
  if (active && trace.step >= active.steps.length - 1) {
    trace.step = 0;
    updateEmphasis();
  }
  startPlaying();
}

function startPlaying() {
  pausePlaying();
  trace.timer = window.setInterval(() => {
    const active = activeTrace();
    if (!active || trace.step >= active.steps.length - 1) {
      pausePlaying();
    } else {
      trace.step += 1;
      updateEmphasis();
    }
    updateTraceControls();
  }, TRACE_STEP_MS);
  updateTraceControls();
}

function pausePlaying() {
  window.clearInterval(trace.timer);
  trace.timer = null;
}

function updateTraceControls() {
  const active = activeTrace();
  traceOutcomes.querySelectorAll(".trace-outcome").forEach((button) => {
    button.setAttribute("aria-pressed", String(button.dataset.traceId === trace.id));
  });
  tracePlayer.hidden = !active;
  if (!active) return;
  const lastStep = active.steps.length - 1;
  traceCount.textContent = `Step ${trace.step + 1} of ${active.steps.length}`;
  traceCaption.textContent = active.steps[trace.step].caption;
  tracePrev.disabled = trace.step === 0;
  traceNext.disabled = trace.step === lastStep;
  tracePlay.textContent = trace.timer ? "Pause" : trace.step === lastStep ? "Replay" : "Play";
}

// Dims everything except the lit subgraph: the current trace step when a
// trace is active, otherwise the hovered or focused node and its neighbors.
function updateEmphasis() {
  const active = activeTrace();
  let lit = null;
  if (active) {
    const step = active.steps[trace.step];
    lit = { nodes: new Set(step.nodes), edges: new Set(step.edges.map(([from, to]) => edgeKey(from, to))) };
  } else if (focusedNodeId) {
    lit = neighborhood(focusedNodeId);
  }
  svg.classList.toggle("is-dimmed", Boolean(lit));
  svg.classList.toggle("is-tracing", Boolean(active));
  svg.querySelectorAll("[data-node-id]").forEach((element) => {
    element.classList.toggle("is-lit", Boolean(lit?.nodes.has(element.dataset.nodeId)));
  });
  svg.querySelectorAll("[data-edge]").forEach((element) => {
    element.classList.toggle("is-lit", Boolean(lit?.edges.has(element.dataset.edge)));
  });
}

function neighborhood(nodeId) {
  const nodes = new Set([nodeId]);
  const edges = new Set();
  components[currentView].edges.forEach(([from, to]) => {
    if (from === nodeId || to === nodeId) {
      nodes.add(from);
      nodes.add(to);
      edges.add(edgeKey(from, to));
    }
  });
  return { nodes, edges };
}

function setFocusedNode(nodeId) {
  if (focusedNodeId === nodeId) return;
  focusedNodeId = nodeId;
  if (!trace.id) updateEmphasis();
}

function edgeKey(from, to) {
  return `${from}->${to}`;
}

function renderList(list, items) {
  list.innerHTML = "";
  items.forEach((item) => {
    const li = document.createElement("li");
    if (Array.isArray(item)) {
      // [lead, detail] items render as a bold lead followed by one sentence.
      const [lead, detail] = item;
      const strong = document.createElement("strong");
      setInlineText(strong, lead);
      const text = document.createElement("span");
      setInlineText(text, detail);
      li.append(strong, " ", text);
    } else {
      setInlineText(li, item);
    }
    list.appendChild(li);
  });
}

// Renders `backtick` spans as inline code without interpreting any markup.
function setInlineText(element, text) {
  element.replaceChildren(...text.split("`").map((part, index) => {
    if (index % 2 === 0) return document.createTextNode(part);
    const code = document.createElement("code");
    code.textContent = part;
    return code;
  }));
}

// Collapsible sidebar sections keep their open state across views and, when
// browser storage is available, across visits.
const SECTION_STATE_KEY = "hubu-architecture-sections";

function restoreSectionState() {
  let saved = {};
  try {
    saved = JSON.parse(window.localStorage.getItem(SECTION_STATE_KEY)) || {};
  } catch {
    saved = {};
  }
  detailSections.forEach((section) => {
    section.open = saved[section.dataset.section] === true;
    section.addEventListener("toggle", saveSectionState);
  });
}

function saveSectionState() {
  const state = Object.fromEntries(
    [...detailSections].map((section) => [section.dataset.section, section.open]),
  );
  try {
    window.localStorage.setItem(SECTION_STATE_KEY, JSON.stringify(state));
  } catch {
    // Storage can be unavailable (private windows, blocked site data).
  }
}

function renderSourceLinks(links) {
  sourceLinks.innerHTML = "";
  links.forEach(([label, path]) => {
    const li = document.createElement("li");
    const anchor = document.createElement("a");
    anchor.href = `https://github.com/hacker-no-ice/hubu/blob/main/${path}`;
    anchor.target = "_blank";
    anchor.rel = "noreferrer";
    anchor.textContent = label;
    const pathText = document.createElement("code");
    pathText.className = "source-path";
    pathText.textContent = path;
    li.append(anchor, pathText);
    sourceLinks.appendChild(li);
  });
}

function renderDiagram(view) {
  svg.innerHTML = "";
  svg.setAttribute("viewBox", view.viewBox || "0 0 1200 700");
  addMarker();
  (view.zones || []).forEach(drawZone);
  layers = {
    edges: svg.appendChild(makeSvg("g", { class: "edge-layer" })),
    labels: svg.appendChild(makeSvg("g", { class: "label-layer" })),
    nodes: svg.appendChild(makeSvg("g", { class: "node-layer" })),
  };
  labelPlacement = createLabelPlacement(view);
  const nodesById = Object.fromEntries(view.nodes.map((node) => [node.id, node]));
  view.edges.forEach(([from, to, label, options = {}], index) => {
    drawEdge(nodesById[from], nodesById[to], label, index, { ...options, key: edgeKey(from, to) });
  });
  view.nodes.forEach(drawNode);
  updateEmphasis();
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
  const traceMarker = marker.cloneNode(true);
  traceMarker.id = "arrow-tip-trace";
  traceMarker.firstChild.setAttribute("fill", "var(--trace)");
  defs.appendChild(traceMarker);
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
    "data-edge": options.key,
  });
  layers.edges.appendChild(path);

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
  }, options.key);
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

function drawEdgeLabel(label, anchor, key) {
  const labelWidth = Math.max(58, label.length * 8 + 18);
  const point = labelPlacement.place(anchor, labelWidth);
  const group = makeSvg("g", { class: "edge-label", "data-edge": key });
  if (Math.hypot(point.x - anchor.x, point.y - anchor.y) > LABEL_LEADER_MIN_SHIFT) {
    group.appendChild(makeSvg("line", {
      class: "arrow-label-leader",
      x1: anchor.x,
      y1: anchor.y - LABEL_HEIGHT / 2 + 6,
      x2: point.x,
      y2: point.y - LABEL_HEIGHT / 2 + 6,
    }));
  }
  group.appendChild(makeSvg("rect", {
    class: "arrow-label-back",
    x: point.x - labelWidth / 2,
    y: point.y - LABEL_BASELINE_OFFSET,
    width: labelWidth,
    height: LABEL_HEIGHT,
    rx: "4",
  }));
  const text = makeSvg("text", {
    class: "arrow-label",
    x: point.x,
    y: point.y,
    "text-anchor": "middle",
  });
  text.textContent = label;
  group.appendChild(text);
  layers.labels.appendChild(group);
}

const LABEL_HEIGHT = 23;
const LABEL_BASELINE_OFFSET = 17;
const LABEL_NODE_PADDING = 8;
const LABEL_GAP = 4;
const LABEL_SEARCH_STEP = 6;
const LABEL_SEARCH_RADIUS = 150;
const LABEL_SEARCH_DIRECTIONS = 16;
const LABEL_LEADER_MIN_SHIFT = 22;

// Edge labels start at their authored position and move to the nearest spot
// that clears nodes, zone titles, and earlier labels, so a label in a narrow
// gap is not hidden behind the shapes it connects. Falls back to the
// least-overlapping candidate when no clear spot is within reach.
function createLabelPlacement(view) {
  const [minX, minY, width, height] = (view.viewBox || "0 0 1200 700").split(/\s+/).map(Number);
  const bounds = { x: minX, y: minY, w: width, h: height };
  const obstacles = view.nodes.map((node) => padRect(
    { x: node.x, y: node.y, w: node.w, h: node.h },
    LABEL_NODE_PADDING,
  ));
  svg.querySelectorAll(".zone-label").forEach((text) => {
    const box = text.getBBox();
    if (box.width > 0) obstacles.push(padRect({ x: box.x, y: box.y, w: box.width, h: box.height }, LABEL_GAP));
  });
  const offsets = labelSearchOffsets();

  return {
    place(anchor, labelWidth) {
      let best = null;
      for (const offset of offsets) {
        const point = { x: anchor.x + offset.x, y: anchor.y + offset.y };
        const rect = labelRect(point, labelWidth);
        if (!containsRect(bounds, rect)) continue;
        const overlap = obstacles.reduce((total, obstacle) => total + overlapArea(rect, obstacle), 0);
        if (!best || overlap < best.overlap) best = { point, rect, overlap };
        if (overlap === 0) break;
      }
      const chosen = best || { point: anchor, rect: labelRect(anchor, labelWidth) };
      obstacles.push(padRect(chosen.rect, LABEL_GAP));
      return chosen.point;
    },
  };
}

function labelSearchOffsets() {
  const offsets = [{ x: 0, y: 0 }];
  for (let radius = LABEL_SEARCH_STEP; radius <= LABEL_SEARCH_RADIUS; radius += LABEL_SEARCH_STEP) {
    for (let index = 0; index < LABEL_SEARCH_DIRECTIONS; index += 1) {
      const angle = (index / LABEL_SEARCH_DIRECTIONS) * Math.PI * 2 - Math.PI / 2;
      offsets.push({ x: Math.round(Math.cos(angle) * radius), y: Math.round(Math.sin(angle) * radius) });
    }
  }
  return offsets;
}

function labelRect(point, labelWidth) {
  return { x: point.x - labelWidth / 2, y: point.y - LABEL_BASELINE_OFFSET, w: labelWidth, h: LABEL_HEIGHT };
}

function padRect(rect, padding) {
  return { x: rect.x - padding, y: rect.y - padding, w: rect.w + padding * 2, h: rect.h + padding * 2 };
}

function containsRect(outer, inner) {
  return inner.x >= outer.x && inner.y >= outer.y
    && inner.x + inner.w <= outer.x + outer.w && inner.y + inner.h <= outer.y + outer.h;
}

function overlapArea(a, b) {
  const width = Math.min(a.x + a.w, b.x + b.w) - Math.max(a.x, b.x);
  const height = Math.min(a.y + a.h, b.y + b.h) - Math.max(a.y, b.y);
  return width > 0 && height > 0 ? width * height : 0;
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

  group.addEventListener("mouseenter", () => setFocusedNode(node.id));
  group.addEventListener("mouseleave", () => setFocusedNode(null));
  group.addEventListener("focus", () => setFocusedNode(node.id));
  group.addEventListener("blur", () => setFocusedNode(null));

  if (drillable) {
    group.addEventListener("click", () => drill(node.id));
    group.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        drill(node.id);
      }
    });
  }
  layers.nodes.appendChild(group);
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
    navigateTo(nodeId);
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

restoreSectionState();
syncViewFromLocation();
