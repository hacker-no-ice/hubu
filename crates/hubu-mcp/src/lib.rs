use std::{
    env,
    io::{self, BufRead, Read, Write},
    net::{Shutdown, TcpStream},
};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8787";
const PROTOCOL_VERSION: &str = "2024-11-05";

pub fn run_stdio_from_env() -> Result<()> {
    let base_url = env::var("HUBU_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
    run_stdio(&base_url, io::stdin().lock(), io::stdout().lock())
}

pub fn run_stdio(base_url: &str, input: impl BufRead, mut output: impl Write) -> Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = serde_json::from_str(&line)?;
        if let Some(response) = handle_json_rpc(base_url, request) {
            writeln!(output, "{}", serde_json::to_string(&response)?)?;
            output.flush()?;
        }
    }
    Ok(())
}

fn handle_json_rpc(base_url: &str, request: Value) -> Option<Value> {
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
            }
        })),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => {
            let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
            call_tool(base_url, params)
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
        approval_tool(
            "hubu_register_human",
            "Register or select the active human user. Requires a human click.",
            json_schema(json!({
                "display_name": { "type": "string" },
                "email": { "type": "string" },
                "human_approved": { "type": "boolean" }
            })),
        ),
        approval_tool(
            "hubu_register_agent",
            "Register an agent for the active Hubu user. Requires a human click.",
            json_schema(json!({
                "name": { "type": "string" },
                "version": { "type": "string" },
                "human_approved": { "type": "boolean" }
            })),
        ),
        approval_tool(
            "hubu_add_policy",
            "Attach a spending policy to an agent. Requires a human click.",
            json_schema(json!({
                "agent_id": { "type": "string" },
                "policy_yaml": { "type": "string" },
                "daily_limit_cents": { "type": "integer" },
                "human_approved": { "type": "boolean" }
            })),
        ),
        approval_tool(
            "hubu_create_budget",
            "Create a human-scoped budget. Requires a human click.",
            json_schema(json!({
                "amount_cents": { "type": "integer" },
                "starting_at": { "type": "string" },
                "ending_before": { "type": "string" },
                "human_approved": { "type": "boolean" }
            })),
        ),
        approval_tool(
            "hubu_create_recurring_budget",
            "Create a recurring human-scoped budget series. Requires a human click.",
            json_schema(json!({
                "amount_cents": { "type": "integer" },
                "recurrence": {
                    "type": "string",
                    "enum": ["daily", "monthly", "yearly"]
                },
                "period_count": { "type": "integer" },
                "starting_at": { "type": "string" },
                "human_approved": { "type": "boolean" }
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
        read_tool(
            "hubu_list_agents",
            "List registered agents for the active Hubu user.",
            json_schema(json!({})),
        ),
        read_tool(
            "hubu_list_budgets",
            "List budgets for the active Hubu user.",
            json_schema(json!({})),
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
    tool(name, description, input_schema, true, false, "none")
}

fn write_tool(name: &str, description: &str, input_schema: Value) -> Value {
    tool(name, description, input_schema, false, false, "conditional")
}

fn approval_tool(name: &str, description: &str, input_schema: Value) -> Value {
    tool(name, description, input_schema, false, true, "required")
}

fn tool(
    name: &str,
    description: &str,
    input_schema: Value,
    read_only: bool,
    destructive: bool,
    human_approval: &str,
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
            "x_hubu_human_approval": human_approval
        }
    })
}

fn call_tool(base_url: &str, params: Value) -> Result<Value> {
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
        "hubu_register_human" => {
            require_human_approval(&arguments, name)?;
            post_json(base_url, "/init", strip_human_approved(arguments))?
        }
        "hubu_register_agent" => {
            require_human_approval(&arguments, name)?;
            post_json(
                base_url,
                "/agents/register",
                strip_human_approved(arguments),
            )?
        }
        "hubu_add_policy" => {
            require_human_approval(&arguments, name)?;
            post_json(base_url, "/policies", strip_human_approved(arguments))?
        }
        "hubu_create_budget" => {
            require_human_approval(&arguments, name)?;
            post_json(base_url, "/budgets", strip_human_approved(arguments))?
        }
        "hubu_create_recurring_budget" => {
            require_human_approval(&arguments, name)?;
            post_json(base_url, "/budgets/series", strip_human_approved(arguments))?
        }
        "hubu_submit_spend" => {
            let response = post_json(base_url, "/spend", arguments)?;
            return Ok(tool_result(spend_response_with_approval_hint(response)));
        }
        "hubu_list_agents" => get_json(base_url, "/agents")?,
        "hubu_list_budgets" => get_json(base_url, "/budgets")?,
        "hubu_list_ledger" => get_json(base_url, "/ledger")?,
        _ => bail!("unknown Hubu MCP tool `{name}`"),
    };

    Ok(tool_result(response))
}

fn require_human_approval(arguments: &Value, tool_name: &str) -> Result<()> {
    if arguments
        .get("human_approved")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        Ok(())
    } else {
        Err(anyhow!(
            "{tool_name} requires human approval; retry after the human approves this MCP tool call"
        ))
    }
}

fn strip_human_approved(mut arguments: Value) -> Value {
    if let Some(object) = arguments.as_object_mut() {
        object.remove("human_approved");
    }
    arguments
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
    let mut stream = TcpStream::connect((host.as_str(), port))
        .with_context(|| format!("connect to Hubu server at {base_url}"))?;

    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
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
    fn approval_flag_is_removed_before_forwarding() {
        let stripped = strip_human_approved(json!({
            "agent_id": "agt_123",
            "human_approved": true
        }));

        assert!(stripped.get("human_approved").is_none());
        assert_eq!(stripped["agent_id"], "agt_123");
    }
}
