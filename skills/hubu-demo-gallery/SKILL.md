---
name: hubu-demo-gallery
description: Automatically save existing FLUX and Gemini results to a local demo gallery with exact Hubu settled costs during an activated gallery session. Does not authorize spend or execute providers.
---

# Hubu demo gallery

Activate once when the user asks for a demo gallery session and gives an absolute
output directory. Keep that directory for the session. After **every** successful
FLUX or Gemini operation, export **every** image before completing the turn,
without asking for another save prompt. Preserve the generation task's spend
and approval rules. A gallery session grants local export permission only.

Use existing `hubu-unified-mcp` tools for all backend reads; no provider keys,
Gongbu MCP surface, direct database reads, artifact URL downloads or provider
replays. Inline image previews are handled separately in HUB-201.

## Export each completed operation

1. Retain the exact result of `hubu_submit_governed_execution`. If approval is
   pending, retain its `operation_handle`, resolve approval by the session's
   authorized flow, and continue with `hubu_resume_operation`. Observe the same
   handle using `hubu_get_operation_status` until `state: succeeded` and
   `terminal: true`. Never submit a replacement to obtain an image.
2. Use the latest public `decision_id` from the submit result's
   `structuredContent.authorization` (or the resume result's
   `structuredContent.hubu_result`). Call `hubu_get_authorization_record` with
   that `authorization_id`; wait until the record is `settled` with a receipt.
3. Call `hubu_list_ledger` for the record's `agent_id` and `account_id`. Follow
   `next_cursor` until all `ledger_transaction_ids` from the record are present.
   Preserve the first page's response shape and append subsequent `transactions`
   to its array in the local bundle. Costs come from `effective_cost`, never
   an estimate, authorized maximum, reservation or rounded budget charge.
4. Call `gongbu_list_artifacts` with the completed operation's `execution_id`.
   For each image, call `gongbu_get_artifact` with its `artifact_id`. Preserve
   the full tool result, including text metadata and base64 `image` content.
   The helper verifies size, SHA-256 and execution/settlement identities.
5. Write a private local JSON bundle with these keys (values are exact MCP
   tool result objects, or complete JSON-RPC responses containing `result`):
   - `operation`: succeeded submit/resume/status result.
   - `submission`: latest submit/resume result carrying the public decision ID;
     required if a terminal status result has no authorization projection.
     Its handle must match `operation`. Prefer the authorized resume result
     after human approval, since it can carry a newer authorization revision.
   - `authorization`: `hubu_get_authorization_record` result.
   - `ledger`: combined `hubu_list_ledger` result from step 3.
   - `artifact_list`: `gongbu_list_artifacts` result.
   - `artifact`: one `gongbu_get_artifact` result.
   - `size`: actual requested size, normalized to `512`, `1k`, `2k`, `4k` or
     `custom`. Get it from the submitted image arguments; do not invent a size.
   - `tier`: `draft` or `final`, carried from the session's selected demo policy
     or generation intent. Do not infer it from price alone.
6. Run the bundled helper for every image, using the same output directory:

   ```sh
   python3 /ABSOLUTE/SKILL/PATH/scripts/export_image.py \
     --input /ABSOLUTE/PRIVATE/operation-image.json \
     --output /ABSOLUTE/DEMO/gallery
   ```

Delete the temporary bundle after successful export. Keep `.gallery.json`,
`.gallery.lock`, images and `.receipt.json` sidecars in the dedicated gallery.
The helper makes no network calls. Its inputs must be faithful local copies of
trusted tool responses; it checks their consistency, not their authenticity.
If the harness only exposes truncated image content, stop export and report
that limitation; never fabricate bytes or regenerate the image.

Retry local export from the same results if it is interrupted. Sequence numbers
are shared across providers; `(execution_id, artifact_id)` is idempotent, including
resumed operations. PNG and JPEG extensions match the returned media type.
Example: `03-flux-2k-draft-6c.png`; fractional costs remain exact, such as
`04-gemini-1k-draft-0.1c.png`. Filenames show the **operation's total expense**:
when one operation returns multiple images, each carries that same total; do not
sum those labels as per-image prices. Sidecars make this semantic explicit.

If evidence is missing, non-USD, mismatched, corrected, or multiple linked
postings exist, report the export failure and keep the existing operation.
Corrections need explicit gallery reconciliation; do not quietly relabel a prior
receipt or claim an old filename reflects a corrected total. These demo helpers
never settle or correct ledger entries.

## Session activation and gallery pane

From a checkout, this skill is discoverable through `.agents/skills`. To use it
outside the repository, copy `skills/hubu-demo-gallery` to your harness's personal
skills directory and start a new session. For example, with Codex:

```sh
cp -R skills/hubu-demo-gallery "${CODEX_HOME:-$HOME/.codex}/skills/hubu-demo-gallery"
```

Activate it once with a standing instruction such as:

> Use $hubu-demo-gallery for this entire session. Save every FLUX and Gemini
> output to /absolute/demo/gallery automatically. Use draft/final labels from
> my selected demo policy. Narrate Hubu decisions and report export failures.

Create/select the dedicated directory before recording. On macOS, open it in
Finder, use Gallery view (Command-4), and hide sidecars using the image Kind
filter if desired. The image pane updates as atomic image writes land. Other
platforms can use an auto-refreshing image folder viewer. Confirm refresh in the
chosen viewer during rehearsal. The helper runs on macOS/Linux with Python 3;
it uses POSIX file locks to serialize concurrent exports.

Validate locally with `python3 -m unittest discover -s
skills/hubu-demo-gallery/scripts -p 'test_*.py'`. Live acceptance still requires
one authorized FLUX image, one authorized Gemini image and an approval/resume
image in the configured demo profile; this skill does not grant provider spend.
