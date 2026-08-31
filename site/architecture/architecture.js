const repository = "https://github.com/hacker-no-ice/hubu/blob/main/";

const components = {
  registration: {
    kind: "IDENTITY & RELATIONSHIPS",
    title: "One owner, four agent records",
    copy: "Registration resolves a stable agent lineage, its versions, one spending account, and a fresh session under the active human owner. Fingerprints make the reusable records deterministic.",
    diagramMode: "relations",
    diagram: [
      ["Owner user", "1 → many", "Agent identities"],
      ["Agent identity", "1 → many", "Agent versions"],
      ["Agent identity", "1 → 1", "Agent account"],
      ["Agent identity", "1 → many", "Agent sessions"],
      ["Agent session", "many → 1", "Selected agent version"],
    ],
    responsibilities: [
      "Publish compact guidance that separates human inputs from client-derived runtime fields.",
      "Canonicalize identity and version payloads and recompute both fingerprints on the server.",
      "Reuse matching identity, version, and account records; create a fresh session for every successful registration.",
      "Bind every record to the active human owner without selecting a Gongbu startup principal.",
    ],
    links: [
      ["Registration protocol", "docs/agent-registration.md"],
      ["Registration manager", "crates/hubu-core/src/registration/manager.rs"],
      ["Registration model", "crates/hubu-core/src/registration/model.rs"],
    ],
  },
  policy: {
    kind: "RULES & EVALUATION",
    title: "Evaluate every rule, then decide",
    copy: "Hubu selects the assigned immutable policy revision, validates it, evaluates every typed rule against the canonical spend request, and returns a decision with an auditable trace.",
    diagram: [
      ["Policy resource", "stable identity + revision history"],
      ["Assignment", "agent override → user default"],
      ["SpendRequest + policy", "canonical scope + selected revision"],
      ["Validation", "rule fields + condition types"],
      ["Rule evaluation", "every condition → RuleResult"],
      ["Effect precedence", "deny > needs approval > allow > default"],
      ["Evaluation", "decision + reasons + trace"],
    ],
    responsibilities: [
      "Keep authored rules in immutable revisions while assignments choose the current policy.",
      "Validate typed condition trees before evaluation.",
      "Evaluate every rule; rule order does not determine the result.",
      "Merge matched effects with deny-first precedence and preserve reasons plus RuleResult trace.",
    ],
    links: [
      ["Policy guide", "docs/policy-engine.md"],
      ["Policy engine", "crates/hubu-core/src/policy/engine.rs"],
      ["Policy model", "crates/hubu-core/src/policy/model.rs"],
      ["Policy conditions", "crates/hubu-core/src/policy/condition.rs"],
    ],
  },
  budgets: {
    kind: "BUDGET LIFECYCLE",
    title: "Version, reserve, then finalize",
    copy: "A logical budget keeps immutable limit versions and one cumulative balance. Allowed work freezes the authorized maximum; finalization settles actual cost, releases confirmed non-billing, or preserves uncertainty for reconciliation.",
    diagram: [
      ["Logical budget", "stable bgt_ identity"],
      ["Immutable version", "limit + effective window"],
      ["Derived availability", "state + time + balance"],
      ["Frozen hold", "authorized maximum"],
      ["Executor claim", "exclusive lease"],
      ["Final outcome", "settle · release · reconcile"],
      ["Balance + audit", "consumed · released · evidence"],
    ],
    responsibilities: [
      "Append limit changes with compare-and-set revision checks without resetting consumed or frozen usage.",
      "Derive availability by precedence—revoked, scheduled, expired, exhausted, then active—not as a transition sequence.",
      "Reserve capacity atomically only while the selected budget is effectively active.",
      "Settle exact confirmed cost and release unused capacity; release the hold only after confirmed non-billing.",
      "Keep ambiguous billing and legitimate overruns frozen for explicit human reconciliation.",
    ],
    links: [
      ["Spend lifecycle", "docs/spend-lifecycle.md"],
      ["Budget manager", "crates/hubu-core/src/budget/manager.rs"],
      ["Budget model", "crates/hubu-core/src/budget/model.rs"],
      ["Executor claims", "crates/hubu-core/src/app/executor_claim.rs"],
    ],
  },
  executor: {
    kind: "GONGBU EXECUTOR",
    title: "Execute only approved work",
    copy: "At startup, Gongbu production-validates the supported provider profile. For approved work, it derives the target and price, exact-matches Hubu authority, persists execution before scheduling, and owns submit-once provider work through receipt and artifact evidence.",
    diagram: [
      ["Unified MCP / caller", "authorization + execution intent"],
      ["Supported profile validation", "target + price + execution policies"],
      ["Hubu resolution", "exact-match authority"],
      ["Execution record", "persist before scheduling"],
      ["Temporal workflow", "claim authorization"],
      ["Provider submit", "generation POST once"],
      ["Operation checkpoint", "provider ID + host + deadline"],
      ["Resume polling", "same provider operation · read-only polling"],
      ["Receipt + artifacts", "persist exact evidence"],
      ["Hubu finalization", "settle or release; otherwise human review"],
    ],
    responsibilities: [
      "Validate the exact supported-profile contract and reject missing or mutated target, pricing, polling, delivery, retry, or fallback inputs before claim, persistence, or provider work.",
      "Reject work whose target, scope, amount, operation identity, or price differs from Hubu authorization.",
      "Claim the authorization durably, submit asynchronous generation once, and checkpoint the safe provider operation before polling.",
      "Recover Temporal work by resuming read-only polling of the same operation; never resubmit after an ambiguous post-transmission interruption or route to a fallback.",
      "Own Temporal state, provider credentials, attempts, checkpoints, receipts, artifacts, and the Gongbu database.",
      "Persist exact cost and pricing evidence before asking Hubu to settle or release; uncertain and over-limit outcomes remain queued for protected human reconciliation.",
    ],
    links: [
      ["Execution plane guide", "docs/gongbu-execution.md"],
      ["Managed FLUX runbook", "docs/operations/managed-flux-profile.md"],
      ["Supported profile contract", "contracts/provider-profiles-v1.json"],
      ["Production validator", "crates/gongbu-api/src/provider/supported_profiles.rs"],
      ["Temporal activities", "crates/gongbu-api/src/temporal.rs"],
      ["FLUX adapter", "crates/gongbu-api/src/provider/flux2_api.rs"],
      ["Spend lifecycle", "docs/spend-lifecycle.md"],
    ],
  },
};

