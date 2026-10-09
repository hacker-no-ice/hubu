# Local stack demo: real providers, governed spend

Generate real images through an agent harness, then inspect the decisions and spending behind them.

**0.2.2-dev · 10:53 · MCP + CLI · Real provider calls and charges**

<div class="intro-video-player">
<iframe src="https://www.youtube-nocookie.com/embed/2lLNKL6ykjY" title="Hubu Local Stack Demo: Real Providers, Governed Spend" width="960" height="540" loading="lazy" referrerpolicy="strict-origin-when-cross-origin" allow="encrypted-media; picture-in-picture; fullscreen" allowfullscreen></iframe>
</div>

[Watch on YouTube ↗](https://youtu.be/2lLNKL6ykjY)

## The short version

This companion script summarizes the recording, not installation steps. Waiting periods have been shortened. Amounts describe this recorded configuration, not guaranteed current provider prices.

### 1. Real providers, bounded spending

Hubu governs how AI agents spend money. Here, an agent iterates on a Hubu mascot using FLUX and Gemini. Hubu, Gongbu, and Temporal run locally; image generation uses live, billable providers.

The demo policy permits approved USD image-generation scopes through Gongbu. Requests below 4¢ are automatically allowed, requests above 15¢ are denied, and the middle range requires human approval. These are limits on the requested authorization cap, not the eventual settled cost.

### 2. Watch the budget and decisions

The CLI's `hubu watch` view shows the budget and recent decisions alongside the agent's work. Frozen funds are reserved, not yet spent. Settlement records the actual cost and frees any unused reservation.

### 3. Let a small request proceed

The agent requests a 1K FLUX mascot holding a ledger. Its 3¢ authorization cap is within the automatic-allow tier, so generation proceeds without interrupting the user.

Gongbu executes the authorized work and returns the image. Hubu accounts for the settled cost.

### 4. Stop an expensive request before execution

A 4K Gemini request needs a 16¢ authorization cap. The policy denies it before provider execution: no image is generated and no charge is incurred.

This is a per-request policy limit, not an exhausted budget. Having money remaining does not override the rule.

### 5. Ask a person when policy requires it

A 1K Gemini variation needs a 7¢ cap, so Hubu returns `needs_approval`. A phone notification alerts the user, and the operation waits for an explicit decision.

After approval, that operation proceeds. The recorded provider cost is 6.7¢, below the 7¢ authorization cap. The notification is an alert; approval is a separate decision.

### 6. Inspect the evidence

Compare the generated images and review the decisions, settled costs, and remaining budget. The agent can keep iterating while policy determines which requests proceed, stop, or need a person.

## Mascot gallery

The original mascot is the starting design. The FLUX and Gemini variations below are artifacts from the governed requests in this demo. Select an image to view it full size.

<div class="demo-gallery">
<figure class="original-mascot">
<a href="/brand/hubu-mascot.png"><img src="/brand/hubu-mascot.png" alt="Original cream-and-jade Hubu seal mascot on a cinnabar base" width="1388" height="1133" loading="lazy" decoding="async"></a>
<figcaption><strong>Original mascot</strong><span>The selected seal design, before the demo's iterations.</span></figcaption>
</figure>
<figure>
<a href="/demos/local-stack/mascot-flux-ledger.jpg"><img src="/demos/local-stack/mascot-flux-ledger.jpg" alt="FLUX-generated Hubu mascot holding a green ledger" width="1024" height="1024" loading="lazy" decoding="async"></a>
<figcaption><strong>FLUX · holding a ledger</strong><span>3¢ authorization cap · automatically allowed.</span></figcaption>
</figure>
<figure>
<a href="/demos/local-stack/mascot-gemini-blocked.jpg"><img src="/demos/local-stack/mascot-gemini-blocked.jpg" alt="Gemini-generated Hubu mascot with a stern expression, holding an open ledger" width="1408" height="768" loading="lazy" decoding="async"></a>
<figcaption><strong>Gemini · a stern expression</strong><span>7¢ authorization cap · approved by a person.</span></figcaption>
</figure>
</div>

### Earlier concept sheets

These explorations preceded the recording; they are not additional executions shown in the video. The original seal was selected from the built-in image generator's concept sheet.

<div class="demo-gallery">
<figure>
<a href="/demos/local-stack/concepts-builtin.png"><img src="/demos/local-stack/concepts-builtin.png" alt="Built-in image generator's five Hubu mascot concepts: treasurer, seal, guardian, owl, and ink spirit" width="1536" height="1024" loading="lazy" decoding="async"></a>
<figcaption><strong>Built-in image generator</strong><span>Initial concept sheet, including the original seal.</span></figcaption>
</figure>
<figure>
<a href="/demos/local-stack/concepts-flux.jpg"><img src="/demos/local-stack/concepts-flux.jpg" alt="FLUX concept sheet exploring several jade-and-cinnabar Hubu characters" width="1024" height="1024" loading="lazy" decoding="async"></a>
<figcaption><strong>FLUX · concepts</strong><span>An earlier exploration of the same mascot brief.</span></figcaption>
</figure>
<figure>
<a href="/demos/local-stack/concepts-gemini.jpg"><img src="/demos/local-stack/concepts-gemini.jpg" alt="Gemini concept sheet showing treasurer, seal, guardian, owl, and ink-spirit characters" width="1376" height="768" loading="lazy" decoding="async"></a>
<figcaption><strong>Gemini · concepts</strong><span>An earlier exploration of the same mascot brief.</span></figcaption>
</figure>
</div>

## Try it yourself

Start with the [sandbox demo](sandbox.md) for a no-charge walkthrough and the [local stack quick start](../local-stack.md) for installation.

For live providers, follow the [complete Gemini + FLUX profile](../configuration/local-stack/v1/examples.md#live-gemini-developer-api-and-flux2) and [live provider operations](../operations/live-providers.md). Review provider configuration, credentials, pricing, and spending limits before enabling billable work. This recording uses a development build; current setup steps may differ.

[Explore the source on GitHub ↗](https://github.com/hacker-no-ice/hubu)
