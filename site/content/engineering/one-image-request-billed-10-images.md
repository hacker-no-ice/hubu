On November 11, Hubu authorized two Gemini 3 Pro Image requests for **$0.27**. Google billed **$1.92**, about 7× the authorization, because it returned 14 images where each request asked for one.

The money was from my own pocket, and it is small. The guarantee it broke is not: Hubu promises that an agent's spend is authorized before it happens, for no more than the authorized amount. This request could cost more than Hubu authorized, and nothing stopped it. This post covers what happened, why, what I learned about how Gemini bills image generation, and what I'm changing.

## What happened

I was comparing image models for Hubu's mascot. An agent asked Hubu to approve two image edits through Gemini 3 Pro Image: one embroidered patch and one sticker, each at 1K, each from one reference image. Both prompts said "Create one". Hubu approved each at $0.135, the price of one 1K image, and delegated the calls to Gongbu, its executor. The agent never calls Gongbu directly.

| Request | Images returned | Billed | Authorized |
| --- | --: | --: | --: |
| Patch | 4 | $0.55 | $0.135 |
| Sticker | 10 | $1.37 | $0.135 |
| **Total** | **14** | **$1.92** | **$0.27** |

Gongbu stopped waiting after 30 seconds, but Google kept generating and finished 43 and 103 seconds after submission. Because Gongbu never saw the responses, the check that would have flagged extra images never ran. I noticed the timeouts right away, but Hubu showed them as requests with an unknown outcome, not as an overrun. Eight days later, while reconciling outstanding timed-out spend, I checked Google's billing and found the real charge.

## What held

- **No retries.** Gongbu sent exactly one request per authorization. A timed-out request with an unknown outcome is never resent, so the overrun didn't double.
- **The real cost was recorded.** Hubu kept the exact amount Google billed, not the quote, and charged it to the agent's budget in full.
- **The overrun is visible.** Hubu's claim that overruns are recorded and reconciled by the owner held.

## Why it happened

**I modeled a metered provider as fixed-price.** Gemini bills by token, and the model decides how many images to produce. My provider pack, the data file that tells Gongbu how to call and price a provider, quoted one image and nothing else. The request sent no limit on images, tokens or dollars, because the pack format had no way to say a provider's cost is unbounded.

**"Verified" didn't mean bounded.** The pack was marked verified, but verification checked that requests and prices were correct for the expected response. It never asked what the most a request could cost was, or what enforced that limit. I noted the unpriced token costs when the pack shipped and accepted them as a known gap.

**Hubu's only cost check ran after the money was spent.** Gongbu compares the response with the quote, but only once it has the response. Here a 30-second timeout meant it never did. A longer timeout helps Gongbu see the response, but it doesn't limit what Google bills.

## What I learned about Gemini 3 Pro Image billing

If you call Gemini 3 Pro Image directly, this applies to you too.

- **A one-image request can return, and bill, several images.** Each response alternated a thinking step with a finished image, 4 and 10 times. Every image was billed at 1,120 tokens. The API marks none of them as drafts, and there is no parameter to limit the count; candidate counts are rejected for image models. Google's docs describe up to two interim thinking images that are not charged ([image generation docs](https://ai.google.dev/gemini-api/docs/image-generation)). Other developers have reported the same behavior since June ([forum report](https://discuss.ai.google.dev/t/gemini-3-pro-image-sometimes-returns-multiple-full-image-outputs-for-one-call-up-to-7-all-billed/173790)).
- **Complex edits seem to trigger it.** My more constrained prompt produced more images. A third-party provider found plain generation and simple edits return one image, while complex edits with many constraints return 2 to 10, and "output only one image" in the prompt is largely ignored ([apiyi guide](https://docs.apiyi.com/en/api-capabilities/nano-banana-dev-guide)). I couldn't reproduce an exact count, so treat it as nondeterministic.
- **There are extra tokens the usage report doesn't explain.** Beyond the image tokens, each response billed about 200 text-rate tokens per image, while reporting zero thinking tokens and containing no text.
- **The real ceiling is the output-token limit.** Google limits output images per request by the model's 32,768 output tokens ([model spec](https://docs.cloud.google.com/gemini-enterprise-agent-platform/models/gemini/3-pro-image)). That allows up to 29 images at 1K, or about $3.93 of output per request: 29× the price of the one image you asked for.

I'm reporting this to Google and asking whether it's intended and what the unexplained tokens are. If you depend on predictable image costs, budget for the worst case, not the expected one.

## What's changing

The new rule: **Gongbu won't submit governed work unless the provider enforces a cost ceiling and Hubu has authorized that ceiling.**

| Change | Status |
| --- | --- |
| Block Gemini image generation and editing until a ceiling is verified | Shipping in v0.3.0 |
| Every provider pack declares whether its cost is fixed, bounded or unbounded; unbounded packs can't run | Planned for v0.3.1 |
| Hubu reserves the worst-case cost across everything billable, not the expected cost | Planned for v0.3.1 |
| "Verified" requires a worst-case cost derivation and a test where the provider returns many times the expected output | Planned for v0.3.1 |
| Pause a provider and alert the owner as soon as a request costs more than authorized | Planned for v0.3.1 |
| Charge budgets from the provider's reported usage, never the quote; reconcile when usage is missing | Planned for v0.3.1 |
| Keep every image a provider returns, mark the last as the result, and record requested vs returned | Planned for v0.3.1 |

The last two rows are lessons for anyone building on metered providers. **Settle from what the provider reports.** Google's usage data was enough to reproduce the bill to seven decimal places, so the quote should only ever be used to authorize, never to charge. **Expect more output than you asked for, and keep it.** Every extra image was paid for; throwing them away wastes money and hides the evidence that something went wrong.

I audited all 20 verified provider packs. The 4 Gemini packs are unbounded. The 16 Black Forest Labs (FLUX) packs are bounded: BFL bills per image at a published price, its API generates one image per request, and it reports each request's cost ([BFL pricing](https://bfl.ai/pricing)).

To bring Gemini back, I'll either verify that `max_output_tokens` caps everything billed, so Hubu can authorize a tight ceiling, or reserve the documented worst case of about $4 per request.
