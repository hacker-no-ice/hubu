fn main() {
    if let Err(error) = hubu_api::run_server_from_env() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
