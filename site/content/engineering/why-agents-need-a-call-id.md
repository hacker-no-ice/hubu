An AI agent that can spend money needs one boring field before anything else: a trusted ID for each tool call. In Hubu, that ID is what stops a retry from becoming a second charge.

I found this out in practice. The same Hubu plugin generated a FLUX image from Codex on my desktop for 3 cents, under budget and with no approval needed. From ChatGPT on my phone, it refused to spend at all:

```text
Hubu spend tools require supported trusted identity from Codex, Claude Code, or the Hubu platform-invocation envelope
```

That refusal is deliberate. This post explains why the per-call ID (Codex's `callId`, Claude Code's `toolUseId`) matters so much, why Hubu won't take it from the tool's own arguments, how the MCP server turns it into an idempotency key, and what I found when I looked at what each client actually sends.

## The question every paid tool call has to answer

When a request to spend money arrives, the server has to decide: is this the same request again, or a new request that happens to look the same?

Both happen all the time with agents:

- **The same request again.** A connection drops after the server charged but before the response arrived, and the client retries. A tunnel reconnects and redelivers. The server restarts mid-operation and the client asks again. The right answer is to return the original result and charge nothing more.
- **A new request that looks the same.** You ask for an origami fox image, like it, and ask for another with exactly the same prompt and settings. The right answer is a second operation and a second charge.

From the arguments alone, these two cases are identical, byte for byte. Get the first one wrong and you double charge. Get the second one wrong and you silently drop work someone asked for.

The only thing that tells them apart is an identifier that belongs to the call itself. It is assigned once, when the agent decides to call the tool, and repeated unchanged every time that call is delivered. That is exactly what an idempotency key is, and it is what payment APIs like Stripe ask their callers to send.

## Why not just add an idempotency_key argument?

The obvious design is to add an `idempotency_key` field to the tool schema and let the agent fill it in. Hubu deliberately doesn't. Tool arguments are written by the model, and a model is the wrong author for an identity that guards money.

- **Models aren't reliable sources of unique values.** A model asked for a random key can repeat one it saw earlier in the conversation, reuse the same placeholder on every call, or make up a fresh value when it retries. Each of those breaks one of the two cases above.
- **Retries are often invisible to the model.** Redelivery after a dropped connection or a tunnel reconnect happens below the conversation. The model never decided to retry, so it can't know to repeat its key.
- **Arguments can be steered.** Anything in the context window can influence what the model writes: a web page, a document, another tool's output. If identity lives in the arguments, a prompt injection can choose it, reusing a key to collide with another operation or rotating keys to defeat deduplication.
- **Identity and intent must stay separate.** Hubu hashes the arguments to check that a retry asks for exactly the same thing. If the identity were one of those arguments, the check would be circular.

So Hubu takes the identity from the harness: the client program that runs the agent loop and actually sends the tool call. In MCP, that travels in the request's `_meta` field, outside the model-authored `arguments`. Codex sets `_meta.callId`. Claude Code sets `_meta["claudecode/toolUseId"]`. A controlled platform can send a typed `hubu.dev/platform-invocation` envelope.

The rule also works in the other direction. If model-authored arguments contain `operation_key`, `callId`, `tool_use_id`, `platform`, `task_id`, `_meta` or similar fields, the call is rejected, and the spend tool schemas don't expose those fields at all. The model describes what to buy, the harness says which call this is, and neither can do the other's job.

## Under the hood: from callId to operation key

Hubu's agent-facing MCP server, `hubu-unified-mcp`, turns the harness's call ID into a private operation key, which the Hubu backend uses as its idempotency key. For every spend call, it runs these steps before any money moves:

