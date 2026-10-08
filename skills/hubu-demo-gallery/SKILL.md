---
name: hubu-demo-gallery
description: Automatically export existing FLUX and Gemini images to a local demo gallery with exact Hubu settled costs during an activated gallery session. Uses the native client; does not authorize spend or execute providers.
---

# Hubu demo gallery

Activate once when the user requests a demo gallery session and gives an
absolute output directory. Keep that directory throughout the session. After
**every** successful FLUX or Gemini operation, run the native export command
before completing the turn, without asking for another save prompt. Preserve
the generation task's spend and approval rules; gallery activation grants only
local export of existing images.

## Export a completed operation

Retain the public `operation_handle` returned by
`hubu_submit_governed_execution`. If approval is pending, resolve approval by
the session's authorized flow and continue with `hubu_resume_operation` using
that same handle. Observe it through `hubu_operation_status` until
`state: succeeded` and `terminal: true`. Gallery export does **not** resume,
execute, authorize, settle or reconcile work. Never rerun a provider to obtain
an export.

Call the installed native command with only the public handle, output directory
and the session's labels:

```sh
hubu gallery export \
  --operation-handle 'hubu:public-operation:v1:EXACT_HANDLE' \
  --output /absolute/demo/gallery --tier draft --size 2k
```

The command uses the selected initialized stack profile. Use
`--stack-profile /absolute/profile` after `gallery export` to select an explicit
profile. The profile supplies its separate Hubu/Gongbu endpoints, credential
files and existing unified operation registry. It does not grant provider keys
or approval authority to the exporter.

For a manual setup whose shell already has the same backend configuration and
`HUBU_UNIFIED_OPERATION_STATE_PATH` as the agent's unified MCP server, use:

```sh
hubu-unified-mcp gallery export \
  --operation-handle 'hubu:public-operation:v1:EXACT_HANDLE' \
  --output /absolute/demo/gallery --tier draft --size 2k
```

Use `draft`/`final` from the session's selected demo policy or generation intent.
Use the `image_size` you submitted (`512`, `1k`, `2k`, `4k`). The registry does
not keep it after success, so this label is yours; the receipt also records
Gongbu's pixel width and height. If the request omitted `image_size`, use
`custom` rather than guessing the provider's default. The tier is a human-readable
session label, not a new policy decision.

**Never write base64 or an artifact bundle.** Do not copy MCP image content,
construct image bytes, invoke `gongbu_get_artifact` for export, or download a
provider artifact URL. The native client fetches existing bytes directly from
Gongbu and keeps them out of the model. Its output contains only local paths
and safe receipt metadata; no per-image save prompt or model-to-file image
transfer is needed. Inline previews remain separate in HUB-201.

The native client opens the operation registry read-only and starts no MCP
server or worker. It obtains the bound execution and authorization IDs itself,
reads canonical Hubu ledger pages, retrieves PNG/JPEG artifacts, verifies
execution identity/size/SHA-256, and re-reads authorization accounting after the
artifact fetches. Filenames use exact `effective_cost`, including fractional
cents, rather than an estimate, reservation or rounded budget charge. The
ledger and receipt use canonical lowercase `usd`.

Retry the same export command after an interrupted local write; existing
`(execution_id, artifact_id)` exports are idempotent. Shared sequence numbers,
atomic image writes, `.gallery.json` and receipt sidecars are maintained by the
client. Keep them together. Examples: `03-flux-2k-draft-6c.png` and
`04-gemini-1k-draft-0.1c.png`.

A filename carries the **operation's total expense at export time**. If one
operation returns multiple images, every filename carries the same total; do
not add those labels as per-image prices. Sidecars make this explicit. Later
ledger corrections require explicit gallery reconciliation.

If accounting changed during export, retry the read-only command once. If it
changes again, or evidence is mismatched, corrected, non-USD, or missing, report
export pending reconciliation and retain the existing operation. The native
command never repairs financial state or fabricates a cost. If no export command
is installed, report the missing release capability; do not fall back to
copying image blocks through the model.

## Session activation and gallery pane

From a checkout this skill is discoverable through `.agents/skills`. Outside the
repository, copy `skills/hubu-demo-gallery` to the harness's personal skills
directory and start a new session, for example:

```sh
cp -R skills/hubu-demo-gallery "${CODEX_HOME:-$HOME/.codex}/skills/hubu-demo-gallery"
```

Install Hubu and `hubu-unified-mcp` from the same release containing native gallery
export. Activate once with a standing instruction such as:

> Use $hubu-demo-gallery throughout this session. Export every FLUX and Gemini
> result automatically to /absolute/demo/gallery. Use draft/final from my demo
> policy. Narrate Hubu decisions and report export failures.

Before recording, open the dedicated folder in Finder Gallery view (Command-4)
or another auto-refreshing image viewer. Filter to image Kind to hide receipt
sidecars if desired. Confirm the selected viewer refreshes when images land.
Native locking currently supports macOS/Linux. Mock native fetch-to-file tests
run with `cargo test -p hubu-unified-mcp --test gallery_export --locked` from the
repository root. Live provider/gallery acceptance still requires an authorized
FLUX image, Gemini image and approval/resume operation in the recording profile;
this skill grants no provider spend.
