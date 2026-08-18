const sharedLinks = {
  readme: ["README", "README.md"],
  api: ["Local HTTP API", "crates/hubu-api/src/lib.rs"],
  appSpend: ["Spend approval service", "crates/hubu-core/src/app/spend_approval.rs"],
  appClaims: ["Executor claim service", "crates/hubu-core/src/app/executor_claim.rs"],
  cli: ["CLI", "crates/hubu-cli/src/main.rs"],
  mcp: ["MCP adapter", "crates/hubu-mcp/src/lib.rs"],
  operationKeySkill: ["Operation-key skill", "skills/generate-hubu-operation-key/SKILL.md"],
  operationKeyHelper: ["Operation-key helper", "skills/generate-hubu-operation-key/scripts/operation_keys.py"],
  common: ["Shared models", "crates/hubu-common/src/lib.rs"],
  user: ["User manager", "crates/hubu-core/src/user.rs"],
  registration: ["Registration manager", "crates/hubu-core/src/registration/manager.rs"],
  registrationModel: ["Registration model", "crates/hubu-core/src/registration/model.rs"],
  registrationProtocol: ["Registration protocol doc", "docs/agent-registration-protocol.md"],
  policyEngine: ["Policy engine", "crates/hubu-core/src/policy/engine.rs"],
  policyModel: ["Policy model", "crates/hubu-core/src/policy/model.rs"],
  policyCondition: ["Policy conditions", "crates/hubu-core/src/policy/condition.rs"],
  spend: ["Spend manager", "crates/hubu-core/src/spend/manager.rs"],
  spendModel: ["Spend model", "crates/hubu-core/src/spend/model.rs"],
  spendExecutor: ["Spend executor contract", "docs/spend-executor-contract.md"],
  futureWallet: ["Future execution modes", "docs/notes/future-wallet-and-credit-use-cases.md"],
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
  releases: ["Release runbook", "docs/releases.md"],
  releaseWorkflow: ["Release workflow", ".github/workflows/release.yml"],
  gongbuOverview: ["Gongbu overview", "docs/gongbu/README.md"],
  gongbuServer: ["Gongbu server runbook", "docs/gongbu/server.md"],
  gongbuApplication: ["Gongbu composition", "crates/gongbu-api/src/application.rs"],
  gongbuWorkflow: ["Gongbu workflow", "crates/gongbu-api/src/workflow.rs"],
  gongbuExecution: ["Gongbu execution store", "crates/gongbu-api/src/execution/mod.rs"],
  gongbuArtifact: ["Gongbu artifact service", "crates/gongbu-api/src/artifact/mod.rs"],
  gongbuProvider: ["Gongbu provider boundary", "crates/gongbu-api/src/provider/mod.rs"],
  gongbuHubu: ["Gongbu Hubu client", "crates/gongbu-api/src/hubu/mod.rs"],
  gongbuMcp: ["Gongbu MCP adapter", "crates/gongbu-mcp/src/lib.rs"],
  gongbuConfig: ["Gongbu server example", "examples/gongbu/gongbu.server.json"],
};

