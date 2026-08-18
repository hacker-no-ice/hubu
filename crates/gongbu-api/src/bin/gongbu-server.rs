use gongbu_api::server;
use std::path::PathBuf;

const HELP: &str = "gongbu-server\n\nUSAGE:\n    gongbu-server serve --config /absolute/path/gongbu.json\n    gongbu-server credentials bootstrap --config /absolute/path/gongbu.json [--hubu-token-file FILE]\n    gongbu-server credentials rotate <caller|hubu> --config /absolute/path/gongbu.json [--hubu-token-file FILE]\n    gongbu-server credentials rollback <caller|hubu> --config /absolute/path/gongbu.json\n    gongbu-server credentials revoke-rollback <caller|hubu> --config /absolute/path/gongbu.json\n    gongbu-server --version\n\nCredential classes:\n    caller  caller-to-Gongbu capability generated and stored in Keychain\n    hubu    Hubu executor/service credential discovered, protected-endpoint verified, and stored in Keychain\n\nSpend-auth token IDs are request identifiers, not credentials. The human reconciliation capability belongs only in Hubu operator clients and is never accepted by Gongbu. Provider credentials use their own Keychain references. Credential changes require a Gongbu restart.";

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        if let Some(gongbu_api::server::ServerError::Credential(message)) =
            error.downcast_ref::<gongbu_api::server::ServerError>()
        {
            eprintln!("gongbu-server: server stopped (credential): {message}");
        } else {
            eprintln!(
                "gongbu-server: server stopped ({})",
                error_category(error.as_ref())
            );
        }
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
        [command, action, rest @ ..] if command == "credentials" => {
            credentials(action, rest).map_err(Into::into)
        }
        _ => Err(std::io::Error::other(HELP).into()),
    }
}

fn credentials(action: &str, args: &[String]) -> Result<(), gongbu_api::server::ServerError> {
    use gongbu_api::config::setup::{self, CredentialClass};

    let value = |flag: &str| -> Option<PathBuf> {
        args.windows(2)
            .find(|pair| pair[0] == flag)
            .map(|pair| PathBuf::from(&pair[1]))
    };
    let config = value("--config").ok_or_else(|| {
        gongbu_api::server::ServerError::Invalid("credentials command requires --config".into())
    })?;
    let token_file = value("--hubu-token-file");
    let message = match action {
        "bootstrap" => setup::bootstrap(&config, token_file.as_deref())?,
        "rotate" | "rollback" | "revoke-rollback" => {
            let class = args.first().ok_or_else(|| {
                gongbu_api::server::ServerError::Invalid(
                    "credentials command requires class `caller` or `hubu`".into(),
                )
            })?;
            let class = CredentialClass::parse(class)?;
            match action {
                "rotate" => setup::rotate(&config, class, token_file.as_deref())?,
                "rollback" => setup::rollback(&config, class)?,
                _ => setup::revoke_rollback(&config, class)?,
            }
        }
        _ => {
            return Err(gongbu_api::server::ServerError::Invalid(
                "credential action must be bootstrap, rotate, rollback, or revoke-rollback".into(),
            ))
        }
    };
    println!("{message}");
    Ok(())
}
