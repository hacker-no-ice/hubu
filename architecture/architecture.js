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
    copy: "The repository and release are unified. The running systems are not. Hubu and Gongbu retain separate processes, state, credentials, provider execution, artifacts, and recovery paths.",
    responsibilities: [
      "Ship compatible binaries from one locked source revision.",
      "Communicate only through the versioned executor contract.",
      "Keep backend credentials, databases, and failures isolated.",
      "Expose one agent-facing MCP surface without collapsing ownership.",
      "Verify the real local process lifecycle and deterministic execution with a non-billable acceptance canary.",
    ],
    links: [
      ["Repository overview", "README.md"],
      ["Local stack contract", "docs/local-stack.md"],
      ["Local stack acceptance canary", "scripts/integration-local-stack-acceptance.sh"],
      ["Spend lifecycle", "docs/spend-lifecycle.md"],
      ["Release operations", "docs/operations/releases.md"],
    ],
  },
  agent: {
    kind: "CALLER",
    title: "Structured intent, not authority",
    copy: "The agent supplies workload intent through the unified MCP surface. Trusted session metadata and server-issued capabilities establish authority outside model-authored arguments.",
    responsibilities: [
      "Discover the currently eligible tool catalog.",
      "Reuse stable operation identity across retries.",
      "Submit structured, canonical spend and execution inputs.",
    ],
    links: [["Registration protocol", "docs/agent-registration.md"], ["Unified MCP setup", "docs/unified-mcp.md"]],
  },
  mcp: {
    kind: "AGENT SURFACE",
    title: "Route without merging",
    copy: "hubu-unified-mcp is a thin stdio adapter over separate Hubu and Gongbu HTTP clients. Its static ownership table routes each tool to one backend and fails closed on unknown names or incompatible versions.",
    responsibilities: [
      "Probe Hubu and Gongbu independently for compatibility.",
      "Publish one sanitized, availability-aware tool catalog.",
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
  gongbu: {
    kind: "EXECUTION PLANE",
    title: "Execute and recover",
    copy: "Gongbu owns provider work end to end: credentials, pricing, Temporal workflows, retries, execution records, receipts, and artifacts. Hubu never opens Gongbu state.",
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
    window.clearInterval(playTimer);
    playTimer = null;
    selectStage(button.dataset.stageButton);
  });
});

document.querySelectorAll("[data-component]").forEach((button) => {
  button.addEventListener("click", () => selectComponent(button.dataset.component));
});

document.querySelector("#play-flow").addEventListener("click", (event) => {
  if (playTimer) {
    window.clearInterval(playTimer);
    playTimer = null;
    event.currentTarget.innerHTML = '<span aria-hidden="true">▶</span> Play the flow';
    return;
  }
  let index = stageOrder.indexOf(currentStage);
  selectStage(stageOrder[index]);
  event.currentTarget.innerHTML = '<span aria-hidden="true">Ⅱ</span> Pause the flow';
  playTimer = window.setInterval(() => {
    index = (index + 1) % stageOrder.length;
    selectStage(stageOrder[index]);
  }, 2100);
});

selectStage("authorize");
selectComponent("stack");
