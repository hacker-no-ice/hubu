use gongbu_mcp::{tool_definitions, Config, GongbuClient};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

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
                json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"gongbu-mcp","version":env!("CARGO_PKG_VERSION")}}})
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
