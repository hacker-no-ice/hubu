use std::{
    env, fs,
    io::{self, BufRead, Read, Write},
    net::{Shutdown, TcpStream},
};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8787";
const AUTH_TOKEN_ENV: &str = "HUBU_AUTH_TOKEN";
const AUTH_TOKEN_FILE_ENV: &str = "HUBU_AUTH_TOKEN_FILE";
const DEFAULT_AUTH_TOKEN_FILE: &str = "hubu.auth-token";
const PROTOCOL_VERSION: &str = "2024-11-05";
const HUBU_APPROVAL_PROFILE_VERSION: &str = "hubu-mcp-client-approval-v1";
const HUBU_MCP_INSTRUCTIONS: &str = "Hubu approval policy: clients should allow read tools and hubu_authorize_spend/hubu_submit_spend without a pre-call human prompt. Protected setup/admin tools require a human approval prompt before tools/call. If a spend response has requires_human_approval=true, no payment was executed; stop and surface it to the human.";
const READ_TOOL_NAMES: &[&str] = &[
    "hubu_health",
    "hubu_registration_guidance",
    "hubu_client_approval_profile",
    "hubu_list_users",
    "hubu_list_agents",
    "hubu_list_budgets",
    "hubu_list_ledger",
];
const SPEND_TOOL_NAMES: &[&str] = &["hubu_authorize_spend", "hubu_submit_spend"];
const APPROVAL_TOOL_NAMES: &[&str] = &[
    "hubu_register_human",
    "hubu_register_agent",
    "hubu_add_policy",
    "hubu_create_budget",
    "hubu_create_recurring_budget",
];

#[derive(Debug, Clone, Copy)]
struct McpConfig {
    protected_tools_enabled: bool,
}

pub fn run_stdio_from_env() -> Result<()> {
    let base_url = env::var("HUBU_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
    let config = McpConfig {
        protected_tools_enabled: env_flag("HUBU_MCP_TRUST_CLIENT_APPROVAL"),
    };
    run_stdio_with_config(&base_url, config, io::stdin().lock(), io::stdout().lock())
}

pub fn run_stdio(base_url: &str, input: impl BufRead, mut output: impl Write) -> Result<()> {
    run_stdio_with_config(
        base_url,
        McpConfig {
            protected_tools_enabled: false,
        },
        input,
        &mut output,
    )
}

fn run_stdio_with_config(
    base_url: &str,
    config: McpConfig,
    input: impl BufRead,
    mut output: impl Write,
) -> Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = serde_json::from_str(&line)?;
        if let Some(response) = handle_json_rpc(base_url, config, request) {
            writeln!(output, "{}", serde_json::to_string(&response)?)?;
            output.flush()?;
        }
    }
    Ok(())
}

fn env_flag(name: &str) -> bool {
    env::var(name)
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

fn handle_json_rpc(base_url: &str, config: McpConfig, request: Value) -> Option<Value> {
    let id = request.get("id").cloned();
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "serverInfo": {
                "name": "hubu-mcp-server",
                "version": env!("CARGO_PKG_VERSION")
            },
            "capabilities": {
                "tools": {}
            },
            "instructions": HUBU_MCP_INSTRUCTIONS
        })),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => {
            let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
            call_tool(base_url, config, params)
        }
        "notifications/initialized" => return None,
        _ => Err(anyhow!("unsupported MCP method `{method}`")),
    };

    let Some(id) = id else {
        return None;
    };
    Some(match result {
        Ok(result) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result
        }),
        Err(error) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32000,
                "message": error.to_string()
            }
        }),
    })
}

