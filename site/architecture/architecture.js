const repository = "https://github.com/hacker-no-ice/hubu/blob/main/";

const stages = {
  discover: {
    eyebrow: "01 · DISCOVER",
    title: "One surface, explicit owners",
    summary: "The agent connects to hubu-unified-mcp, which reports backend compatibility and routes each named tool to exactly one owner.",
    outcome: "eligible hubu_* + gongbu_* tools",
    links: ["link-agent-mcp"],
    components: ["agent", "mcp"],
  },
  authorize: {
    eyebrow: "02 · AUTHORIZE",
    title: "Govern before execution",
    summary: "Hubu evaluates trusted identity, policy, targets, and budgets before it issues a scoped authorization.",
    outcome: "allow · deny · needs approval",
    links: ["link-agent-mcp", "link-mcp-hubu"],
    components: ["agent", "mcp", "hubu"],
  },
  execute: {
    eyebrow: "03 · EXECUTE",
    title: "Cross one versioned boundary",
    summary: "Gongbu claims the authorization, runs a durable provider workflow, applies pricing, and keeps credentials and artifacts in its own plane.",
    outcome: "completed · failed · retrying",
    links: ["link-hubu-gongbu", "link-gongbu-provider"],
    components: ["hubu", "gongbu", "provider"],
  },
  settle: {
    eyebrow: "04 · SETTLE",
    title: "Tie receipts back to intent",
    summary: "Gongbu returns compact receipt and artifact references. Hubu finalizes the claim, releases or consumes the hold, and records ledger state.",
    outcome: "settled · released · reconciliation",
    links: ["link-gongbu-provider", "link-hubu-gongbu"],
    components: ["provider", "gongbu", "hubu"],
  },
};

