use std::io::{self, BufReader};

fn main() {
    if std::env::args()
        .nth(1)
        .is_some_and(|argument| matches!(argument.as_str(), "version" | "--version" | "-V"))
    {
        println!(
            "{}",
            serde_json::json!({
                "product": "hubu-unified-mcp",
                "product_version": hubu_unified_mcp::product_version(),
                "source_commit": hubu_unified_mcp::source_commit(),
                "executor_contract": hubu_unified_mcp::EXECUTOR_CONTRACT_VERSION,
                "unified_mcp_contract": hubu_unified_mcp::UNIFIED_CONTRACT_VERSION,
            })
        );
        return;
    }

    if let Err(error) =
        hubu_unified_mcp::run_stdio_from_env(BufReader::new(io::stdin()), io::stdout().lock())
    {
        eprintln!("hubu-unified-mcp: {error}");
        std::process::exit(1);
    }
}