fn tool_definitions() -> Vec<Value> {
    vec![
        read_tool(
            "hubu_health",
            "Check whether the local Hubu server is reachable.",
            json_schema(json!({})),
        ),
        read_tool(
            "hubu_registration_guidance",
            "Read compact agent registration guidance.",
            json_schema(json!({})),
        ),
        read_tool(
            "hubu_client_approval_profile",
            "Read Hubu's generic MCP client approval profile for configuring agent harnesses.",
            json_schema(json!({})),
        ),
        read_tool(
            "hubu_list_users",
            "List registered human users and public user ids.",
            json_schema(json!({})),
        ),
        approval_tool(
            "hubu_register_human",
            "Register or select the active human user. Requires a human click.",
            json_schema(json!({
                "username": { "type": "string" },
                "display_name": { "type": "string" },
                "email": { "type": "string" }
            })),
        ),
        approval_tool(
            "hubu_register_agent",
            "Register an agent for an explicit Hubu user. Requires a human click.",
            json_schema(json!({
                "owner_user_id": { "type": "string" },
                "name": { "type": "string" },
                "version": { "type": "string" }
            })),
        ),
        approval_tool(
            "hubu_add_policy",
            "Attach a spending policy to the active Hubu user. Requires a human click.",
            json_schema(json!({
                "policy_yaml": { "type": "string" },
                "daily_limit_cents": { "type": "integer" }
            })),
        ),
        approval_tool(
            "hubu_create_budget",
            "Create an agent-scoped budget. Requires a human click.",
            json_schema(json!({
                "amount_cents": { "type": "integer" },
                "agent_id": { "type": "string" },
                "starting_at": { "type": "string" },
                "ending_before": { "type": "string" }
            })),
        ),
        approval_tool(
            "hubu_create_recurring_budget",
            "Create a recurring agent-scoped budget series. Requires a human click.",
            json_schema(json!({
                "amount_cents": { "type": "integer" },
                "agent_id": { "type": "string" },
                "recurrence": {
                    "type": "string",
                    "enum": ["daily", "monthly", "yearly"]
                },
                "period_count": { "type": "integer" },
                "starting_at": { "type": "string" }
            })),
        ),
        approval_tool(
            "hubu_revoke_budget",
            "Revoke an active budget. Requires a human click.",
            json_schema(json!({
                "budget_id": { "type": "string" }
            })),
        ),
        approval_tool(
            "hubu_replace_budget",
            "Replace an active budget with a new forward-looking allowance. Requires a human click.",
            json_schema(json!({
                "budget_id": { "type": "string" },
                "amount_cents": { "type": "integer" }
            })),
        ),
        approval_tool(
            "hubu_set_user_cap",
            "Set a global spend cap for the active Hubu user. Requires a human click.",
            json_schema(json!({
                "amount_cents": { "type": "integer" },
                "starting_at": { "type": "string" },
                "ending_before": { "type": "string" }
            })),
        ),
        approval_tool(
            "hubu_revoke_user_cap",
            "Revoke a global spend cap for the active Hubu user. Requires a human click.",
            json_schema(json!({
                "cap_id": { "type": "string" }
            })),
        ),
        read_tool(
            "hubu_show_user_caps",
            "Show global spend caps for the active Hubu user.",
            json_schema(json!({
                "include_all": { "type": "boolean" }
            })),
        ),
        write_tool(
            "hubu_submit_spend",
            "Submit an agent spend request. Human approval is only required when the returned decision is needs_approval.",
            json_schema(json!({
                "agent_id": { "type": "string" },
                "account_id": { "type": "string" },
                "amount_cents": { "type": "integer" },
                "reason": { "type": "string" },
                "merchant": { "type": "string" }
            })),
        ),
        write_tool(
            "hubu_authorize_spend",
            "Authorize an agent spend request and reserve budget without executing payment.",
            json_schema(json!({
                "agent_id": { "type": "string" },
                "account_id": { "type": "string" },
                "amount_cents": { "type": "integer" },
                "reason": { "type": "string" },
                "merchant": { "type": "string" }
            })),
        ),
        read_tool(
            "hubu_list_agents",
            "List registered agents for the active Hubu user.",
            json_schema(json!({})),
        ),
        read_tool(
            "hubu_list_budgets",
            "List active budgets for the active Hubu user.",
            json_schema(json!({
                "include_all": { "type": "boolean" }
            })),
        ),
        read_tool(
            "hubu_list_ledger",
            "List local ledger transactions.",
            json_schema(json!({})),
        ),
    ]
}

fn json_schema(properties: Value) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "additionalProperties": false
    })
}

fn read_tool(name: &str, description: &str, input_schema: Value) -> Value {
    tool(
        name,
        description,
        input_schema,
        true,
        false,
        "none",
        "auto",
        "none",
    )
}

fn write_tool(name: &str, description: &str, input_schema: Value) -> Value {
    tool(
        name,
        description,
        input_schema,
        false,
        false,
        "conditional",
        "auto",
        "hubu_policy_needs_approval",
    )
}

fn approval_tool(name: &str, description: &str, input_schema: Value) -> Value {
    tool(
        name,
        description,
        input_schema,
        false,
        true,
        "required",
        "prompt_before_call",
        "client_human_approval_required",
    )
}

