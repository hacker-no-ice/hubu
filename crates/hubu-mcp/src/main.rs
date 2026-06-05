fn main() {
    if let Err(error) = hubu_mcp::run_stdio_from_env() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