const components = {
  top: {
    title: "System Map",
    kind: "Top level",
    viewBox: "0 0 1200 760",
    copy:
      "Hubu and Gongbu share one source repository, locked workspace, and five-binary release archive. Their binaries remain separate at runtime: Hubu governs spend in the control plane, while Gongbu executes provider work across an authenticated contract.",
    responsibilities: [
      "Humans register, attach user-level policies, optionally set advisory spending targets, create agent budgets, review protected actions, and reconcile uncertain expired claims.",
      "Agents discover Hubu through configured MCP tools, while humans use the CLI for setup and administration.",
      "For local dogfooding, a repository Codex skill allocates a model-managed operation key once, binds it to immutable spend scope, and persists recovery state outside the Hubu server.",
      "The CLI and MCP adapter are part of broader Hubu, but they are not the Hubu server.",
      "Local HTTP callers reach the API with the Hubu bearer token before protected routes resolve user authority.",
      "Release archives contain all five production binaries under one product version and source provenance identity.",
      "Gongbu exclusively claims, then settles actual vendor cost with receipt metadata or releases active authorized spend without Hubu performing provider work; expired uncertainty returns to a human decision.",
      "The API handles local HTTP concerns and delegates spend approval, payment, and executor claim lifecycle orchestration to core app services.",
      "Gongbu owns its process, database, Temporal workflow state, vendor credentials, provider adapters, model calls, artifacts, retries, and failure domain; Hubu stores only governance state and compact provider/artifact references.",
      "SQLite-backed records preserve users, agents, advisory spending targets, budgets, policies, executor claims and receipts, reconciliation evidence, payments, and ledger entries.",
      "The Hubu and Gongbu MCP adapters remain separate agent-facing surfaces; repository and release consolidation does not redesign their protocols.",
    ],
    links: [sharedLinks.readme, sharedLinks.api, sharedLinks.appSpend, sharedLinks.appClaims, sharedLinks.cli, sharedLinks.mcp, sharedLinks.gongbuOverview, sharedLinks.gongbuApplication, sharedLinks.gongbuMcp, sharedLinks.releases, sharedLinks.releaseWorkflow, sharedLinks.spendExecutor],
    zones: [
      { label: "One source repository + locked workspace", x: 292, y: 24, w: 820, h: 716 },
      { label: "Hubu control-plane process", x: 570, y: 36, w: 542, h: 692, labelX: 636, labelY: 86 },
      { label: "Gongbu execution-plane process", x: 304, y: 616, w: 248, h: 112, labelY: 642 },
    ],
    nodes: [
      { id: "human", label: "Human owner", sub: "funds + policy", x: 48, y: 82, w: 190, h: 94, tone: "human" },
      { id: "agent", label: "AI agent", sub: "spend requests", x: 48, y: 330, w: 190, h: 94, tone: "agent" },
      { id: "cli", label: "Hubu CLI", sub: "developer commands", x: 334, y: 82, w: 184, h: 88, tone: "surface" },
      { id: "operationKeys", label: "Operation-key skill", sub: "local recipe + SQLite", x: 334, y: 218, w: 184, h: 76, tone: "data", path: "skills/generate-hubu-operation-key/SKILL.md" },
      { id: "mcp", label: "MCP adapter", sub: "agent tools", x: 334, y: 330, w: 184, h: 88, tone: "surface" },
      { id: "release", label: "Release artifacts", sub: "pinned + checksummed", x: 334, y: 526, w: 184, h: 88, tone: "surface" },
      { id: "api", label: "Local HTTP API", sub: "routes + auth", x: 620, y: 184, w: 190, h: 98, tone: "core" },
      { id: "app", label: "App services", sub: "approval + claims", x: 620, y: 360, w: 190, h: 98, tone: "core", path: "crates/hubu-core/src/app/mod.rs" },
      { id: "gongbu", label: "Gongbu server", sub: "separate executor", x: 328, y: 652, w: 200, h: 62, tone: "executor", path: "crates/gongbu-api/src/bin/gongbu-server.rs" },
      { id: "registration", label: "Registration", sub: "identity + sessions", x: 900, y: 46, w: 202, h: 86, tone: "core" },
      { id: "policy", label: "Policy engine", sub: "deterministic rules", x: 904, y: 166, w: 198, h: 86, tone: "core" },
      { id: "budget", label: "Budgets + targets", sub: "warn + reserve", x: 904, y: 304, w: 198, h: 86, tone: "core" },
      { id: "payment", label: "Payment manager", sub: "rail boundary", x: 904, y: 448, w: 198, h: 86, tone: "wallet" },
      { id: "ledger", label: "SQLite ledger", sub: "double-entry audit", x: 900, y: 626, w: 202, h: 88, tone: "data" },
    ],
    edges: [
      ["human", "cli", "cmd/reconcile"],
      ["agent", "operationKeys", "begin/reuse"],
      ["operationKeys", "mcp", "operation key"],
      ["cli", "api", "token", { labelDx: -12, labelDy: -26, labelT: 0.42 }],
      ["mcp", "api", "token", { labelDx: -14, labelDy: 34, labelT: 0.42 }],
      ["agent", "gongbu", "work + token"],
      ["gongbu", "api", "claim/receipt"],
      ["api", "registration", "register"],
      ["api", "app", "dispatch"],
      ["app", "policy", "evaluate"],
      ["app", "budget", "reserve/settle"],
      ["app", "payment", "submit payment", { labelDx: -20, labelDy: 56, labelT: 0.45 }],
      ["payment", "ledger", "ledger", { labelDx: 54, labelDy: 18, labelT: 0.5 }],
      ["budget", "ledger", "audit", { labelDx: -104, labelDy: 48, labelT: 0.62 }],
    ],
  },
  release: {
    title: "Immutable Releases",
    kind: "Component",
    copy:
      "The release workflow turns one exact main commit into target-specific archives containing all five production binaries from the unified workspace.",
    responsibilities: [
      "Creates a commit-addressed prerelease for each eligible main build and accepts explicit stable SemVer promotion for an exact main revision.",
      "Runs formatting, Clippy, workspace tests, the core integration flow, and locked release builds before publication.",
      "Builds native Linux and macOS archives for x86-64 and ARM64 with hubu, hubu-server, hubu-mcp-server, gongbu-server, gongbu-mcp, licenses, notices, the lockfile, manifest, and per-target provenance.",
      "Preserves separate Hubu and Gongbu runtime boundaries while sharing one product version and source provenance identity.",
      "Publishes SHA-256 checksums without overwriting existing tags or assets, then smoke-tests downloads, legal files, manifests, startup, MCP initialization, and all five version surfaces.",
      "Keeps the Hubu product version separate from the hubu-spend-executor-v4 contract identifier so consumers can negotiate compatibility explicitly.",
    ],
    links: [sharedLinks.releaseWorkflow, sharedLinks.releases, sharedLinks.common, sharedLinks.api, sharedLinks.cli, sharedLinks.mcp, sharedLinks.gongbuApplication, sharedLinks.gongbuMcp],
    nodes: [
      { id: "source", label: "Exact main commit", sub: "40-character SHA", x: 62, y: 224, w: 210, h: 92, tone: "data" },
      { id: "checks", label: "Release gates", sub: "fmt + lint + tests", x: 352, y: 224, w: 210, h: 92, tone: "core" },
      { id: "matrix", label: "Native builds", sub: "macOS + Linux", x: 642, y: 224, w: 210, h: 92, tone: "core" },
      { id: "published", label: "GitHub Release", sub: "archives + SHA-256", x: 928, y: 112, w: 210, h: 92, tone: "data" },
      { id: "smoke", label: "Clean smoke", sub: "download + start", x: 928, y: 356, w: 210, h: 92, tone: "agent" },
      { id: "consumer", label: "Pinned consumer", sub: "tag + checksum", x: 642, y: 510, w: 210, h: 92, tone: "executor" },
    ],
    edges: [
      ["source", "checks", "checkout"],
      ["checks", "matrix", "gate"],
      ["matrix", "published", "publish once"],
      ["published", "smoke", "download"],
      ["smoke", "consumer", "validated pin"],
    ],
  },
  api: {
    title: "Local HTTP API",
    kind: "Component",
    copy:
      "The local server is a small TCP HTTP API. It authenticates protected local requests with a bearer token, owns the shared process state, exposes JSON routes, resolves public IDs, and leaves spend approval, payment, and claim state transitions to core app services.",
    responsibilities: [
      "Frames each request at CRLF-CRLF, validates Content-Length, reads exactly the declared body, and bounds header size, body size, and socket read time.",
      "Keeps health and guidance public while requiring a local bearer token for protected routes and a second human capability for reconciliation mutations.",
      "Uses the local token and current user context for protected workflow authority, while refusing to treat executor possession of that token as human reconciliation approval.",
      "Hydrates state from the configured SQLite path and reconciles expired budget holds at startup.",
      "Delegates authorize/payment to `SpendApprovalService` and claim, lookup, queue selection, settle/release, and reconciliation to `ExecutorClaimService` so both workflows are testable without HTTP.",
      "Bridges wallet payment authorization and durable external executor claims through shared spend and budget state.",
      "Uses one immutable platform operation key as the agent-scoped workflow identity across authorization, claim, and finalization.",
      "Uses SQLite as the finalization authority so receipt, claim, token, hold, and balance commit atomically, settle serializes against release, and identical executor or human reconciliation retries return stored state.",
    ],
    links: [sharedLinks.api, sharedLinks.appSpend, sharedLinks.appClaims, sharedLinks.spendExecutor, sharedLinks.persistence, sharedLinks.telemetry],
    nodes: [
      { id: "routes", label: "HTTP framing + routes", sub: "bounded GET/POST JSON", x: 72, y: 92, w: 220, h: 90, tone: "agent" },
      { id: "auth", label: "Local auth", sub: "bearer + human cap", x: 410, y: 76, w: 220, h: 92, tone: "core" },
      { id: "state", label: "ServerState", sub: "shared managers", x: 410, y: 250, w: 220, h: 96, tone: "core" },
      { id: "app", label: "App services", sub: "approval + claims", x: 410, y: 432, w: 220, h: 92, tone: "core", path: "crates/hubu-core/src/app/mod.rs" },
      { id: "registration", label: "Registration", sub: "agent records", x: 805, y: 48, w: 190, h: 84, tone: "core" },
      { id: "governance", label: "Governance DB", sub: "budget/claims/receipts", x: 804, y: 180, w: 196, h: 84, tone: "data" },
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
      "Evaluates a spend request against the selected policy and persists the immutable spend decision.",
      "Reserves exactly one active agent budget for an allowed spend decision.",
      "Persists the spend auth token and frozen budget hold after the budget accepts the request.",
      "Submits wallet payments, persists payment attempts, marks successful tokens used, and settles, releases, or keeps the hold frozen according to the failed-payment retry policy.",
      "Creates and looks up executor claims, derives the expired reconciliation queue, and coordinates receipt-backed executor or human finalization through one atomic repository boundary.",
      "Returns domain-shaped approval, rejection, payment, and claim state while the API owns authentication, public IDs, and JSON response shape.",
    ],
    links: [sharedLinks.appSpend, sharedLinks.appClaims, sharedLinks.spend, sharedLinks.budget, sharedLinks.persistence, sharedLinks.payment, sharedLinks.paymentAttempt],
    nodes: [
      { id: "input", label: "Use-case input", sub: "internal IDs + policy", x: 78, y: 112, w: 230, h: 92, tone: "core" },
      { id: "approval", label: "Spend approval", sub: "authorize + payment", x: 410, y: 72, w: 224, h: 92, tone: "core", path: "crates/hubu-core/src/app/spend_approval.rs" },
      { id: "claims", label: "Executor claims", sub: "claim + reconcile", x: 410, y: 282, w: 224, h: 92, tone: "core", path: "crates/hubu-core/src/app/executor_claim.rs" },
      { id: "managers", label: "Domain managers", sub: "spend + budget", x: 410, y: 492, w: 224, h: 92, tone: "core", path: "crates/hubu-core/src/spend/manager.rs" },
      { id: "persist", label: "Governance store", sub: "claims + receipts", x: 798, y: 188, w: 238, h: 98, tone: "data", path: "crates/hubu-core/src/persistence.rs" },
      { id: "payment", label: "Payment submit", sub: "wallet boundary", x: 798, y: 444, w: 238, h: 98, tone: "wallet", path: "crates/hubu-wallet/src/payment.rs" },
    ],
    edges: [
      ["input", "approval", "authorize"],
      ["input", "claims", "claim/finalize"],
      ["approval", "managers", "evaluate/reserve"],
      ["claims", "managers", "read/apply"],
      ["approval", "persist", "save"],
      ["claims", "persist", "atomic transition", { labelDx: 8, labelDy: -22, labelT: 0.54 }],
      ["approval", "payment", "execute", { labelDx: 50, labelDy: 34, labelT: 0.62 }],
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
      { id: "guidance", label: "Guidance", sub: ".well-known JSON", x: 84, y: 352, w: 218, h: 88, tone: "agent", path: "docs/agent-registration-protocol.md" },
      { id: "review", label: "Human review", sub: "name + version", x: 84, y: 512, w: 218, h: 88, tone: "human", path: "docs/agent-registration-protocol.md" },
      { id: "envelope", label: "Envelope", sub: "identity + version", x: 436, y: 430, w: 230, h: 98, tone: "core", path: "docs/agent-registration-protocol.md" },
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
    title: "Policy Engine",
    kind: "Component",
    copy:
      "The policy engine evaluates one structured spend request against one validated policy and returns a decision trace. Matching rule effects merge with deny-first precedence.",
    responsibilities: [
      "Validates policy shape before condition evaluation.",
      "Evaluates typed condition trees over amount, currency, agent, merchant, and category fields.",
      "Merges matched effects as deny > needs_approval > allow > default.",
    ],
    links: [sharedLinks.policyEngine, sharedLinks.policyModel, sharedLinks.policyCondition, ["Policy doc", "docs/policy-engine.md"]],
    nodes: [
      { id: "request", label: "SpendRequest", sub: "amount/currency/context", x: 70, y: 104, w: 210, h: 90, tone: "agent" },
      { id: "policy", label: "Policy", sub: "rules + default", x: 70, y: 338, w: 210, h: 90, tone: "human" },
      { id: "validate", label: "Validate", sub: "typed rules", x: 420, y: 220, w: 190, h: 90, tone: "core" },
      { id: "conditions", label: "Conditions", sub: "all/any/not/compare", x: 760, y: 108, w: 218, h: 90, tone: "core" },
      { id: "trace", label: "Rule trace", sub: "matched results", x: 760, y: 296, w: 218, h: 90, tone: "data" },
      { id: "decision", label: "Decision", sub: "allow/approval/deny", x: 760, y: 482, w: 218, h: 90, tone: "wallet" },
    ],
    edges: [
      ["request", "validate", "input"],
      ["policy", "validate", "input"],
      ["validate", "conditions", "evaluate"],
      ["conditions", "trace", "record"],
      ["trace", "decision", "precedence"],
    ],
  },
  budget: {
    title: "Budgets & Spending Targets",
    kind: "Component",
    copy:
      "Agent budgets are hard execution-scoped allocations; user spending targets are separate advisory records. Spend reserves exactly one agent-budget hold for payment or executor completion, and expired executor uncertainty remains frozen until a human reconciles vendor billing.",
    responsibilities: [
      "Creates single or finite recurring budget periods owned by exactly one agent.",
      "Revokes active budgets and replaces them by preserving history and creating a new forward-looking allowance.",
      "Persists user spending targets separately and compares them with the maximum concurrent allocation of overlapping agent budgets.",
      "Returns structured advisory warnings without blocking budget creation or spend.",
      "Keys authorization, claim, and finalization by agent and platform operation key while returning stored state for identical retries.",
      "Reserves one hold per decision, moves executor work from frozen to exclusively claimed, and extends it to the workload claim lease.",
      "Enforces unique agent-scoped operation ownership and finalizes receipt, claim, token, hold, and budget balance in one immediate SQLite transaction while leaving expired claims frozen for reconciliation.",
      "Lists expired claims for the owning user and requires a server-verified human capability before recording the provider receipt, reference, evidence, outcome, actor, and timestamp.",
      "Executor settlement consumes actual vendor cost and returns the unused authorization remainder; release returns the full hold.",
      "A future shared allocation would be an explicit budget pool with agent membership, not a task-scoped branch in the MVP budget model.",
    ],
    links: [sharedLinks.budget, sharedLinks.budgetModel, sharedLinks.spendingTarget, sharedLinks.appSpend, sharedLinks.appClaims, sharedLinks.spendExecutor, sharedLinks.persistence, ["Budget DTOs", "crates/hubu-core/src/budget/dto.rs"]],
    nodes: [
      { id: "create", label: "Create budget/target", sub: "hard + advisory", x: 76, y: 76, w: 206, h: 92, tone: "human" },
      { id: "periods", label: "Periods", sub: "half-open windows", x: 420, y: 76, w: 210, h: 92, tone: "core" },
      { id: "advisory", label: "Target advisory", sub: "max concurrent allocation", x: 780, y: 76, w: 230, h: 92, tone: "human", path: "crates/hubu-core/src/spending_target.rs" },
      { id: "agentSpend", label: "App service", sub: "authorize operation", x: 76, y: 248, w: 206, h: 92, tone: "core", path: "crates/hubu-core/src/app/spend_approval.rs" },
      { id: "reserve", label: "Reserve hold", sub: "frozen → claimed", x: 420, y: 248, w: 210, h: 92, tone: "core" },
      { id: "payment", label: "Hubu payment", sub: "success/failure", x: 76, y: 414, w: 206, h: 92, tone: "wallet" },
      { id: "executor", label: "Claim service", sub: "same operation + lease", x: 76, y: 548, w: 238, h: 92, tone: "executor", path: "crates/hubu-core/src/app/executor_claim.rs" },
      { id: "settle", label: "Settle/release", sub: "actual cost + remainder", x: 420, y: 480, w: 238, h: 92, tone: "core" },
      { id: "store", label: "Governance store", sub: "claims + receipts", x: 780, y: 282, w: 230, h: 96, tone: "data" },
      { id: "reconcile", label: "Human reconciliation", sub: "evidence + receipt", x: 780, y: 500, w: 230, h: 96, tone: "human", path: "crates/hubu-core/src/app/executor_claim.rs" },
    ],
    edges: [
      ["create", "periods", "expand"],
      ["periods", "advisory", "compare"],
      ["advisory", "store", "warn"],
      ["periods", "store", "persist"],
      ["agentSpend", "reserve", "authorize"],
      ["reserve", "store", "freeze one"],
      ["payment", "settle", "payment", { labelDx: 18, labelDy: -18, labelT: 0.56 }],
      ["reserve", "executor", "claim lease", { labelDx: -30, labelDy: 18, labelT: 0.62 }],
      ["executor", "settle", "receipt", { labelDx: 8, labelDy: 24, labelT: 0.56 }],
      ["executor", "reconcile", "lease expires", { labelDx: 10, labelDy: 24, labelT: 0.58 }],
      ["reconcile", "settle", "billed / not billed", { labelDx: 0, labelDy: -20, labelT: 0.48 }],
      ["reconcile", "store", "audit receipt"],
      ["settle", "store", "one transaction"],
      ["periods", "reserve", "active limits"],
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
      "Validates token, owner, amount, agent, account, merchant, task, and currency before rail execution.",
      "Records successful payments in the immutable double-entry ledger, then marks the spend token used.",
      "Returns failed rail responses without ledger writes or token use; the app service persists attempts and decides whether to release holds or keep them frozen for retry.",
    ],
    links: [sharedLinks.payment, sharedLinks.paymentAttempt, sharedLinks.rail, sharedLinks.ledger, ["Payment flow doc", "docs/payment-ledger-flow.md"]],
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
      ["rail", "response", "failed", { labelDx: -70, labelDy: 68, labelT: 0.68 }],
      ["response", "attempts", "persist"],
    ],
  },
  gongbu: {
    title: "Gongbu Execution Plane",
    kind: "Runtime component",
    viewBox: "0 0 1280 760",
    copy:
      "Gongbu is in the Hubu source repository, unified product model, and shared release archive. It remains outside the Hubu control-plane process, database, credential boundary, provider execution boundary, and failure domain.",
    responsibilities: [
      "Accepts authenticated execution requests through its HTTP API or separate Gongbu MCP adapter; callers cannot override operator-owned account, target, price, endpoint, or credentials.",
      "Persists an immutable execution and provider attempt before crossing billable boundaries, then runs recovery through Gongbu-owned Temporal workflow state.",
      "Claims the Hubu authorization before provider work and validates the claim again immediately before the call.",
      "Resolves Gongbu-held credentials and invokes exactly the operator-selected provider adapter without routing or fallback.",
      "Stores normalized artifacts under the Gongbu artifact root and persists metadata in the Gongbu database, never in Hubu storage.",
      "Settles actual cost or safely releases through the v4 HTTP contract; ambiguous outcomes stay in reconciliation instead of causing blind provider retries.",
      "Keeps the Hubu and Gongbu processes, databases, credentials, provider work, artifacts, MCP surfaces, and failure domains separate despite shared source and release identity.",
    ],
    links: [sharedLinks.gongbuOverview, sharedLinks.gongbuServer, sharedLinks.gongbuApplication, sharedLinks.gongbuWorkflow, sharedLinks.gongbuExecution, sharedLinks.gongbuArtifact, sharedLinks.gongbuProvider, sharedLinks.gongbuHubu, sharedLinks.gongbuMcp, sharedLinks.gongbuConfig, sharedLinks.spendExecutor, sharedLinks.api],
    zones: [
      { label: "Gongbu process + owned state", x: 300, y: 44, w: 650, h: 670 },
      { label: "Provider boundary", x: 986, y: 44, w: 246, h: 250 },
      { label: "Hubu control plane", x: 986, y: 474, w: 246, h: 240 },
    ],
    nodes: [
      { id: "agent", label: "Agent client", sub: "Gongbu HTTP/MCP", x: 58, y: 130, w: 196, h: 92, tone: "agent", path: "crates/gongbu-mcp/src/lib.rs" },
      { id: "gongbuApi", label: "Execution API", sub: "auth + admission", x: 340, y: 112, w: 210, h: 92, tone: "executor", path: "crates/gongbu-api/src/http/mod.rs" },
      { id: "workflow", label: "Durable workflow", sub: "claim → execute → settle", x: 674, y: 112, w: 230, h: 92, tone: "executor", path: "crates/gongbu-api/src/workflow.rs" },
      { id: "executionDb", label: "Gongbu SQLite", sub: "executions + attempts", x: 340, y: 352, w: 210, h: 96, tone: "data", path: "crates/gongbu-api/src/execution/mod.rs" },
      { id: "temporal", label: "Temporal state", sub: "timers + recovery", x: 674, y: 276, w: 230, h: 92, tone: "data", path: "crates/gongbu-api/src/temporal.rs" },
      { id: "artifacts", label: "Artifact store", sub: "normalized bytes", x: 340, y: 548, w: 210, h: 96, tone: "data", path: "crates/gongbu-api/src/artifact/mod.rs" },
      { id: "provider", label: "Provider adapter", sub: "selected target only", x: 674, y: 474, w: 230, h: 92, tone: "executor", path: "crates/gongbu-api/src/provider/mod.rs" },
      { id: "credentials", label: "Keychain secrets", sub: "Gongbu-held", x: 674, y: 606, w: 230, h: 80, tone: "data", path: "crates/gongbu-api/src/config/secrets.rs" },
      { id: "vendor", label: "Provider", sub: "external model/API", x: 1012, y: 120, w: 194, h: 124, tone: "vendor" },
      { id: "hubu", label: "Hubu server", sub: "claim + settle/release", x: 1010, y: 556, w: 198, h: 100, tone: "core", path: "crates/hubu-api/src/lib.rs" },
    ],
    edges: [
      ["agent", "gongbuApi", "request + token"],
      ["gongbuApi", "executionDb", "persist first"],
      ["gongbuApi", "workflow", "schedule"],
      ["workflow", "temporal", "durable state"],
      ["workflow", "executionDb", "attempt/receipt", { labelDx: -36, labelDy: 26 }],
      ["workflow", "hubu", "claim/finalize", { labelDy: -26 }],
      ["workflow", "provider", "execute once"],
      ["credentials", "provider", "resolve secret"],
      ["provider", "vendor", "model call", { labelDy: -26 }],
      ["vendor", "provider", "result/usage", { labelDy: 44, labelT: 0.58 }],
      ["provider", "artifacts", "normalized bytes"],
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
    copy:
      "The CLI is the human developer surface and setup helper. It prepares registration envelopes, posts JSON to the local API, configures Codex MCP discovery, and prints compact reviews and results.",
    responsibilities: [
      "Supports init, Codex MCP setup, register, user list and spending-target commands, protocol, policy template/add/list commands, agent list with scoped/all modes, budget, spend, ledger, and health commands.",
      "Writes a managed Codex config block that lets agents in other projects discover Hubu MCP tools without reading the Hubu repo.",
      "Builds canonical registration envelopes with the current owner context and fingerprints from server guidance.",
      "Loads the local Hubu token from env or file and sends it as a bearer header on HTTP JSON requests.",
    ],
    links: [sharedLinks.cli, sharedLinks.api, sharedLinks.registrationProtocol],
    nodes: [
      { id: "commands", label: "Commands", sub: "init/register/spend", x: 90, y: 132, w: 230, h: 92, tone: "human" },
      { id: "guidance", label: "Protocol fetch", sub: "agent-registration JSON", x: 448, y: 132, w: 220, h: 92, tone: "agent" },
      { id: "fingerprint", label: "Envelope builder", sub: "canonical SHA-256", x: 448, y: 356, w: 220, h: 92, tone: "core" },
      { id: "http", label: "HTTP client", sub: "bearer + JSON", x: 804, y: 244, w: 210, h: 92, tone: "core" },
    ],
    edges: [
      ["commands", "guidance", "read"],
      ["guidance", "fingerprint", "fill"],
      ["fingerprint", "http", "POST"],
      ["commands", "http", "GET/POST"],
    ],
  },
  mcp: {
    title: "MCP Adapter",
    kind: "Interface",
    copy:
      "The MCP stdio adapter exposes Hubu as agent tools that Codex and other MCP clients can discover after configuration. Read-only calls are safe to inspect; protected setup tools require trusted client approval.",
    responsibilities: [
      "Implements initialize, tools/list, and tools/call over JSON-RPC stdio.",
      "Can be wired into Codex by `hubu init codex` so agents outside the Hubu repository see Hubu tools at session startup.",
      "Publishes a generic client approval profile so any harness can auto-approve read/spend tools and prompt before setup/admin tools.",
      "Uses Codex per-tool approval overrides as one rendering of that profile while leaving Hubu policy responsible for needs_approval outcomes.",
      "Annotates tools with read-only, destructive, idempotent, open-world, and Hubu approval hints.",
      "Loads the local Hubu token, forwards tool calls to the HTTP API, and marks needs_approval spend responses for the agent client.",
    ],
    links: [sharedLinks.mcp, ["MCP transport doc", "docs/mcp-transport.md"], sharedLinks.api],
    nodes: [
      { id: "agent", label: "Agent client", sub: "JSON-RPC stdio", x: 78, y: 134, w: 220, h: 92, tone: "agent" },
      { id: "tools", label: "Tool catalog", sub: "read/write/approval", x: 430, y: 134, w: 230, h: 92, tone: "core" },
      { id: "approval", label: "Approval gate", sub: "trusted env flag", x: 430, y: 356, w: 230, h: 92, tone: "human" },
      { id: "api", label: "HTTP forwarder", sub: "bearer + Hubu", x: 802, y: 244, w: 220, h: 92, tone: "core" },
    ],
    edges: [
      ["agent", "tools", "list/call"],
      ["tools", "approval", "protected"],
      ["approval", "api", "allowed"],
      ["tools", "api", "read/spend"],
    ],
  },
  agent: {
    title: "Agent Spend Path",
    kind: "Flow",
    copy:
      "Agents never hold private keys. They register as distinct accounts, operate under the current user's policy and their own hard budget, and submit spend intent for Hubu to authorize. User spending targets remain advisory to humans.",
    responsibilities: [
      "Consumes registration guidance instead of guessing protocol fields from prose.",
      "Uses the repository skill to allocate each local-dogfood operation key once and persist its immutable scope in `.hubu/operation-keys.sqlite3` for retry and process recovery.",
      "Submits structured spend requests with amount, reason, merchant, and agent account identity.",
      "Reuses one stable operation key throughout the spend workflow and allocates a different key for intentionally distinct work.",
      "Receives allow, needs_approval, or deny decisions with traceable reasons.",
    ],
    links: [sharedLinks.mcp, sharedLinks.cli, sharedLinks.spend, sharedLinks.registrationProtocol, sharedLinks.operationKeySkill, sharedLinks.operationKeyHelper],
    nodes: [
      { id: "register", label: "Register", sub: "identity/session", x: 90, y: 110, w: 220, h: 92, tone: "agent" },
      { id: "policy", label: "User policy", sub: "human-authored", x: 436, y: 110, w: 220, h: 92, tone: "human" },
      { id: "operation", label: "Operation registry", sub: "scope + stable key", x: 90, y: 336, w: 220, h: 92, tone: "data", path: "skills/generate-hubu-operation-key/scripts/operation_keys.py" },
      { id: "spend", label: "Spend request", sub: "structured intent", x: 436, y: 336, w: 220, h: 92, tone: "agent" },
      { id: "decision", label: "Decision", sub: "trace + token", x: 806, y: 336, w: 220, h: 92, tone: "core" },
    ],
    edges: [
      ["register", "policy", "inherits"],
      ["register", "operation", "agent scope"],
      ["policy", "spend", "governs"],
      ["operation", "spend", "operation key"],
      ["spend", "decision", "evaluate"],
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
    ],
    links: [sharedLinks.cli, sharedLinks.mcp, sharedLinks.registrationProtocol, sharedLinks.budget],
    nodes: [
      { id: "user", label: "User", sub: "username + public id", x: 90, y: 126, w: 210, h: 92, tone: "human" },
      { id: "review", label: "Review", sub: "compact fields", x: 430, y: 126, w: 220, h: 92, tone: "human" },
      { id: "policy", label: "Policy", sub: "rules", x: 800, y: 100, w: 210, h: 92, tone: "core" },
      { id: "budget", label: "Budget + target", sub: "hard + advisory", x: 800, y: 334, w: 210, h: 92, tone: "core" },
      { id: "audit", label: "Audit", sub: "ledger/list views", x: 430, y: 454, w: 220, h: 92, tone: "data" },
    ],
    edges: [
      ["user", "review", "approve"],
      ["review", "policy", "attach"],
      ["review", "budget", "create"],
      ["policy", "audit", "observe"],
      ["budget", "audit", "observe"],
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

let currentView = "top";

const svg = document.getElementById("architecture-canvas");
const title = document.getElementById("diagram-title");
const crumb = document.getElementById("diagram-crumb");
const detailsTitle = document.getElementById("details-title");
const detailsKind = document.getElementById("details-kind");
const detailsCopy = document.getElementById("details-copy");
const responsibilities = document.getElementById("responsibilities");
const codeLinks = document.getElementById("code-links");
const pathTooltip = document.getElementById("path-tooltip");
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
  renderResponsibilities(view.responsibilities);
  renderCodeLinks(view.links || []);
  renderDiagram(view);
}

function renderResponsibilities(items) {
  responsibilities.innerHTML = "";
  items.forEach((item) => {
    const li = document.createElement("li");
    li.textContent = item;
    responsibilities.appendChild(li);
  });
}

function renderCodeLinks(items) {
  codeLinks.innerHTML = "";
  items.forEach(([label, path]) => {
    const li = document.createElement("li");
    const link = document.createElement("a");
    link.href = `https://github.com/hacker-no-ice/hubu/blob/main/${path}`;
    link.target = "_blank";
    link.rel = "noreferrer";
    link.textContent = `${label}: ${path}`;
    li.appendChild(link);
    codeLinks.appendChild(li);
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
  const start = center(from);
  const end = center(to);
  const dx = end.x - start.x;
  const dy = end.y - start.y;
  const horizontal = Math.abs(dx) >= Math.abs(dy);
  const fromPoint = edgePoint(from, horizontal ? Math.sign(dx) : 0, horizontal ? 0 : Math.sign(dy));
  const toPoint = edgePoint(to, horizontal ? -Math.sign(dx) : 0, horizontal ? 0 : -Math.sign(dy));
  const jitter = (index % 2 === 0 ? 1 : -1) * 18;
  const controlA = horizontal
    ? { x: fromPoint.x + dx * 0.32, y: fromPoint.y + jitter }
    : { x: fromPoint.x + jitter, y: fromPoint.y + dy * 0.32 };
  const controlB = horizontal
    ? { x: toPoint.x - dx * 0.32, y: toPoint.y - jitter }
    : { x: toPoint.x - jitter, y: toPoint.y - dy * 0.32 };
  const path = makeSvg("path", {
    class: "arrow-line",
    d: `M ${fromPoint.x} ${fromPoint.y} C ${controlA.x} ${controlA.y}, ${controlB.x} ${controlB.y}, ${toPoint.x} ${toPoint.y}`,
    "marker-end": "url(#arrow-tip)",
  });
  svg.appendChild(path);

  const midpoint = cubicPoint(fromPoint, controlA, controlB, toPoint, options.labelT || 0.5);
  const labelText = label;
  const labelWidth = Math.max(58, labelText.length * 8 + 18);
  const labelX = midpoint.x + (options.labelDx || 0);
  const labelY = midpoint.y - 8 + (options.labelDy || 0);
  const labelBack = makeSvg("rect", {
    class: "arrow-label-back",
    x: labelX - labelWidth / 2,
    y: labelY - 17,
    width: labelWidth,
    height: 23,
    rx: "4",
  });
  svg.appendChild(labelBack);

  const text = makeSvg("text", {
    class: "arrow-label",
    x: labelX,
    y: labelY,
    "text-anchor": "middle",
  });
  text.textContent = labelText;
  svg.appendChild(text);
}

function drawNode(node) {
  const path = pathForNode(node);
  const group = makeSvg("g", {
    class: nodeClass(node),
    tabindex: "0",
    role: "button",
    "aria-label": `${node.label}. Click for details.${path ? ` Path: ${path}.` : ""}`,
  });
  group.dataset.nodeId = node.id;
  if (path) {
    group.dataset.path = path;
    const title = makeSvg("title");
    title.textContent = path;
    group.appendChild(title);
  }

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
  });
  sub.textContent = node.sub;
  group.appendChild(sub);

  group.addEventListener("click", () => drill(node.id));
  group.addEventListener("mouseenter", (event) => showPathTooltip(event, node));
  group.addEventListener("mousemove", (event) => positionPathTooltip(event.clientX, event.clientY));
  group.addEventListener("mouseleave", hidePathTooltip);
  group.addEventListener("focus", () => showFocusedPathTooltip(group, node));
  group.addEventListener("blur", hidePathTooltip);
  group.addEventListener("keydown", (event) => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      drill(node.id);
    }
  });
  svg.appendChild(group);
}

function nodeClass(node) {
  return [
    "node",
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

function pathForNode(node) {
  return node.path || components[node.id]?.links?.[0]?.[1] || null;
}

function showPathTooltip(event, node) {
  const path = pathForNode(node);
  if (!path) return;
  pathTooltip.textContent = path;
  pathTooltip.classList.add("is-visible");
  positionPathTooltip(event.clientX, event.clientY);
}

function showFocusedPathTooltip(group, node) {
  const path = pathForNode(node);
  if (!path) return;
  const box = group.getBoundingClientRect();
  pathTooltip.textContent = path;
  pathTooltip.classList.add("is-visible");
  positionPathTooltip(box.left + box.width / 2, box.top + box.height / 2);
}

function positionPathTooltip(clientX, clientY) {
  const offset = 14;
  const tooltipBox = pathTooltip.getBoundingClientRect();
  const x = Math.min(clientX + offset, window.innerWidth - tooltipBox.width - offset);
  const y = Math.min(clientY + offset, window.innerHeight - tooltipBox.height - offset);
  pathTooltip.style.left = `${Math.max(offset, x)}px`;
  pathTooltip.style.top = `${Math.max(offset, y)}px`;
}

function hidePathTooltip() {
  pathTooltip.classList.remove("is-visible");
}

function drill(nodeId) {
  if (components[nodeId]) {
    hidePathTooltip();
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

function cubicPoint(a, b, c, d, t) {
  const mt = 1 - t;
  return {
    x: mt ** 3 * a.x + 3 * mt ** 2 * t * b.x + 3 * mt * t ** 2 * c.x + t ** 3 * d.x,
    y: mt ** 3 * a.y + 3 * mt ** 2 * t * b.y + 3 * mt * t ** 2 * c.y + t ** 3 * d.y,
  };
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

function makeSvg(name, attrs = {}) {
  const element = document.createElementNS("http://www.w3.org/2000/svg", name);
  Object.entries(attrs).forEach(([key, value]) => element.setAttribute(key, value));
  return element;
}

showView("top");