fn tool(
    name: &str,
    description: &str,
    input_schema: Value,
    read_only: bool,
    destructive: bool,
    human_approval: &str,
    client_approval_mode: &str,
    runtime_approval: &str,
) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
        "annotations": {
            "readOnlyHint": read_only,
            "destructiveHint": destructive,
            "idempotentHint": false,
            "openWorldHint": true,
            "x_hubu_human_approval": human_approval,
            "x_hubu_client_approval_mode": client_approval_mode,
            "x_hubu_runtime_approval": runtime_approval
        }
    })
}

fn approval_profile() -> Value {
    let auto_approve_tools = READ_TOOL_NAMES
        .iter()
        .chain(SPEND_TOOL_NAMES.iter())
        .copied()
        .collect::<Vec<_>>();
    json!({
        "protocol_version": HUBU_APPROVAL_PROFILE_VERSION,
        "summary": "Configure agent harnesses to auto-call Hubu read and spend tools, prompt before setup/admin tools, and rely on Hubu policy for needs_approval spend outcomes.",
        "client_policy": {
            "auto_approve_tools": auto_approve_tools,
            "prompt_before_call_tools": APPROVAL_TOOL_NAMES,
            "hubu_policy_conditional_tools": SPEND_TOOL_NAMES
        },
        "response_contract": {
            "needs_approval_field": "requires_human_approval",
            "needs_approval_meaning": "Hubu policy required human review and no payment was executed.",
            "agent_action": "Stop the spend workflow and surface approval_reason plus the structured response to the human."
        },
        "annotation_fields": {
            "client_pre_call": "x_hubu_client_approval_mode",
            "runtime_policy": "x_hubu_runtime_approval",
            "legacy_hubu_field": "x_hubu_human_approval"
        },
        "tools": [
            {
                "names": READ_TOOL_NAMES,
                "x_hubu_client_approval_mode": "auto",
                "x_hubu_runtime_approval": "none"
            },
            {
                "names": SPEND_TOOL_NAMES,
                "x_hubu_client_approval_mode": "auto",
                "x_hubu_runtime_approval": "hubu_policy_needs_approval"
            },
            {
                "names": APPROVAL_TOOL_NAMES,
                "x_hubu_client_approval_mode": "prompt_before_call",
                "x_hubu_runtime_approval": "client_human_approval_required"
            }
        ]
    })
}

fn call_tool(base_url: &str, config: McpConfig, params: Value) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("tools/call missing params.name"))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let response = match name {
        "hubu_health" => get_json(base_url, "/health")?,
        "hubu_registration_guidance" => get_json(base_url, "/registration/guidance")?,
        "hubu_client_approval_profile" => approval_profile(),
        "hubu_list_users" => get_json(base_url, "/users")?,
        "hubu_register_human" => {
            require_trusted_client_approval(config, name)?;
            post_json(base_url, "/init", arguments)?
        }
        "hubu_register_agent" => {
            require_trusted_client_approval(config, name)?;
            post_json(base_url, "/agents/register", arguments)?
        }
        "hubu_add_policy" => {
            require_trusted_client_approval(config, name)?;
            post_json(base_url, "/policies", arguments)?
        }
        "hubu_create_budget" => {
            require_trusted_client_approval(config, name)?;
            post_json(base_url, "/budgets", arguments)?
        }
        "hubu_create_recurring_budget" => {
            require_trusted_client_approval(config, name)?;
            post_json(base_url, "/budgets/series", arguments)?
        }
        "hubu_revoke_budget" => {
            require_trusted_client_approval(config, name)?;
            post_json(base_url, "/budgets/revoke", arguments)?
        }
        "hubu_replace_budget" => {
            require_trusted_client_approval(config, name)?;
            post_json(base_url, "/budgets/replace", arguments)?
        }
        "hubu_set_user_cap" => {
            require_trusted_client_approval(config, name)?;
            post_json(base_url, "/user/cap", arguments)?
        }
        "hubu_revoke_user_cap" => {
            require_trusted_client_approval(config, name)?;
            post_json(base_url, "/user/cap/revoke", arguments)?
        }
        "hubu_show_user_caps" => {
            if arguments
                .get("include_all")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                get_json(base_url, "/user/cap?all=true")?
            } else {
                get_json(base_url, "/user/cap")?
            }
        }
        "hubu_submit_spend" => {
            let response = post_json(base_url, "/spend", arguments)?;
            return Ok(tool_result(spend_response_with_approval_hint(response)));
        }
        "hubu_authorize_spend" => {
            let response = post_json(base_url, "/spend/authorize", arguments)?;
            return Ok(tool_result(spend_response_with_approval_hint(response)));
        }
        "hubu_list_agents" => get_json(base_url, "/agents")?,
        "hubu_list_budgets" => {
            if arguments
                .get("include_all")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                get_json(base_url, "/budgets?all=true")?
            } else {
                get_json(base_url, "/budgets")?
            }
        }
        "hubu_list_ledger" => get_json(base_url, "/ledger")?,
        _ => bail!("unknown Hubu MCP tool `{name}`"),
    };

    Ok(tool_result(response))
}

