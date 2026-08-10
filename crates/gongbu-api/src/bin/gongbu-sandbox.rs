use gongbu_api::sandbox::{SandboxConfig, SandboxRun, SandboxWiring};
use std::{env, path::PathBuf, process::ExitCode};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("gongbu sandbox failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let mut config_path = None;
    let mut preserve = None;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--config" => config_path = args.next().map(PathBuf::from),
            "--preserve" => preserve = args.next().map(PathBuf::from),
            "--help" | "-h" => {
                println!(
                    "Usage: gongbu-sandbox --config <profile.json> [--preserve <diagnostics-dir>]"
                );
                return Ok(());
            }
            _ => return Err(format!("unknown argument: {argument}").into()),
        }
    }
    let config_path = config_path.ok_or("--config is required")?;
    let config = SandboxConfig::from_path(config_path)?;
    let _wiring = SandboxWiring::from_config(&config)?;
    let run = SandboxRun::start(&config)?;
    println!("{}", serde_json::to_string_pretty(run.manifest())?);
    if let Some(destination) = preserve {
        let destination = run.preserve(destination)?;
        eprintln!("diagnostics preserved at {}", destination.display());
    }
    Ok(())
}
