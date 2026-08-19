use gongbu_mcp::{tool_definitions, Config, GongbuClient};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

const UNSUPPORTED_NOTICE: &str = "WARNING: gongbu-mcp is deprecated and unsupported. Migrate to the only supported agent-facing surface, hubu-unified-mcp: run `hubu init codex --migrate-standalone --gongbu-endpoint URL --gongbu-token-file FILE` or see docs/unified-mcp-migration.md. Standalone source remains only for HUB-98 removal staging.";

#[derive(Deserialize)]
struct Request {
    jsonrpc: Option<String>,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolCall {
    name: String,
    #[serde(default = "empty_object")]
    arguments: Value,
    #[serde(default, rename = "_meta")]
    meta: Option<Value>,
}

fn empty_object() -> Value {
    json!({})
}

fn main() {
    eprintln!("{UNSUPPORTED_NOTICE}");

    if std::env::args()
        .nth(1)
        .is_some_and(|argument| matches!(argument.as_str(), "help" | "--help" | "-h"))
    {
        println!(
            "gongbu-mcp (unsupported)\n\nUse hubu-unified-mcp instead.\nMigration: hubu init codex --migrate-standalone --gongbu-endpoint URL --gongbu-token-file FILE\nGuide: docs/unified-mcp-migration.md"
        );
        return;
    }

    if std::env::args()
        .nth(1)
        .is_some_and(|argument| matches!(argument.as_str(), "version" | "--version" | "-V"))
    {
        println!(
            "{}",
            serde_json::to_string_pretty(&gongbu_build_info::build_info())
                .expect("build metadata should serialize")
        );
        return;
    }

    if let Err(error) = run() {
        eprintln!("gongbu-mcp: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let client = GongbuClient::new(Config::from_env()?)?;
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        let request: Request = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(_) => {
                write_message(
                    &mut stdout,
                    json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}}),
                )?;
                continue;
            }
        };
        let Some(id) = request.id else { continue };
        if request.jsonrpc.as_deref() != Some("2.0") {
            write_message(
                &mut stdout,
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32600,"message":"Invalid Request"}}),
            )?;
            continue;
        }
        let response = match request.method.as_str() {
            "initialize" => {
                json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":gongbu_build_info::MCP_PROTOCOL_VERSION,"capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"gongbu-mcp","version":gongbu_build_info::build_info().product_version}}})
            }
            "ping" => json!({"jsonrpc":"2.0","id":id,"result":{}}),
            "tools/list" => {
                json!({"jsonrpc":"2.0","id":id,"result":{"tools":tool_definitions()}})
            }
            "tools/call" => match serde_json::from_value::<ToolCall>(request.params) {
                Ok(call) => {
                    let _ = call.meta;
                    json!({"jsonrpc":"2.0","id":id,"result":client.call_tool(&call.name, call.arguments)})
                }
                Err(_) => {
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"Invalid params"}})
                }
            },
            _ => {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not found"}})
            }
        };
        write_message(&mut stdout, response)?;
    }
    Ok(())
}

fn write_message(output: &mut impl Write, message: Value) -> io::Result<()> {
    serde_json::to_writer(&mut *output, &message)?;
    output.write_all(b"\n")?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::UNSUPPORTED_NOTICE;

    #[test]
    fn unsupported_notice_is_actionable_and_contains_no_configuration_values() {
        assert!(UNSUPPORTED_NOTICE.contains("deprecated and unsupported"));
        assert!(UNSUPPORTED_NOTICE.contains("hubu-unified-mcp"));
        assert!(UNSUPPORTED_NOTICE.contains("--migrate-standalone"));
        assert!(!UNSUPPORTED_NOTICE.contains("GONGBU_"));
        assert!(!UNSUPPORTED_NOTICE.contains("bearer"));
    }
}
