use std::path::PathBuf;

const HELP: &str = "hubu-server\n\nUSAGE:\n    hubu-server\n    hubu-server serve --config /absolute/path/hubu-launch.json\n    hubu-server validate-config --config /absolute/path/hubu-launch.json\n    hubu-server --version";

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.as_slice() {
        [argument] if matches!(argument.as_str(), "version" | "--version" | "-V") => {
            println!(
                "{}",
                serde_json::to_string_pretty(&hubu_common::build::build_info())?
            );
            Ok(())
        }
        [argument] if matches!(argument.as_str(), "help" | "--help" | "-h") => {
            println!("{HELP}");
            Ok(())
        }
        [command, flag, path] if command == "validate-config" && flag == "--config" => {
            hubu_api::launch_config::HubuLaunchConfig::from_path(PathBuf::from(path))?;
            println!(
                "{}",
                serde_json::json!({"status": "valid", "schema_version": 1})
            );
            Ok(())
        }
        [command, flag, path] if command == "serve" && flag == "--config" => {
            let config = hubu_api::launch_config::HubuLaunchConfig::from_path(PathBuf::from(path))?;
            for name in [
                "HUBU_AUTH_TOKEN",
                "HUBU_APPROVAL_TOKEN",
                "HUBU_RECONCILIATION_TOKEN",
                "HUBU_LOG_FILE",
                "HUBU_LOG_STDERR",
                "HUBU_SPEND_TIMING_CONFIG",
            ] {
                std::env::remove_var(name);
            }
            std::env::set_var("HUBU_DB_PATH", &config.database_path);
            std::env::set_var("HUBU_AUTH_TOKEN_FILE", &config.auth_token_file);
            std::env::set_var("HUBU_APPROVAL_TOKEN_FILE", &config.approval_token_file);
            std::env::set_var(
                "HUBU_RECONCILIATION_TOKEN_FILE",
                &config.reconciliation_token_file,
            );
            if let Some(path) = &config.log_file {
                std::env::set_var("HUBU_LOG_FILE", path);
            }
            if let Some(path) = &config.spend_timing_config {
                std::env::set_var("HUBU_SPEND_TIMING_CONFIG", path);
            }
            hubu_api::run_server(&config.listen.to_string())
        }
        [bind_addr] => hubu_api::run_server(bind_addr),
        [] => hubu_api::run_server_from_env(),
        _ => anyhow::bail!(HELP),
    }
}
