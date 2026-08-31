const repository = "https://github.com/hacker-no-ice/hubu/blob/main/";

const stages = {
  discover: {
    eyebrow: "01 · DISCOVER",
    title: "One surface, explicit owners",
    summary: "The agent connects to hubu-unified-mcp, which reports backend compatibility, exposes the sanitized managed-provider catalog, and routes each named tool to exactly one owner.",
    outcome: "eligible tools + exact sanitized provider catalog",
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
    summary: "Gongbu claims the authorization, submits asynchronous generation once, checkpoints the provider operation, and recovers by polling that same operation in its own plane.",
    outcome: "submitted once · polling · completed",
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
    copy: "The repository and release are unified. The running systems are not. A versioned FLUX profile flows through source doctor and render, Gongbu production validation, and sanitized CLI/MCP catalog exposure while preserving separate backend processes, state, credentials, provider execution, artifacts, and recovery paths.",
    diagram: [
      ["Supported profile source", "versioned FLUX contract"],
      ["Source doctor", "four readiness facts; no BFL"],
      ["Immutable render", "target + price + policies"],
      ["Gongbu validator", "exact production contract"],
      ["Sanitized catalog", "CLI + MCP"],
    ],
    responsibilities: [
      "Freeze provider flux, adapter flux2_api, model flux-2-pro, one-image PNG/JPEG output, exact certified dimensions and reviewed pricing, zero generation retries, no fallback, polling, delivery, and recovery into one versioned contract.",
      "Report configured, credential-reference-present, production-validated, and live-qualified as independent facts; live-qualified stays false and readiness never calls BFL.",
      "Require only an opaque operator-owned Keychain coordinate and explicit maximum-spend and live-spend review choices.",
      "Reject missing or mutated profile inputs before claim, persistence, provider-attempt creation, or provider work.",
      "Keep Hubu and Gongbu processes, databases, credentials, targets, attempts, artifacts, and failure domains isolated.",
      "Expose one agent-facing MCP surface without collapsing backend ownership.",
    ],
    links: [
      ["Supported profile contract", "contracts/provider-profiles-v1.json"],
      ["Source doctor and catalog", "crates/hubu-cli/src/stack/doctor.rs"],
      ["Production validator", "crates/gongbu-api/src/provider/supported_profiles.rs"],
      ["Managed FLUX runbook", "docs/operations/managed-flux-profile.md"],
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
    copy: "hubu-unified-mcp routing revision 5 is a thin stdio adapter over separate Hubu and Gongbu HTTP clients. Its static ownership table routes 36 backend-owned tools, including the sanitized gongbu_get_provider_catalog read, while exposing 40 tools in total and failing closed on unknown names or incompatible versions.",
    diagram: [
      ["Agent harness", "stdio client"],
      ["Capability probe", "compatibility"],
      ["Static router", "40 tools; 36 backend routes"],
      ["Hubu client", "Hubu credential"],
      ["Gongbu client", "Gongbu credential"],
      ["Provider catalog", "sanitized exact contract"],
    ],
    responsibilities: [
      "Probe Hubu and Gongbu independently for compatibility.",
      "Publish one sanitized, availability-aware tool catalog.",
      "Route the 31 Hubu tools, including safe dynamic budget update and history paths, without merging backend ownership.",
      "Return recursively redacted typed budget rejections as MCP tool errors while preserving existing behavior for other Hubu tools.",
      "Return exact target, model, dimensions, currency, frozen prices, policies, and readiness without exposing credential coordinates or calling a provider.",
      "Never forward one backend credential to the other.",
      "Preserve separate backend clients, state, failure domains, results, and error semantics.",
    ],
    links: [
      ["Unified MCP guide", "docs/unified-mcp.md"],
      ["Router implementation", "crates/hubu-unified-mcp/src/lib.rs"],
      ["Hubu routing", "crates/hubu-unified-mcp/src/hubu/routing.rs"],
      ["Gongbu catalog", "crates/hubu-unified-mcp/src/gongbu/catalog.rs"],
      ["Managed FLUX runbook", "docs/operations/managed-flux-profile.md"],
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
    copy: "Gongbu production-validates the exact managed profile at startup, then owns provider work end to end: isolated credentials and targets, pricing, Temporal workflows, submit-once generation, safe operation checkpoints, execution attempts, receipts, and artifacts. Hubu never opens Gongbu state.",
    diagram: [
      ["Production validator", "exact target + price + policies"],
      ["FLUX tuple", "flux · flux2_api · flux-2-pro"],
      ["Certified outputs", "1 PNG/JPEG · 1k/2k/4k"],
      ["Frozen USD prices", "3/1 · 45/10 · 75/10 minor"],
      ["Mixed provider catalog", "Gemini + FLUX isolated"],
      ["Execution record", "persist first"],
      ["submit_provider", "generation POST once"],
      ["Operation checkpoint", "safe ID + host + deadline"],
      ["poll_provider_operation", "resume status GET"],
      ["Receipt + artifacts", "execution evidence"],
    ],
    responsibilities: [
      "Validate the exact provider, adapter, pinned model, one-image PNG/JPEG capability, 1k 1024×1024, 2k 1920×1088, and 4k 2048×2048 dimensions, their frozen USD prices, zero retries, no fallback, and fixed poll, artifact, and recovery policies before startup.",
      "Keep simultaneous Gemini and FLUX Keychain coordinates, target revisions, provider attempts, and execution-bound artifacts distinct so either provider can fail without routing into the other.",
      "Validate and exclusively claim Hubu authorization.",
      "Patch new Temporal histories into separate submit-once and poll-existing-operation activities while preserving synchronous adapters.",
      "Resume a checkpointed provider operation under its original deadline without another generation submission.",
      "Reconcile an ambiguous post-transmission interruption before checkpoint instead of resubmitting or releasing.",
      "Price actual work and persist safe receipt metadata.",
      "Keep credentials, raw bodies, URLs, and storage paths out of durable workflow payloads.",
      "Own artifact storage and provider recovery as one failure domain.",
      "Remain a separate process, database, credential, artifact, and failure domain from Hubu while retaining submit-once and resume-poll recovery.",
    ],
    links: [
      ["Execution plane guide", "docs/gongbu-execution.md"],
      ["Server runbook", "docs/operations/gongbu-server.md"],
      ["Workflow implementation", "crates/gongbu-api/src/workflow.rs"],
      ["Temporal activities", "crates/gongbu-api/src/temporal.rs"],
      ["Provider boundary", "crates/gongbu-api/src/provider/mod.rs"],
      ["FLUX adapter", "crates/gongbu-api/src/provider/flux2_api.rs"],
      ["Supported profile contract", "contracts/provider-profiles-v1.json"],
      ["Production validator", "crates/gongbu-api/src/provider/supported_profiles.rs"],
      ["Managed FLUX runbook", "docs/operations/managed-flux-profile.md"],
      ["Artifact service", "crates/gongbu-api/src/artifact/mod.rs"],
    ],
  },
  provider: {
    kind: "EXTERNAL EDGE",
    title: "Treat provider work as evidence",
    copy: "External providers receive only the credentials and requests owned by Gongbu. Catalog and readiness inspection never calls BFL, and missing credential references, pricing, delivery policy, poll policy, or unsupported options fail before claim or provider work. Provider responses become priced execution evidence and artifact references—not a second source of governance truth.",
    diagram: [
      ["Target + price gate", "operator config"],
      ["Generation submit", "one billable POST"],
      ["Safe checkpoint", "operation ID"],
      ["Status polling", "same operation GET"],
      ["Receipt + artifact", "sanitized evidence"],
      ["Hubu settlement", "financial finality"],
    ],
    responsibilities: [
      "Remain outside both Hubu and Gongbu trust boundaries.",
      "Receive no request from catalog or readiness checks; those reads expose only the sanitized fixed contract.",
      "Reject missing supported-profile inputs and unsupported options before claim, persistence, or provider activity.",
      "Never infer that a missing submit response makes another generation safe.",
      "Resume only a durably checkpointed asynchronous operation; otherwise reconcile.",
      "Return sanitized receipt metadata and artifacts to Gongbu.",
    ],
    links: [
      ["Live provider testing", "docs/operations/live-provider-testing.md"],
      ["Provider adapters", "crates/gongbu-api/src/provider/mod.rs"],
      ["Supported profile contract", "contracts/provider-profiles-v1.json"],
      ["Production validator", "crates/gongbu-api/src/provider/supported_profiles.rs"],
      ["Managed FLUX runbook", "docs/operations/managed-flux-profile.md"],
    ],
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
