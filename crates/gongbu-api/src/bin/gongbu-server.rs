use gongbu_api::server;
use std::path::PathBuf;

const HELP: &str = "gongbu-server\n\nUSAGE:\n    gongbu-server serve --config /absolute/path/gongbu.json\n    gongbu-server --version\n\nThe persistent local Gongbu service. Hubu must be started independently.";

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
        _ => Err(std::io::Error::other(HELP).into()),
    }
}
