use gongbu_api::server;
use std::path::PathBuf;

const HELP: &str = "gongbu-server\n\nUSAGE:\n    gongbu-server serve --config /absolute/path/gongbu.json\n    gongbu-server validate-config --config /absolute/path/gongbu.json\n    gongbu-server credentials bootstrap-managed --config CONFIG --hubu-token-file FILE --caller-token-file FILE --secret-dir DIR\n    gongbu-server --version\n\nThe persistent local Gongbu service. Hubu must be started independently. The bootstrap-managed command is an internal interface for launcher-owned local stacks.";

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!(
            "gongbu-server: server stopped ({})",
            error_category(error.as_ref())
        );
        std::process::exit(1);
    }
}

fn error_category(error: &(dyn std::error::Error + 'static)) -> &'static str {
    if error
        .downcast_ref::<gongbu_api::server::ServerError>()
        .is_some()
    {
        "configuration"
    } else if error.downcast_ref::<std::io::Error>().is_some() {
        "io"
    } else {
        "dependency"
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [flag] if matches!(flag.as_str(), "--version" | "-V") => {
            println!(
                "{}",
                serde_json::to_string(&gongbu_build_info::build_info())?
            );
            Ok(())
        }
        [] => {
            println!("{HELP}");
            Ok(())
        }
        [flag] if matches!(flag.as_str(), "--help" | "-h") => {
            println!("{HELP}");
            Ok(())
        }
        [command, config_flag, path] if command == "serve" && config_flag == "--config" => {
            server::serve(PathBuf::from(path)).await
        }
        [command, config_flag, path]
            if command == "validate-config" && config_flag == "--config" =>
        {
            let config = server::validate_runtime_inputs(PathBuf::from(path))?;
            println!(
                "{}",
                serde_json::json!({
                    "status": "valid",
                    "schema_version": config.schema_version,
                    "provider_mode": config.providers.mode,
                })
            );
            Ok(())
        }
        [command, action, rest @ ..]
            if command == "credentials" && action == "bootstrap-managed" =>
        {
            bootstrap_managed(rest)?;
            Ok(())
        }
        _ => Err(std::io::Error::other(HELP).into()),
    }
}

fn bootstrap_managed(args: &[String]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    fn value(args: &[String], flag: &str) -> Result<PathBuf, std::io::Error> {
        let matches = args
            .windows(2)
            .filter(|pair| pair[0] == flag)
            .map(|pair| PathBuf::from(&pair[1]))
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(std::io::Error::other(HELP));
        }
        Ok(matches[0].clone())
    }
    if args.len() != 8 {
        return Err(std::io::Error::other(HELP).into());
    }
    let config = value(args, "--config")?;
    let hubu = value(args, "--hubu-token-file")?;
    let caller = value(args, "--caller-token-file")?;
    let directory = value(args, "--secret-dir")?;
    let result = gongbu_api::config::setup::bootstrap_managed(&config, &hubu, &caller, &directory)?;
    println!("{result}");
    Ok(())
}