const components = {
  stack: {
    kind: "SYSTEM BOUNDARY",
    title: "Separate by design",
    copy: "The repository and release are unified. The running systems are not. Managed startup selects private credential locations internally, starts the final Hubu once, completes a Gongbu-owned handoff, and preserves separate backend processes, state, credentials, provider execution, artifacts, and recovery paths.",
    diagram: [
      ["Operator profile", "configuration"],
      ["Immutable generation", "validated inputs"],
      ["Lifecycle launcher", "owned processes"],
      ["Final Hubu", "creates capabilities"],
      ["Gongbu handoff", "verify + persist"],
      ["Gongbu", "starts after handoff"],
      ["Unified MCP", "client-owned"],
    ],
    responsibilities: [
      "Ship compatible binaries from one locked source revision.",
      "Communicate only through the versioned executor contract.",
      "Keep backend credentials, databases, and failures isolated.",
      "Hide managed credential locations from operator source while keeping Hubu and Gongbu credential state separately owned.",
      "Start one final Hubu process, verify its protected capability through Gongbu's narrow handoff, then start Gongbu.",
      "Expose one agent-facing MCP surface without collapsing ownership.",
      "Verify the real local process lifecycle and deterministic execution with a non-billable acceptance canary.",
    ],
    links: [
      ["Repository overview", "README.md"],
      ["Local stack quick start", "docs/local-stack.md"],
      ["Local stack acceptance canary", "scripts/integration-local-stack-acceptance.sh"],
      ["Managed credential handoff", "crates/gongbu-api/src/config/setup.rs"],
      ["Spend lifecycle", "docs/spend-lifecycle.md"],
      ["Release operations", "docs/operations/releases.md"],
    ],
  },
  agent: {
    kind: "CALLER",
    title: "Structured intent, not authority",
    copy: "The agent supplies workload intent through the unified MCP surface. Trusted session metadata and server-issued capabilities establish authority outside model-authored arguments.",
    diagram: [
      ["Human authority", "policy + approval"],
      ["Agent harness", "trusted metadata"],
      ["Unified MCP", "canonical input"],
      ["Owned backend", "one route"],
    ],
    responsibilities: [
      "Discover the currently eligible tool catalog.",
      "Reuse stable operation identity for exact recovery; after a definitive denial, submit corrected work as a new logical operation.",
      "Submit structured, canonical spend and execution inputs.",
    ],
    links: [["Registration protocol", "docs/agent-registration.md"], ["Unified MCP setup", "docs/unified-mcp.md"]],
  },
  mcp: {
    kind: "AGENT SURFACE",
    title: "Route without merging",
    copy: "hubu-unified-mcp routing revision 4 is a thin stdio adapter over separate Hubu and Gongbu HTTP clients. Its static ownership table routes each tool to one backend and fails closed on unknown names or incompatible versions.",
    diagram: [
      ["Agent harness", "stdio client"],
      ["Capability probe", "compatibility"],
      ["Static router", "39 named tools"],
      ["Hubu client", "Hubu credential"],
      ["Gongbu client", "Gongbu credential"],
    ],
    responsibilities: [
      "Probe Hubu and Gongbu independently for compatibility.",
      "Publish one sanitized, availability-aware tool catalog.",
      "Route the 31 Hubu tools, including safe dynamic budget update and history paths, without merging backend ownership.",
      "Return recursively redacted typed budget rejections as MCP tool errors while preserving existing behavior for other Hubu tools.",
      "Never forward one backend credential to the other.",
      "Preserve backend result and error semantics.",
    ],
    links: [
      ["Unified MCP guide", "docs/unified-mcp.md"],
      ["Router implementation", "crates/hubu-unified-mcp/src/lib.rs"],
      ["Hubu routing", "crates/hubu-unified-mcp/src/hubu/routing.rs"],
      ["Gongbu catalog", "crates/hubu-unified-mcp/src/gongbu/catalog.rs"],
    ],
  },
  hubu: {
    kind: "CONTROL PLANE",
    title: "Govern and account",
    copy: "Hubu owns the durable decision boundary: humans, agents, policy, budgets, spending targets, authorizations, claims, reconciliation, payments, and ledger state.",
    diagram: [
      ["Protected API", "caller authority"],
      ["Identity + registration", "canonical owner"],
      ["Policy engine", "allow · deny · review"],
      ["Budgets + claims", "reserve · settle"],
      ["Payment + ledger", "balanced record"],
      ["Hubu SQLite", "governance state"],
    ],
    responsibilities: [
      "Canonicalize identity and spend scope before evaluation.",
      "Apply deterministic policy and reserve budget capacity.",
      "Issue scoped authorizations and exclusive executor claims.",
      "Finalize receipts, reconciliation evidence, and ledger entries atomically.",
    ],
    links: [
      ["Policy engine", "docs/policy-engine.md"],
      ["Spend lifecycle", "docs/spend-lifecycle.md"],
      ["Approval service", "crates/hubu-core/src/app/spend_approval.rs"],
      ["Executor claims", "crates/hubu-core/src/app/executor_claim.rs"],
      ["Ledger", "crates/hubu-wallet/src/ledger.rs"],
    ],
  },
  api: {
    kind: "HUBU INTERFACE",
    title: "Authenticate, then delegate",
    copy: "The Hubu HTTP process owns bounded local transport, caller capabilities, route parsing, and response shaping. Domain transitions remain in independently testable application services.",
    diagram: [
      ["Local caller", "bearer capability"],
      ["HTTP framing", "bounded request"],
      ["Route authority", "owner capabilities"],
      ["App services", "domain transition"],
      ["Safe response", "stable resource IDs"],
    ],
    responsibilities: [
      "Keep public health and guidance separate from protected routes.",
      "Require distinct human capabilities for approval and reconciliation mutations.",
      "Delegate spend approval and executor-claim transitions to core application services.",
      "Parse strict public budget-version paths, expose public current-version identity, and keep update errors typed without leaking storage detail.",
      "Return structured, redacted diagnostics without leaking credentials.",
    ],
    links: [
      ["HTTP API", "crates/hubu-api/src/lib.rs"],
      ["Spend approval service", "crates/hubu-core/src/app/spend_approval.rs"],
      ["Executor claim service", "crates/hubu-core/src/app/executor_claim.rs"],
    ],
  },
  registration: {
    kind: "IDENTITY FLOW",
    title: "Register canonical identity",
    copy: "Registration turns a small human-reviewed input into stable agent identity, version, account, and session records whose fingerprints the server can independently verify.",
    diagram: [
      ["Human inputs", "name + version"],
      ["Server guidance", "required fields"],
      ["Canonical payload", "client prepared"],
      ["Fingerprint check", "server recomputed"],
      ["Identity records", "agent + account"],
    ],
    responsibilities: [
      "Keep the human review compact while giving clients machine-readable guidance.",
      "Canonicalize identity and version payloads before hashing.",
      "Reject mismatched fingerprints before creating or reusing records.",
      "Bind the resulting account and session to the active human owner.",
    ],
    links: [
      ["Registration protocol", "docs/agent-registration.md"],
      ["Registration manager", "crates/hubu-core/src/registration/manager.rs"],
      ["Registration model", "crates/hubu-core/src/registration/model.rs"],
    ],
  },
  policy: {
    kind: "DECISION FLOW",
    title: "Decide before reserving",
    copy: "The policy engine evaluates canonical, trusted request scope deterministically. Its outcome either stops the request, pauses for a human, or permits budget reservation.",
    diagram: [
      ["Canonical scope", "trusted catalog"],
      ["Policy conditions", "ordered rules"],
      ["Decision", "allow · deny · review"],
      ["Human resolution", "immutable snapshot"],
      ["Budget admission", "allowed only"],
    ],
    responsibilities: [
      "Evaluate stable provider, executor, capability, merchant, amount, and owner identity.",
      "Persist needs-approval decisions without starting provider or payment work.",
      "Resume the same operation after an idempotent human resolution.",
      "Keep policy content and assignment history inspectable.",
    ],
    links: [
      ["Policy guide", "docs/policy-engine.md"],
      ["Policy engine", "crates/hubu-core/src/policy/engine.rs"],
      ["Policy conditions", "crates/hubu-core/src/policy/condition.rs"],
    ],
  },
  budgets: {
    kind: "MONEY LIFECYCLE",
    title: "Version, reserve, then finalize",
    copy: "Budgets are stable logical allocations with immutable total-limit versions. Hubu derives availability from administrative state, time, and one cumulative balance, freezes the maximum before execution, then atomically settles actual cost, releases unused capacity, or preserves uncertainty for reconciliation.",
    diagram: [
      ["Logical budget", "stable bgt_ identity"],
      ["Version head", "immutable bgv_ history"],
      ["Frozen hold", "authorized maximum"],
      ["Executor claim", "exclusive work"],
      ["Provider outcome", "billing evidence"],
      ["Settle · release", "or reconcile"],
      ["Double-entry ledger", "balanced finality"],
    ],
    responsibilities: [
      "Prevent overlapping non-revoked budget windows for one agent and currency.",
      "Append total-cap changes with compare-and-set revision checks while preserving consumed and frozen usage.",
      "Return the current snapshot once and immutable provenance in ascending history order.",
      "Reserve capacity atomically only when the evaluated budget is active.",
      "Never release a hold merely because provider billing is ambiguous.",
      "Record successful money movement as immutable balanced entries.",
    ],
    links: [
      ["Spend lifecycle", "docs/spend-lifecycle.md"],
      ["Budget manager", "crates/hubu-core/src/budget/manager.rs"],
      ["Payment orchestration", "crates/hubu-wallet/src/payment.rs"],
      ["Ledger", "crates/hubu-wallet/src/ledger.rs"],
    ],
  },
  persistence: {
    kind: "STORAGE BOUNDARY",
    title: "Commit one durable truth",
    copy: "Hubu persistence stores governance and accounting state without opening Gongbu's execution database or artifact root. Finalization commits related claim, hold, receipt, and ledger changes together.",
    diagram: [
      ["App service", "validated transition"],
      ["SQLite transaction", "atomic boundary"],
      ["Core records", "identity + policy"],
      ["Spend records", "holds + claims"],
      ["Ledger entries", "immutable balance"],
      ["Recovery", "same operation"],
    ],
    responsibilities: [
      "Persist users, agents, policies, budgets, authorizations, claims, and receipts.",
      "Commit finalization state atomically so balances cannot diverge from claims.",
      "Reject ledger updates and deletes after successful money movement.",
      "Remain physically and operationally separate from Gongbu state.",
    ],
    links: [
      ["Core storage", "crates/hubu-core/src/storage.rs"],
      ["Governance persistence", "crates/hubu-core/src/persistence.rs"],
      ["Wallet persistence", "crates/hubu-wallet/src/persistence.rs"],
    ],
  },
  gongbu: {
    kind: "EXECUTION PLANE",
    title: "Execute and recover",
    copy: "Gongbu owns provider work end to end: credentials, pricing, Temporal workflows, retries, execution records, receipts, and artifacts. Hubu never opens Gongbu state.",
    diagram: [
      ["Execution API", "authorized scope"],
      ["Execution record", "persist first"],
      ["Temporal workflow", "durable retry"],
      ["Provider adapter", "owned credential"],
      ["Receipt + artifacts", "execution evidence"],
      ["Gongbu state", "isolated recovery"],
    ],
    responsibilities: [
      "Validate and exclusively claim Hubu authorization.",
      "Run provider calls through durable Temporal activities.",
      "Price actual work and persist safe receipt metadata.",
      "Own artifact storage, retry policy, and recovery as one failure domain.",
    ],
    links: [
      ["Execution plane guide", "docs/gongbu-execution.md"],
      ["Server runbook", "docs/operations/gongbu-server.md"],
      ["Workflow implementation", "crates/gongbu-api/src/workflow.rs"],
      ["Provider boundary", "crates/gongbu-api/src/provider/mod.rs"],
      ["Artifact service", "crates/gongbu-api/src/artifact/mod.rs"],
    ],
  },
  provider: {
    kind: "EXTERNAL EDGE",
    title: "Treat provider work as evidence",
    copy: "External providers receive only the credentials and requests owned by Gongbu. Their responses become priced execution evidence and artifact references—not a second source of governance truth.",
    diagram: [
      ["Target + price gate", "operator config"],
      ["Gongbu adapter", "credential owner"],
      ["Provider API", "external work"],
      ["Receipt + artifact", "sanitized evidence"],
      ["Hubu settlement", "financial finality"],
    ],
    responsibilities: [
      "Remain outside both Hubu and Gongbu trust boundaries.",
      "Use provider-specific idempotency and bounded retries.",
      "Return sanitized receipt metadata and artifacts to Gongbu.",
    ],
    links: [["Live provider testing", "docs/operations/live-provider-testing.md"], ["Provider adapters", "crates/gongbu-api/src/provider/mod.rs"]],
  },
};