1. **Reject identity in the arguments.** The fields listed above are refused if the model wrote them.
2. **Normalize the trusted identity.** It reads `_meta` and requires exactly one supported source. No source means fail closed. Two at once, such as a `callId` and a `toolUseId`, makes the identity ambiguous, and that is refused too. Each identifier is bounded in length and character set, and becomes a `(platform, harness_call_id)` pair such as `(codex, <callId>)`.
3. **Hash the intent.** It canonicalizes the arguments, sorting keys recursively, and computes `sha256(tool name, 0x00, canonical JSON)`. This request hash is a fingerprint of what is being asked for.
4. **Look up the call in a durable registry.** A local SQLite database stores one row per `(platform, installation, harness_call_id)`. The installation is a stable ID the router generates the first time it starts, so separate router installations never collide.
5. **Resolve one of three ways:**
    - **New call ID:** allocate a random private key, `hubu:operation:v1:<platform>:<uuid>`, and a separate random public handle, `hubu:public-operation:v1:<uuid>`. Store both with the request hash, then dispatch.
    - **Same call ID, same request hash:** this is redelivery. If a result is recorded, return it without touching the backend. Otherwise, continue the same operation under the same key.
    - **Same call ID, different request hash:** refuse before reaching the backend. One call can't be reused for different work.
6. **Forward with the private key.** The router injects the operation key into the request to Hubu. The model never sees it, and responses are recursively scrubbed of it. The agent gets only the public handle, which it can use to check status or resume an approval.

<figure>
  <img src="/blogs/why-agents-need-a-call-id/02-call-id-to-operation.png" alt="Flow of a spend tool call through hubu-unified-mcp. The call passes two gates, clean arguments and exactly one trusted ID; failing either refuses it before the backend. A registry lookup by platform and call ID then branches three ways: a new call ID allocates a key and public handle, the same ID with the same hash replays or continues, and the same ID with a new hash is refused. Accepted calls reach Hubu keyed by agent and operation key, which allows at most one provider call per Hubu decision." width="1536" height="1024" loading="lazy" decoding="async">
  <figcaption>Two gates come before any lookup. Of the three outcomes, only a new call ID allocates a key, and everything that reaches Hubu carries the one private key bound to that call.</figcaption>
</figure>

The Hubu backend applies the same rule again. It stores each authorization record under `(agent_id, operation_key)`. An identical retry returns the same record, while a retry under that key with a different amount, account or execution scope is rejected with `create_new_operation`. Downstream, the execution service, Gongbu, keys each provider call to Hubu's decision ID, which also seeds the vendor's idempotency key. One decision leads to at most one provider call.

So a retried FLUX request has to pass three checks, and each must agree it is the same call: the router's registry, Hubu's authorization record and the provider's idempotency key.

Human approval follows the same pattern. If policy says a spend needs approval, the original call becomes replay-only. Approving it resumes the original, immutable operation through its public handle instead of creating a new one, and re-sending the original call can never skip the approval. The full rules are in the [spend lifecycle](/docs/spend-lifecycle) and the [unified MCP reference](/docs/unified-mcp).

## What the clients actually send

Codex and ChatGPT Work send a per-call `callId`; ChatGPT chat does not. That one difference explains the phone failure. The phone itself isn't the cause: ChatGPT Work on the same phone was accepted.

To find this, I added an opt-in diagnostic to the MCP server. On each tool call, it logs only the shape of `_meta`: key names, JSON types, lengths and short SHA-256 digests of the values. It never logs raw values or tool arguments, and it doesn't change whether a call is allowed. Then I made the same read-only capabilities call from each client, signed in as the same OpenAI user.

| Client | Hubu's verdict | Per-call ID | Other call-level keys | Context keys |
| --- | --- | --- | --- | --- |
| Codex (desktop) | accepted | `callId` | `itemId`, `openai/request_id`, `sessionId`, `windowId`, `progressToken` | `openai/session`, `openai/subject`, `openai/organization`, `openai/userLocation` |
| ChatGPT Work (phone) | accepted | `callId` | the same, plus `traceparent` | the same |
| ChatGPT chat (phone) | rejected | none | none | the same, plus `openai/locale`, `openai/userAgent`, timezone |

Three details stood out:

- **All three share the OpenAI context keys.** `openai/session`, `openai/subject` and `openai/organization` appear everywhere. The only difference is whether the call-level fields are filled in.
- **The JSON-RPC request ID isn't a substitute.** On the chat connection it was literally `0`; its digest was the SHA-256 of the string "0". It's a counter that matches a request to its response on one connection, and it resets whenever the connection does.
- **A session ID isn't a substitute either.** `openai/session` is the same for every call in a conversation. If Hubu keyed operations on it, "redeliver this call" and "make another image" would collapse into the same thing, which is the exact ambiguity idempotency exists to resolve.

