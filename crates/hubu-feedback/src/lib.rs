//! Pure local report preparation: no network, environment, files, or backend access.
use serde::Deserialize;
use serde_json::{json, Map, Value};

pub const GUIDE_URL: &str = "https://github.com/hacker-no-ice/hubu/blob/main/docs/feedback.md";
pub const GUIDANCE_TOOL: &str = "hubu_feedback_guidance";
pub const PREPARE_TOOL: &str = "hubu_prepare_feedback";

pub fn input_schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false,
        "required": ["trying", "happened"],
        "properties": {
            "trying": {"type": "string", "minLength": 1, "maxLength": 4000, "description": "What were you trying to do? Describe behavior; do not paste prompts, credentials or logs."},
            "happened": {"type": "string", "minLength": 1, "maxLength": 4000, "description": "What happened? Describe actual behavior without raw logs."},
            "kind": {"type": "string", "enum": ["bug", "idea", "private"], "default": "bug"},
            "diagnostics": {"type": "object", "description": "Optional safe projection only. Unknown/unsafe fields are omitted and named only by a generic warning.", "properties": {
                "operation_handle": {"type": "string", "description": "Only a hubu:public-operation:v1:<32 hex> handle; never an operation key or private backend ID."},
                "error_code": {"type": "string", "description": "Stable lowercase snake_case error code, never an error message."}
            }, "additionalProperties": false}
        }
    })
}

pub fn guidance() -> Value {
    json!({
        "schema_version": "hubu-feedback-v1", "entry_point": GUIDE_URL,
        "required_prompts": ["What were you trying to do?", "What happened?"],
        "prepare_tool": PREPARE_TOOL, "input_schema": input_schema(),
        "destinations": {
            "bug": {"url":"https://github.com/hacker-no-ice/hubu/issues/new?template=bug.md", "visibility":"public"},
            "idea": {"url":"https://github.com/hacker-no-ice/hubu/issues/new?template=idea.md", "visibility":"public"},
            "private_support": {"url":null, "available":false},
            "vulnerability": {"url":"https://github.com/hacker-no-ice/hubu/security/advisories/new", "visibility":"private", "scope":"Security vulnerabilities only; not general billing support."}
        },
        "diagnostics": "Client adds its build version and OS/architecture. Optionally copy only operation_handle and error_code from an existing safe public result. Missing diagnostics are fine. Do not fetch logs, prompts, credentials, signed URLs or private backend identifiers.",
        "review": "Show the entire returned preview, including destination, title, body, diagnostics and warnings. Obtain explicit user authorization for this exact content and destination before using any external connector. Changed content requires a new review.",
        "submission": "These tools never send, open a browser or upload attachments. Use an authorized existing connector or manually copy the reviewed title/body to the destination. No GitHub account: use the private route if available.",
        "manual_fallback": GUIDE_URL
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    trying: String,
    happened: String,
    #[serde(default = "default_kind")]
    kind: String,
    #[serde(default)]
    diagnostics: Map<String, Value>,
}
fn default_kind() -> String {
    "bug".into()
}

// Defense in depth only. Unstructured prose cannot be certified secret-free;
// it is always explicitly provided and reviewed, never harvested from execution.
fn redact(text: &str) -> (String, bool) {
    let mut changed = false;
    let lines = text
        .lines()
        .map(|line| {
            let lower = line.to_ascii_lowercase();
            if [
                "https://",
                "http://",
                "bearer ",
                "authorization",
                "password",
                "secret",
                "token=",
                "token:",
                "api_key",
                "api-key",
                "sk-",
                "-----begin",
                "prompt:",
                "prompt=",
                "raw_logs",
            ]
            .iter()
            .any(|marker| lower.contains(marker))
            {
                changed = true;
                "[redacted: possible sensitive content]".to_string()
            } else {
                line.chars()
                    .filter(|c| !c.is_control() || *c == '\t')
                    .collect()
            }
        })
        .collect::<Vec<_>>();
    (lines.join("\n"), changed)
}

pub fn prepare(input: Value, version: &str, platform: &str) -> Result<Value, &'static str> {
    let input: Input = serde_json::from_value(input).map_err(|_| {
        "Invalid feedback fields; consult feedback guidance. No report was prepared."
    })?;
    if [&input.trying, &input.happened]
        .iter()
        .any(|s| s.trim().is_empty() || s.chars().count() > 4000)
    {
        return Err("Both behavior descriptions must contain 1–4000 characters.");
    }
    let (destination, visibility, title) = match input.kind.as_str() {
        "bug" => (
            Some("https://github.com/hacker-no-ice/hubu/issues/new?template=bug.md"),
            "public",
            "Bug report",
        ),
        "idea" => (
            Some("https://github.com/hacker-no-ice/hubu/issues/new?template=idea.md"),
            "public",
            "Idea for Hubu",
        ),
        "private" => (None, "private", "Private support report"),
        _ => return Err("Feedback kind must be bug, idea or private."),
    };
    let mut warnings = vec!["Review all prose for sensitive information. Pattern redaction cannot detect every secret or prompt. No attachments are included."];
    let (trying, r1) = redact(&input.trying);
    let (happened, r2) = redact(&input.happened);
    if r1 || r2 {
        warnings.push(
            "Potentially sensitive description lines were redacted; review the resulting text.",
        );
    }
    let mut diagnostics = json!({"client_version": version, "platform": platform});
    for (key, value) in input.diagnostics {
        let safe = value.as_str().is_some_and(|s| match key.as_str() {
            "operation_handle" => s
                .strip_prefix("hubu:public-operation:v1:")
                .is_some_and(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())),
            "error_code" => {
                (3..=80).contains(&s.len())
                    && s.contains('_')
                    && s.bytes()
                        .all(|b| b.is_ascii_lowercase() || b == b'_' || b.is_ascii_digit())
            }
            _ => false,
        });
        if safe {
            diagnostics[&key] = value;
        } else if !warnings.contains(&"Unavailable, unknown or unsafe diagnostics were omitted.") {
            warnings.push("Unavailable, unknown or unsafe diagnostics were omitted.");
        }
    }
    if destination.is_none() {
        warnings.push("Private support destination is not configured. Keep this draft local; never use a public issue as fallback for sensitive details.");
    }
    let body = format!("## What were you trying to do?\n\n{trying}\n\n## What happened?\n\n{happened}\n\n## Reviewed diagnostics\n\n```json\n{}\n```\n", serde_json::to_string_pretty(&diagnostics).expect("diagnostics serialize"));
    Ok(json!({
        "schema_version": "hubu-feedback-v1", "status": "prepared_not_sent",
        "destination": {"url": destination, "visibility": visibility},
        "title": title, "body": body, "diagnostics": diagnostics, "warnings": warnings,
        "requires_user_authorization": true, "attachments": [], "manual_fallback": GUIDE_URL
    }))
}

