# Sandbox demo: registration to settlement

Set spending boundaries in the CLI, run one governed request in Codex, and inspect the result.

**v0.2.2 · 14:40 · Happy path · No real provider calls or charges**

<div class="intro-video-player">
<iframe src="https://www.youtube-nocookie.com/embed/01A1RemvK1A" title="Hubu Sandbox Demo: From Agent Registration to Spend Settlement" width="960" height="540" loading="lazy" referrerpolicy="strict-origin-when-cross-origin" allow="encrypted-media; picture-in-picture; fullscreen" allowfullscreen></iframe>
</div>

[Watch on YouTube ↗](https://youtu.be/01A1RemvK1A)

## The short version

This companion script summarizes the recorded demo; it is not a verbatim transcript or an installation runbook.

### 1. Register the user and agent

Hubu governs how AI agents spend money. In sandbox mode, the full Hubu, Gongbu, and Temporal stack runs locally with a simulated provider.

We check the version and service readiness, then register a user and an agent. Hubu verifies the registration fingerprints. The agent ID identifies the policy and budget subject; its account ID identifies the spending source.

### 2. Set policy and budget

Policy defines what the agent may do. The recorded policy allows only Gongbu's local-fixture image scope, up to 10 simulated cents per request, and denies unmatched requests.

Budget defines how much it may spend. We give the agent one simulated dollar, expiring in 24 hours—a finite window, not a recurring allowance.

### 3. Ask Codex to do the work

We switch to Codex, inspect the MCP tools, and select the same registered agent. CLI and MCP access the same governance state.

```text
use the mock provider to generate an image of a blue dot with 1k resolution
```

Codex submits one request through `hubu_submit_governed_execution`. Hubu checks policy and budget and reserves funds before execution begins.

### 4. Follow execution and settlement

Gongbu resolves the authorization, acquires an exclusive claim, and runs the mock provider through its Temporal workflow. It stores the artifact and reports the receipt to Hubu for settlement.

Codex follows that same operation to completion. The executor handles claim and settlement; the agent does not need to manage those steps itself.

### 5. Inspect the evidence

The sandbox returns a fixed **1×1 PNG fixture**, not a real 1024×1024 generated image. This tests the integration, not image quality.

Back in the CLI, we check the budget and ledger:

```bash
hubu budget list
hubu ledger list
```

The recorded cost is one simulated cent: USD 0.01 consumed, nothing frozen, and USD 0.99 remaining. The ledger records the settled cost; authorization records separately show requests and decisions.

## Try it yourself

People set the boundaries. Agents work within them. Results stay inspectable.

Follow the [local stack quick start](../local-stack.md) to prepare your own sandbox. This video begins with a running stack and focuses on the happy path. Real providers, approval flows, and failure handling will get separate demos.

[Explore the source on GitHub ↗](https://github.com/hacker-no-ice/hubu)
