fn main() {
    if std::env::args()
        .nth(1)
        .is_some_and(|argument| matches!(argument.as_str(), "version" | "--version" | "-V"))
    {
        println!(
            "{}",
            serde_json::to_string_pretty(&hubu_common::build::build_info())
                .expect("build metadata should serialize")
        );
        return;
    }

    if let Err(error) = hubu_mcp::run_stdio_from_env() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