const stageOrder = ["discover", "authorize", "execute", "settle"];
let currentStage = "authorize";
let playTimer = null;

function selectStage(stageName) {
  const stage = stages[stageName];
  if (!stage) return;
  currentStage = stageName;
  document.body.dataset.stage = stageName;
  document.querySelectorAll("[data-stage-button]").forEach((button) => {
    button.setAttribute("aria-pressed", button.dataset.stageButton === stageName ? "true" : "false");
  });
  document.querySelectorAll(".flow-link").forEach((link) => link.classList.remove("is-active"));
  stage.links.forEach((className) => document.querySelector(`.${className}`)?.classList.add("is-active"));
  document.querySelectorAll(".flow-node").forEach((node) => node.classList.toggle("is-active", stage.components.includes(node.dataset.component)));
  document.querySelector("#stage-eyebrow").textContent = stage.eyebrow;
  document.querySelector("#stage-title").textContent = stage.title;
  document.querySelector("#stage-summary").textContent = stage.summary;
  document.querySelector("#stage-outcome").textContent = stage.outcome;
}

function selectComponent(componentName) {
  const component = components[componentName];
  if (!component) return;
  document.querySelectorAll("[role='tab'][data-component]").forEach((tab) => {
    tab.setAttribute("aria-selected", tab.dataset.component === componentName ? "true" : "false");
  });
  document.querySelector("#detail-kind").textContent = component.kind;
  document.querySelector("#detail-title").textContent = component.title;
  document.querySelector("#detail-copy").textContent = component.copy;
  document.querySelector("#detail-diagram").replaceChildren(
    ...component.diagram.map(([label, detail]) => {
      const item = document.createElement("li");
      const title = document.createElement("strong");
      const description = document.createElement("span");
      title.textContent = label;
      description.textContent = detail;
      item.append(title, description);
      return item;
    }),
  );
  document.querySelector("#detail-responsibilities").replaceChildren(
    ...component.responsibilities.map((text) => Object.assign(document.createElement("li"), { textContent: text })),
  );
  document.querySelector("#detail-links").replaceChildren(
    ...component.links.map(([label, path]) => {
      const item = document.createElement("li");
      const link = document.createElement("a");
      link.href = repository + path;
      link.textContent = `${label} ↗`;
      item.append(link);
      return item;
    }),
  );
}

document.querySelectorAll("[data-stage-button]").forEach((button) => {
  button.addEventListener("click", () => {
    stopPlayback();
    selectStage(button.dataset.stageButton);
  });
});

document.querySelectorAll("[data-component]").forEach((button) => {
  button.addEventListener("click", () => selectComponent(button.dataset.component));
});

document.querySelector("#play-flow").addEventListener("click", (event) => {
  if (playTimer) {
    stopPlayback();
    return;
  }
  let index = stageOrder.indexOf(currentStage);
  selectStage(stageOrder[index]);
  event.currentTarget.innerHTML = '<span aria-hidden="true">Ⅱ</span> Pause the flow';
  event.currentTarget.setAttribute("aria-pressed", "true");
  playTimer = window.setInterval(() => {
    index = (index + 1) % stageOrder.length;
    selectStage(stageOrder[index]);
  }, 2100);
});

function stopPlayback() {
  window.clearInterval(playTimer);
  playTimer = null;
  const control = document.querySelector("#play-flow");
  control.innerHTML = '<span aria-hidden="true">▶</span> Play the flow';
  control.setAttribute("aria-pressed", "false");
}

selectStage("authorize");
selectComponent("stack");