const tabs = [...document.querySelectorAll("[role='tab'][data-component]")];

function selectComponent(componentName, focus = false) {
  const component = components[componentName];
  if (!component) return;

  tabs.forEach((tab) => {
    const selected = tab.dataset.component === componentName;
    tab.setAttribute("aria-selected", selected ? "true" : "false");
    tab.tabIndex = selected ? 0 : -1;
    if (selected && focus) tab.focus();
  });

  const selectedTab = tabs.find((tab) => tab.dataset.component === componentName);
  document.querySelector("#component-panel").setAttribute("aria-labelledby", selectedTab.id);

  document.querySelector("#detail-kind").textContent = component.kind;
  document.querySelector("#detail-title").textContent = component.title;
  document.querySelector("#detail-copy").textContent = component.copy;

  const diagram = document.querySelector("#detail-diagram");
  const isRelations = component.diagramMode === "relations";
  diagram.classList.toggle("is-relations", isRelations);
  diagram.setAttribute(
    "aria-label",
    isRelations ? "Selected component entity relationships" : "Selected component internal flow",
  );
  diagram.replaceChildren(
    ...component.diagram.map((entry) => {
      const item = document.createElement("li");
      if (isRelations) {
        const [source, relation, target] = entry;
        const sourceName = document.createElement("strong");
        const relationName = document.createElement("span");
        const targetName = document.createElement("b");
        sourceName.textContent = source;
        relationName.textContent = relation;
        targetName.textContent = target;
        item.append(sourceName, relationName, targetName);
      } else {
        const [label, detail] = entry;
        const title = document.createElement("strong");
        const description = document.createElement("span");
        title.textContent = label;
        description.textContent = detail;
        item.append(title, description);
      }
      return item;
    }),
  );

  document.querySelector("#detail-responsibilities").replaceChildren(
    ...component.responsibilities.map((text) =>
      Object.assign(document.createElement("li"), { textContent: text }),
    ),
  );
  document.querySelector("#detail-links").replaceChildren(
    ...component.links.map(([label, path]) => {
      const item = document.createElement("li");
      const link = document.createElement("a");
      link.href = repository + path;
      link.target = "_blank";
      link.rel = "noreferrer";
      link.textContent = `${label} ↗`;
      item.append(link);
      return item;
    }),
  );
}

tabs.forEach((tab, index) => {
  tab.addEventListener("click", () => selectComponent(tab.dataset.component));
  tab.addEventListener("keydown", (event) => {
    let nextIndex = null;
    if (event.key === "ArrowRight" || event.key === "ArrowDown") nextIndex = (index + 1) % tabs.length;
    if (event.key === "ArrowLeft" || event.key === "ArrowUp") nextIndex = (index - 1 + tabs.length) % tabs.length;
    if (event.key === "Home") nextIndex = 0;
    if (event.key === "End") nextIndex = tabs.length - 1;
    if (nextIndex === null) return;
    event.preventDefault();
    selectComponent(tabs[nextIndex].dataset.component, true);
  });
});

selectComponent("registration");