fn require_trusted_client_approval(config: McpConfig, tool_name: &str) -> Result<()> {
    if config.protected_tools_enabled {
        Ok(())
    } else {
        Err(anyhow!(
            "{tool_name} requires a trusted MCP client approval gate; set HUBU_MCP_TRUST_CLIENT_APPROVAL=1 only when the MCP client prompts a human before invoking destructive tools"
        ))
    }
}

fn spend_response_with_approval_hint(mut response: Value) -> Value {
    let requires_human_approval = response
        .get("decision")
        .and_then(Value::as_str)
        .map(|decision| decision == "needs_approval")
        .unwrap_or(false);
    if let Some(object) = response.as_object_mut() {
        object.insert(
            "requires_human_approval".to_string(),
            Value::Bool(requires_human_approval),
        );
        if requires_human_approval {
            object.insert(
                "approval_reason".to_string(),
                Value::String(
                    "policy returned needs_approval; Hubu did not execute payment".to_string(),
                ),
            );
        }
    }
    response
}

fn tool_result(value: Value) -> Value {
    json!({
        "content": [
            {
                "type": "text",
                "text": serde_json::to_string_pretty(&value)
                    .expect("tool response should serialize")
            }
        ],
        "structuredContent": value
    })
}

fn get_json(base_url: &str, path: &str) -> Result<Value> {
    request_json(base_url, "GET", path, None)
}

fn post_json(base_url: &str, path: &str, body: Value) -> Result<Value> {
    request_json(base_url, "POST", path, Some(body))
}

fn request_json(base_url: &str, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
    let (host, port) = parse_base_url(base_url)?;
    let body_text = body.map(|body| body.to_string()).unwrap_or_default();
    let authorization_header = auth_token()?
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let mut stream = TcpStream::connect((host.as_str(), port))
        .with_context(|| format!("connect to Hubu server at {base_url}"))?;

    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\n{authorization_header}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body_text.len(),
        body_text
    )?;
    stream.shutdown(Shutdown::Write)?;

    let mut raw = String::new();
    stream.read_to_string(&mut raw)?;
    let (status, response_body) = parse_http_response(&raw)?;
    let json: Value = serde_json::from_str(response_body)
        .with_context(|| format!("parse server response body `{response_body}`"))?;

    if !(200..300).contains(&status) {
        let message = json
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("request failed");
        bail!("Hubu server returned HTTP {status}: {message}");
    }

    Ok(json)
}

fn parse_base_url(base_url: &str) -> Result<(String, u16)> {
    let trimmed = base_url
        .strip_prefix("http://")
        .ok_or_else(|| anyhow!("only http:// Hubu URLs are supported"))?;
    let host_port = trimmed.trim_end_matches('/');
    let (host, port) = host_port
        .rsplit_once(':')
        .ok_or_else(|| anyhow!("Hubu URL must include a port"))?;
    Ok((host.to_string(), port.parse()?))
}