I can't tell from this data whether ChatGPT chat leaves the field out on purpose. But two OpenAI surfaces built on the same metadata layer do send it, so it looks more like an inconsistency than a limit of the protocol.

## Design tradeoffs

Every choice here gives up some convenience to make double spending structurally hard.

| Decision | What we gain | What we give up |
| --- | --- | --- |
| Fail closed without a per-call ID | No guessing with money, and no silent double charge | Clients like ChatGPT chat can't spend through Hubu today |
| Identity from the harness, never the arguments | Prompt injection and model mistakes can't choose or rotate the key | Each client integration has to be recognized explicitly |
| No fallback to a session ID plus an argument hash | Identical but separate requests stay separate | Clients that only send session context get no partial support |
| Bind the call ID to a request hash | A reused call ID can't be pointed at different work | Any change to the arguments, even a trivial one, has to be a new call |
| Router-generated private key, separate public handle | The backend key never reaches the model, logs or chat transcripts | Two identifiers to explain, and a mapping the router has to keep |
| Durable local registry | Redelivery is recognized even after a crash or restart | Router state must be kept: deleting the registry loses recovery |
| Check again at Hubu and the provider | One bug in one layer doesn't become a double charge | More moving parts, and more contracts to keep aligned |

The biggest tradeoff is the first one. A friendlier system could fall back to "same session plus same arguments within N seconds means a retry". It would work most of the time, but it would wrongly merge a legitimate "do it again" and wrongly split a slow redelivery. For a feature that spends money, "most of the time" isn't good enough, so Hubu refuses the call and says why.

The second biggest is trust. Hubu trusts the harness that launched it to tell the truth in `_meta`. That's reasonable for a local stdio process, but anyone with direct access to the server's input could still forge an envelope. Hubu bounds and types every identifier and never accepts identity from the arguments, but it doesn't verify signatures.

## How this could get better

The fix that would help everyone is a standard, trusted per-call ID in MCP itself, instead of a vendor-specific key for each client.

- **A standard per-call ID.** Today every harness picks its own name: `callId`, `claudecode/toolUseId` and so on. A spec-level field for the tool-call ID, with defined rules, would let servers support every compliant client without special cases.
- **A stated stability guarantee.** An ID only helps if it stays the same on every redelivery of a call and differs for every new call. Clients should document that, including across reconnects and restarts, so servers don't have to infer it from observation.
- **ChatGPT chat parity.** The smallest practical step is for ChatGPT chat to send the same `callId` (or `openai/request_id`) that Codex and Work already send. Hubu could map it with a small change, without a redesign.
- **Verifiable identity.** In the longer term, a client could sign the invocation envelope, or a gateway could vouch for it, so a server doesn't have to trust whoever is on the other end of stdio.
- **More clients, one contract.** Each new client maps its native ID into Hubu's typed `hubu.dev/platform-invocation` envelope, so supporting another client doesn't touch the core deduplication logic.
- **Measured support.** The metadata-shape diagnostic turns "does this client work?" into a five-minute check instead of guesswork, and it can feed a published support matrix with tested versions.

Until ChatGPT chat sends a per-call ID, there are two ways to spend from a phone: use ChatGPT Work, or drive Codex or Claude Code on a desktop through remote control. Either way, the tool call reaches Hubu with a call ID it trusts.

## Takeaways for anyone building tools that spend money

- Decide early how you will tell "the same call again" from "a new call that looks the same". Everything else depends on it.
- Take that identity from the harness, never from model-written arguments, and reject arguments that try to supply it.
- Bind the identity to a hash of the request, so a reused ID can't be pointed at different work.
- Keep the backend key private, and give the agent a separate public handle for status and approvals.
- Make the record durable, and enforce it again at every layer that can move money.
- When the identity is missing, fail closed and say exactly what's missing. An error that names the problem is better than a charge that shouldn't have happened.

The failure on my phone looked like a bug. It turned out to be the safety design working: one client sent the field that makes retries safe, and the other didn't.
