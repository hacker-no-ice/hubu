const sharedLinks = {
  readme: ["README", "README.md"],
  api: ["Local HTTP API", "crates/hubu-api/src/lib.rs"],
  cli: ["CLI", "crates/hubu-cli/src/main.rs"],
  mcp: ["MCP adapter", "crates/hubu-mcp/src/lib.rs"],
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
  payment: ["Payment manager", "crates/hubu-wallet/src/payment.rs"],
  paymentAttempt: ["Payment attempt store", "crates/hubu-wallet/src/persistence.rs"],
  rail: ["Payment rail", "crates/hubu-wallet/src/rail.rs"],
  ledger: ["Ledger", "crates/hubu-wallet/src/ledger.rs"],
  storage: ["Core SQLite storage", "crates/hubu-core/src/storage.rs"],
  persistence: ["Governance persistence", "crates/hubu-core/src/persistence.rs"],
  telemetry: ["Telemetry", "crates/hubu-core/src/telemetry.rs"],
};

const components = {
  top: {
    title: "System Map",
    kind: "Top level",
    viewBox: "0 0 1200 760",
    copy:
      "Hubu includes the CLI, MCP adapter, local server, governance core, wallet, and ledger. Gongbu is shown outside Hubu because it performs external model calls and other work through the executor contract.",
    responsibilities: [
      "Humans register, attach user-level policies, set user caps, create agent budgets, and review protected actions.",
      "Agents discover Hubu through configured MCP tools, while humans use the CLI for setup and administration.",
      "The CLI and MCP adapter are part of broader Hubu, but they are not the Hubu server.",
      "Local HTTP callers reach the API with the Hubu bearer token before protected routes resolve user authority.",
      "Gongbu and other external executors validate, settle, or release authorized spend without Hubu performing the work.",
      "The API coordinates core governance managers, wallet execution, and executor-facing spend state.",
      "Gongbu owns vendor credentials, provider adapters, model calls, artifacts, and execution retries outside Hubu.",
      "SQLite-backed records preserve users, agents, caps, budgets, policies, payments, and ledger entries.",
    ],
    links: [sharedLinks.readme, sharedLinks.api, sharedLinks.cli, sharedLinks.mcp, sharedLinks.spendExecutor, sharedLinks.futureWallet],
    zones: [
      { label: "Broader Hubu", x: 292, y: 24, w: 820, h: 704 },
      { label: "Hubu server", x: 570, y: 36, w: 542, h: 692, labelX: 636, labelY: 58 },
      { label: "Outside Hubu", x: 36, y: 578, w: 244, h: 136, labelY: 604 },
    ],
    nodes: [
      { id: "human", label: "Human owner", sub: "funds + policy", x: 48, y: 82, w: 190, h: 94, tone: "human" },
      { id: "agent", label: "AI agent", sub: "spend requests", x: 48, y: 330, w: 190, h: 94, tone: "agent" },
      { id: "cli", label: "Hubu CLI", sub: "developer commands", x: 334, y: 82, w: 184, h: 88, tone: "surface" },
      { id: "mcp", label: "MCP adapter", sub: "agent tools", x: 334, y: 330, w: 184, h: 88, tone: "surface" },
      { id: "api", label: "Local HTTP API", sub: "orchestrator + auth", x: 620, y: 220, w: 190, h: 112, tone: "core" },
      { id: "gongbu", label: "Gongbu", sub: "outside Hubu executor", x: 58, y: 632, w: 204, h: 74, tone: "executor" },
      { id: "registration", label: "Registration", sub: "identity + sessions", x: 900, y: 46, w: 202, h: 86, tone: "core" },
      { id: "policy", label: "Policy engine", sub: "deterministic rules", x: 904, y: 166, w: 198, h: 86, tone: "core" },
      { id: "budget", label: "Budget manager", sub: "reserve + settle", x: 904, y: 304, w: 198, h: 86, tone: "core" },
      { id: "payment", label: "Payment manager", sub: "rail boundary", x: 904, y: 448, w: 198, h: 86, tone: "wallet" },
      { id: "ledger", label: "SQLite ledger", sub: "double-entry audit", x: 900, y: 626, w: 202, h: 88, tone: "data" },
    ],
    edges: [
      ["human", "cli", "commands"],
      ["agent", "mcp", "tools/call"],
      ["cli", "api", "HTTP JSON + token", { labelDx: -22, labelDy: -18 }],
      ["mcp", "api", "HTTP JSON + token", { labelDx: -6, labelDy: 28 }],
      ["agent", "gongbu", "work + token"],
      ["gongbu", "api", "validate/settle"],
      ["api", "registration", "register"],
      ["api", "policy", "evaluate"],
      ["api", "budget", "hold funds"],
      ["api", "payment", "submit payment", { labelDx: -20, labelDy: 56, labelT: 0.45 }],
      ["payment", "ledger", "record success"],
      ["budget", "ledger", "audit state", { labelDx: -70, labelDy: 38, labelT: 0.62 }],
    ],
  },
  api: {
    title: "Local HTTP API",
    kind: "Component",
    copy:
      "The local server is a small TCP HTTP API. It authenticates protected local requests with a bearer token, owns the shared process state, exposes JSON routes, and stitches together registration, policy, budget, spend, payment, executor, and ledger managers.",
    responsibilities: [
      "Keeps health and guidance public while requiring a local bearer token for user setup, agent registration, policies, caps, budgets, spend, and ledger listing.",
      "Uses the local token and current user context for protected workflow authority and agent registration ownership.",
      "Hydrates state from the configured SQLite path and reconciles expired budget holds at startup.",
      "Bridges wallet payment authorization and external executor validation through shared spend state.",
    ],
    links: [sharedLinks.api, sharedLinks.spendExecutor, sharedLinks.persistence, sharedLinks.telemetry],
    nodes: [
      { id: "routes", label: "Routes", sub: "GET/POST JSON", x: 72, y: 92, w: 190, h: 90, tone: "agent" },
      { id: "auth", label: "Local auth", sub: "token + owner", x: 410, y: 76, w: 220, h: 92, tone: "core" },
      { id: "state", label: "ServerState", sub: "shared managers", x: 410, y: 250, w: 220, h: 96, tone: "core" },
      { id: "registration", label: "Registration", sub: "agent records", x: 805, y: 48, w: 190, h: 84, tone: "core" },
      { id: "governance", label: "Governance DB", sub: "policy/budget/spend", x: 804, y: 180, w: 196, h: 84, tone: "data" },
      { id: "wallet", label: "Wallet", sub: "payment + ledger", x: 808, y: 310, w: 188, h: 84, tone: "wallet" },
      { id: "telemetry", label: "Telemetry", sub: "JSON events", x: 418, y: 432, w: 210, h: 86, tone: "data" },
    ],
    edges: [
      ["routes", "auth", "protect"],
      ["auth", "state", "dispatch"],
      ["state", "registration", "mutate"],
      ["state", "governance", "persist"],
      ["state", "wallet", "execute"],
      ["state", "telemetry", "log"],
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
    title: "Budget Manager",
    kind: "Component",
    copy:
      "Budgets are execution-scoped allocations for agents today and tasks later. Humans create the agent budget and user cap, then agent spend authorization reserves one budget hold and one cap hold before either Hubu payment completion or executor completion settles or releases both.",
    responsibilities: [
      "Creates single or finite recurring budget periods for agent scopes, with room for task/project scopes later.",
      "Revokes active budgets and replaces them by preserving history and creating a new forward-looking allowance.",
      "Tracks user caps separately in the API/CLI while reusing the underlying balance and hold machinery.",
      "Reserves holds from the agent spend path only after spend fits both an active agent budget and the active user cap.",
      "Reserves, settles, releases, and expires budget/cap holds for wallet payments and external executors.",
      "Current settlement consumes or returns the authorized hold amount; future executor settlement may reconcile actual usage when it differs.",
    ],
    links: [sharedLinks.budget, sharedLinks.budgetModel, sharedLinks.spendExecutor, sharedLinks.persistence, ["Budget DTOs", "crates/hubu-core/src/budget/dto.rs"]],
    nodes: [
      { id: "create", label: "Create budget/cap", sub: "agent allocation + user max", x: 76, y: 76, w: 206, h: 92, tone: "human" },
      { id: "periods", label: "Periods", sub: "half-open windows", x: 420, y: 76, w: 210, h: 92, tone: "core" },
      { id: "agentSpend", label: "Agent spend", sub: "authorize request", x: 76, y: 248, w: 206, h: 92, tone: "agent", path: "crates/hubu-api/src/lib.rs" },
      { id: "reserve", label: "Reserve holds", sub: "budget + cap", x: 420, y: 248, w: 210, h: 92, tone: "core" },
      { id: "payment", label: "Hubu payment", sub: "success/failure", x: 76, y: 414, w: 206, h: 92, tone: "wallet" },
      { id: "executor", label: "Executor completion", sub: "Gongbu settle/release", x: 76, y: 548, w: 238, h: 92, tone: "executor", path: "docs/spend-executor-contract.md" },
      { id: "settle", label: "Settle/release", sub: "authorized hold amount", x: 420, y: 480, w: 238, h: 92, tone: "core" },
      { id: "store", label: "Governance store", sub: "balances + holds", x: 780, y: 282, w: 230, h: 96, tone: "data" },
    ],
    edges: [
      ["create", "periods", "expand"],
      ["periods", "store", "persist"],
      ["agentSpend", "reserve", "authorize"],
      ["reserve", "store", "freeze both"],
      ["payment", "settle", "payment", { labelDx: 18, labelDy: -18, labelT: 0.56 }],
      ["executor", "settle", "executor", { labelDx: 8, labelDy: 24, labelT: 0.56 }],
      ["settle", "store", "update"],
      ["periods", "reserve", "active limits"],
    ],
  },
  payment: {
    title: "Payment Manager",
    kind: "Component",
    copy:
      "The wallet boundary receives an API-built payment request after allowed spend. It checks request shape and idempotency, validates the spend token through a trait boundary, executes the selected rail, records only successful money movement, and marks tokens used only after ledger success.",
    responsibilities: [
      "Rejects malformed amounts, empty idempotency keys, and conflicting idempotency-key replays.",
      "Returns the original response for an identical idempotency replay without revalidating, rerunning the rail, or writing another ledger transaction.",
      "Validates token, owner, amount, agent, account, merchant, task, and currency before rail execution.",
      "Records successful payments in the immutable double-entry ledger, then marks the spend token used.",
      "Returns failed rail responses without ledger writes or token use; the API persists attempts and settles or releases budget holds around this boundary.",
    ],
    links: [sharedLinks.payment, sharedLinks.paymentAttempt, sharedLinks.rail, sharedLinks.ledger, ["Payment flow doc", "docs/payment-ledger-flow.md"]],
    nodes: [
      { id: "request", label: "API payment request", sub: "idempotency + token", x: 70, y: 96, w: 238, h: 92, tone: "core", path: "crates/hubu-api/src/lib.rs" },
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
    title: "Gongbu Executor",
    kind: "External",
    viewBox: "0 0 1280 700",
    copy:
      "Gongbu is outside Hubu. It is primarily the model-calling proxy: agents ask Gongbu to perform work, Gongbu calls external vendors, and Gongbu uses Hubu on the side for spend token validation and budget settle/release.",
    responsibilities: [
      "Accepts model/work requests from agents with a Hubu spend authorization token.",
      "Validates the token with Hubu before irreversible billable vendor work.",
      "Calls external model/API vendors such as Google using Gongbu-held credentials.",
      "Returns outputs to the agent, then settles successful billable work or releases unused budget through Hubu.",
      "Keeps vendor credentials, prompts, provider payloads, and generated artifacts outside Hubu.",
    ],
    links: [sharedLinks.spendExecutor, sharedLinks.futureWallet, sharedLinks.api],
    zones: [
      { label: "Main model call path", x: 46, y: 112, w: 1130, h: 226 },
      { label: "Hubu side control plane", x: 366, y: 396, w: 390, h: 190, labelY: 428 },
      { label: "External vendors", x: 940, y: 136, w: 236, h: 198, labelX: 968 },
    ],
    nodes: [
      { id: "agent", label: "Agent", sub: "work request", x: 82, y: 190, w: 190, h: 92, tone: "agent" },
      { id: "gongbu", label: "Gongbu", sub: "model proxy", x: 520, y: 182, w: 230, h: 108, tone: "executor" },
      { id: "vendor", label: "Google", sub: "model/API vendor", x: 966, y: 176, w: 186, h: 124, tone: "vendor" },
      { id: "hubu", label: "Hubu", sub: "validate + settle", x: 446, y: 462, w: 230, h: 92, tone: "core" },
    ],
    edges: [
      ["agent", "gongbu", "request + token", { labelDy: -34 }],
      ["gongbu", "vendor", "model call", { labelDy: -34 }],
      ["vendor", "gongbu", "vendor result", { labelDy: 54, labelT: 0.55 }],
      ["gongbu", "agent", "return output", { labelDy: 54, labelT: 0.48 }],
      ["gongbu", "hubu", "validate token", { labelDx: -74, labelDy: 10, labelT: 0.58 }],
      ["gongbu", "hubu", "settle/release", { labelDx: 150, labelDy: 38, labelT: 0.7 }],
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
      "Supports init, Codex MCP setup, register, user list and user cap commands, protocol, policy template/add/list commands, agent list with scoped/all modes, budget, spend, ledger, and health commands.",
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
      "Agents never hold private keys. They register as distinct accounts, operate under the current user's policy, agent budget, and user cap, and submit spend intent for Hubu to authorize.",
    responsibilities: [
      "Consumes registration guidance instead of guessing protocol fields from prose.",
      "Submits structured spend requests with amount, reason, merchant, and agent/account identity.",
      "Receives allow, needs_approval, or deny decisions with traceable reasons.",
    ],
    links: [sharedLinks.mcp, sharedLinks.cli, sharedLinks.spend, sharedLinks.registrationProtocol],
    nodes: [
      { id: "register", label: "Register", sub: "identity/session", x: 90, y: 110, w: 220, h: 92, tone: "agent" },
      { id: "policy", label: "User policy", sub: "human-authored", x: 436, y: 110, w: 220, h: 92, tone: "human" },
      { id: "spend", label: "Spend request", sub: "structured intent", x: 436, y: 336, w: 220, h: 92, tone: "agent" },
      { id: "decision", label: "Decision", sub: "trace + token", x: 806, y: 336, w: 220, h: 92, tone: "core" },
    ],
    edges: [
      ["register", "policy", "inherits"],
      ["policy", "spend", "governs"],
      ["spend", "decision", "evaluate"],
    ],
  },
  human: {
    title: "Human Owner Flow",
    kind: "Flow",
    copy:
      "Humans set the financial boundaries. The CLI and MCP adapter aim to keep review small while making identity, policy, cap, and budget state explicit.",
    responsibilities: [
      "Registers humans with separate username and display name fields.",
      "Reviews current owner context, agent name/version, and protected setup actions.",
      "Funds governance by creating a user-level policy, user cap, and agent budget before agent spending.",
    ],
    links: [sharedLinks.cli, sharedLinks.mcp, sharedLinks.registrationProtocol, sharedLinks.budget],
    nodes: [
      { id: "user", label: "User", sub: "username + public id", x: 90, y: 126, w: 210, h: 92, tone: "human" },
      { id: "review", label: "Review", sub: "compact fields", x: 430, y: 126, w: 220, h: 92, tone: "human" },
      { id: "policy", label: "Policy", sub: "rules", x: 800, y: 100, w: 210, h: 92, tone: "core" },
      { id: "budget", label: "Budget", sub: "limits", x: 800, y: 334, w: 210, h: 92, tone: "core" },
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
  return node.path || components[node.id]?.links?.[0]?.[1] || components[currentView]?.links?.[0]?.[1] || null;
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