fn auth_token() -> Result<Option<String>> {
    if let Ok(token) = env::var(AUTH_TOKEN_ENV) {
        let token = token.trim().to_string();
        if token.is_empty() {
            return Err(anyhow!("{AUTH_TOKEN_ENV} cannot be empty"));
        }
        return Ok(Some(token));
    }

    let path =
        env::var(AUTH_TOKEN_FILE_ENV).unwrap_or_else(|_| DEFAULT_AUTH_TOKEN_FILE.to_string());
    match fs::read_to_string(&path) {
        Ok(contents) => {
            let token = contents.trim().to_string();
            if token.is_empty() {
                Err(anyhow!("Hubu auth token file `{path}` is empty"))
            } else {
                Ok(Some(token))
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read Hubu auth token file `{path}`")),
    }
}

fn parse_http_response(raw: &str) -> Result<(u16, &str)> {
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .ok_or_else(|| anyhow!("invalid HTTP response"))?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| anyhow!("invalid HTTP status line"))?
        .parse()?;
    Ok((status, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_tools_are_marked_for_human_approval() {
        let tools = tool_definitions();
        let protected = tools
            .iter()
            .find(|tool| tool["name"] == "hubu_create_budget")
            .expect("budget tool should exist");

        assert_eq!(
            protected["annotations"]["x_hubu_human_approval"],
            "required"
        );
        assert_eq!(protected["annotations"]["destructiveHint"], true);
    }

    #[test]
    fn read_tools_are_marked_read_only() {
        let tools = tool_definitions();
        let guidance = tools
            .iter()
            .find(|tool| tool["name"] == "hubu_registration_guidance")
            .expect("guidance tool should exist");

        assert_eq!(guidance["annotations"]["readOnlyHint"], true);
        assert_eq!(guidance["annotations"]["x_hubu_human_approval"], "none");
        assert_eq!(
            guidance["annotations"]["x_hubu_client_approval_mode"],
            "auto"
        );
    }

    #[test]
    fn spend_needs_approval_adds_human_approval_hint() {
        let response = spend_response_with_approval_hint(json!({
            "decision": "needs_approval",
            "payment": null
        }));

        assert_eq!(response["requires_human_approval"], true);
        assert!(response["approval_reason"].is_string());
    }

    #[test]
    fn authorize_spend_tool_is_agent_callable() {
        let tools = tool_definitions();
        let tool = tools
            .iter()
            .find(|tool| tool["name"] == "hubu_authorize_spend")
            .expect("authorize spend tool should exist");

        assert_eq!(tool["annotations"]["x_hubu_human_approval"], "conditional");
        assert_eq!(tool["annotations"]["x_hubu_client_approval_mode"], "auto");
        assert_eq!(
            tool["annotations"]["x_hubu_runtime_approval"],
            "hubu_policy_needs_approval"
        );
        assert_eq!(tool["annotations"]["destructiveHint"], false);
        assert!(tool["inputSchema"]["properties"]["amount_cents"].is_object());
    }

    #[test]
    fn protected_tool_requires_client_prompt_annotation() {
        let tools = tool_definitions();
        let protected = tools
            .iter()
            .find(|tool| tool["name"] == "hubu_register_agent")
            .expect("agent registration tool should exist");

        assert_eq!(
            protected["annotations"]["x_hubu_client_approval_mode"],
            "prompt_before_call"
        );
        assert_eq!(
            protected["annotations"]["x_hubu_runtime_approval"],
            "client_human_approval_required"
        );
    }

    #[test]
    fn approval_profile_lists_spend_tools_as_auto_with_hubu_policy_runtime() {
        let profile = approval_profile();

        assert_eq!(profile["protocol_version"], HUBU_APPROVAL_PROFILE_VERSION);
        assert!(profile["client_policy"]["auto_approve_tools"]
            .as_array()
            .expect("auto tools should be an array")
            .iter()
            .any(|tool| tool == "hubu_submit_spend"));
        assert!(profile["client_policy"]["prompt_before_call_tools"]
            .as_array()
            .expect("prompt tools should be an array")
            .iter()
            .any(|tool| tool == "hubu_create_budget"));
        assert_eq!(
            profile["response_contract"]["needs_approval_field"],
            "requires_human_approval"
        );
    }

    #[test]
    fn initialize_includes_client_approval_instructions() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize"
        });

        let response = handle_json_rpc(
            DEFAULT_BASE_URL,
            McpConfig {
                protected_tools_enabled: false,
            },
            request,
        )
        .expect("initialize should return a response");

        let instructions = response["result"]["instructions"]
            .as_str()
            .expect("instructions should be present");
        assert!(instructions.contains("hubu_submit_spend"));
        assert!(instructions.contains("Protected setup/admin tools require"));
    }

    #[test]
    fn protected_tool_schema_does_not_accept_agent_controlled_approval() {
        let tools = tool_definitions();
        let protected = tools
            .iter()
            .find(|tool| tool["name"] == "hubu_create_budget")
            .expect("budget tool should exist");

        assert!(protected["inputSchema"]["properties"]
            .get("human_approved")
            .is_none());
    }

    #[test]
    fn protected_tool_rejects_without_trusted_client_gate() {
        let error = require_trusted_client_approval(
            McpConfig {
                protected_tools_enabled: false,
            },
            "hubu_create_budget",
        )
        .expect_err("protected tool should require trusted client gate");

        assert!(error.to_string().contains("trusted MCP client approval"));
    }
}
