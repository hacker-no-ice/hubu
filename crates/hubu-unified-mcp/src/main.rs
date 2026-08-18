use std::io;

fn main() {
    if std::env::args()
        .nth(1)
        .is_some_and(|argument| matches!(argument.as_str(), "version" | "--version" | "-V"))
    {
        println!("{}", hubu_unified_mcp::product_version());
        return;
    }

    if let Err(error) =
        hubu_unified_mcp::run_stdio_from_env(io::stdin().lock(), io::stdout().lock())
    {
        eprintln!("hubu-unified-mcp: {error}");
        std::process::exit(1);
    }
}
