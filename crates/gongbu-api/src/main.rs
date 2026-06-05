fn main() {
    if let Err(error) = gongbu_api::run_server_from_env() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
