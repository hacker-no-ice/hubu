# Send feedback

Tell us **what you were trying to do** and **what happened**. Everything else is
optional. You do not need an execution handle or access to our Linear workspace.

- [Report a bug](https://github.com/hacker-no-ice/hubu/issues/new?template=bug.md): public problems, failed executions, installation or display issues.
- [Suggest an idea](https://github.com/hacker-no-ice/hubu/issues/new?template=idea.md): improvements and general feedback.
- Billing or sensitive details: keep your draft private while the private support destination is being established. Do not use public issues for account details.
- Suspected vulnerabilities: [report a vulnerability privately](https://github.com/hacker-no-ice/hubu/security/advisories/new) and follow the [security policy](../SECURITY.md). This form is for security reports, not billing support.

## Prepare and review

Run `hubu feedback` for structured guidance or `hubu feedback --help` for usage.
Preparation works offline, even when neither service is available:

```sh
hubu feedback prepare <<'JSON'
{
  "trying": "Retrieve my generated image",
  "happened": "The operation completed but no image appeared",
  "kind": "bug",
  "diagnostics": {"error_code": "artifact_unavailable"}
}
JSON
```

The preview includes the exact destination, title and Markdown body. It adds
only the preparing client's version and OS/architecture. `kind` can be `bug`,
`idea` or `private`; use `private` for billing or sensitive support. Optional
`diagnostics` accepts only a safe public `operation_handle` and stable
`error_code`. Missing or unsafe diagnostics are omitted with a warning.
No operation lookup, backend database inspection or diagnostic collection occurs.

Review every line and the destination before submitting. Remove credentials,
signed download URLs, prompt contents, personal/account data and raw logs. The
preparer removes lines with common credential/URL markers as a precaution, but
cannot recognize every secret in prose. Describe behavior instead of pasting
execution inputs or error payloads. No attachments are included. If you choose
to add one later, review its exact contents and authorize it separately.

Preparation prints JSON to stdout; it never sends, uploads or opens a browser.
If you save it to a file, treat that file as your private draft.

## With an agent

The single `hubu-unified-mcp` surface always advertises
`hubu_feedback_guidance` and `hubu_prepare_feedback`, including when backends
are unavailable. Ask the agent to report a problem: it should first read the
guidance, collect the two descriptions, and prepare the preview. It may copy
only the public handle and stable error code from an existing safe result;
it must not request prompts, logs, credentials or private backend identifiers.

The agent must show the destination, title, complete body, warnings and any
attachments, then obtain your explicit authorization before an existing
connector submits that exact report. If content or destination changes, review
it again. No execution failure triggers automatic reporting.

## Manual fallback

Without a connector, open the linked bug/idea template, replace its entire
body with the reviewed body, and use the exact reviewed title. Check GitHub's
preview and repository destination before submitting. Without the CLI or
diagnostics, fill in the template's two behavior sections directly instead.
GitHub requires an account. If you cannot sign in, keep your draft until you
can use the private support route; never move sensitive details to a public
channel because a tool failed.

## Maintainer triage

1. Acknowledge public reports within three business days where possible; state
   the next step without promising a fix date. Follow the security policy for
   vulnerability response targets.
2. Check sensitivity first. If sensitive information was posted publicly,
   restrict/remove exposed content using GitHub controls, advise credential
   rotation when relevant, and continue privately. Do not quote the exposure.
3. Identify duplicates, link the existing public report, and preserve new
   reproduction details. Classify bug, idea, billing or security; identify the
   owning Hubu governance, Gongbu execution/artifact, or client component.
4. Turn actionable work into a Linear issue: sanitized expected/actual behavior,
   minimal reproduction, version/platform, safe public handle/error code if
   available, severity, owner and acceptance criteria. Link the intake report
   internally; do not copy private attachments or billing data into public work.
5. Reply on the original authorized channel with a public issue/PR or a short
   status summary. Reporters never need Linear access. Close the loop when a
   fix ships, an idea is declined, or more information is needed.

Maintainer verification: GitHub's public private-vulnerability-reporting API
returned `enabled: true` on 2026-09-05. Recheck the repository setting after
permission/configuration changes; if unavailable, keep security details private
and follow the security policy fallback.
