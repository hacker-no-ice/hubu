# Demos

Watch Hubu in use, then read the concise companion script. Each demo focuses on one integration story and the results you can inspect.

## Sandbox: registration to settlement

[Watch the sandbox demo and follow along →](sandbox.md)

**10:42 · CLI + Codex MCP · No real charges**

Register a user and agent, define policy and budget, submit one governed request from Codex, and inspect execution, the artifact, settlement, and ledger. The full stack runs locally with a deterministic mock provider.

This first demo follows the happy path, without provider credentials or charges.

## Local stack: real providers, governed spend

[Watch the local-stack demo and follow along →](local-stack.md)

**10:53 · Agent harness + MCP · FLUX + Gemini · Real provider charges**

Generate images with real providers, see policy allow, deny, and human-approval decisions, then inspect the artifacts, settled costs, and remaining budget. Hubu, Gongbu, and Temporal run locally; provider calls are live.

More focused demos will cover failure handling and recovery.

## Try it yourself

Start with the [local stack quick start](../local-stack.md) to install Hubu and prepare a sandbox. The demo walkthrough begins with an already-running stack; it is separate from the installation guide.