pub fn tool_definitions() -> Vec<Value> {
    [(GUIDANCE_TOOL, "Discover feedback destinations, required questions, safe fields and review/submission instructions. Works without either backend.", json!({"type":"object", "properties":{}, "additionalProperties":false})),
     (PREPARE_TOOL, "Prepare an offline feedback preview with exact destination and content for human review. Never submits. Use private for billing or sensitive reports. Works without either backend.", input_schema())]
        .into_iter().map(|(name, description, schema)| json!({"name":name, "description":description, "inputSchema":schema, "annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}})).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn general_feedback_needs_no_operation_or_backend() {
        let report = prepare(
            json!({"trying":"See my budget", "happened":"Blank screen"}),
            "test",
            "test-os",
        )
        .unwrap();
        assert_eq!(report["status"], "prepared_not_sent");
        assert_eq!(
            report["diagnostics"],
            json!({"client_version":"test", "platform":"test-os"})
        );
        assert!(report["body"].as_str().unwrap().contains("Blank screen"));
    }
    #[test]
    fn selects_only_safe_diagnostics_and_redacts_prose() {
        let report = prepare(json!({"trying":"Fetch artifact", "happened":"Failed\nhttps://download.invalid/?signature=private\nAuthorization: Bearer credential", "diagnostics": {
            "operation_handle":"hubu:public-operation:v1:0123456789abcdef0123456789abcdef", "error_code":"backend_unavailable", "prompt":"private-prompt", "raw_logs":"private-log", "credential":"private-secret", "download_url":"private-url", "execution_id":"private-id"
        }}), "test", "test-os").unwrap();
        let serialized = report.to_string();
        for secret in [
            "private-prompt",
            "private-log",
            "private-secret",
            "private-url",
            "private-id",
            "signature=",
            "Bearer credential",
        ] {
            assert!(!serialized.contains(secret), "{secret}");
        }
        assert_eq!(report["diagnostics"].as_object().unwrap().len(), 4);
        assert_eq!(report["requires_user_authorization"], true);
    }
    #[test]
    fn unsafe_diagnostics_do_not_block_and_errors_do_not_echo_input() {
        let report = prepare(json!({"trying":"Try", "happened":"Fail", "kind":"private", "diagnostics":{"operation_handle":"private-key", "error_code":"https://secret"}}), "test", "test").unwrap();
        assert_eq!(report["destination"]["visibility"], "private");
        assert_eq!(report["diagnostics"].as_object().unwrap().len(), 2);
        assert!(!report.to_string().contains("private-key"));
        for input in [
            json!({"trying":"", "happened":"secret"}),
            json!({"trying":"ok", "happened":"ok", "prompt":"secret"}),
            json!({"trying":"ok", "happened":"ok", "kind":"secret"}),
        ] {
            assert!(!prepare(input, "test", "test")
                .unwrap_err()
                .contains("secret"));
        }
    }
}
