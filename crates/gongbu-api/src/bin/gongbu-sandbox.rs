use gongbu_api::sandbox::{BoundaryMode, SandboxConfig, SandboxRun, SandboxWiring};
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
    let mut hubu_mode = None;
    let mut provider_mode = None;
    let mut maximum_spend_minor = None;
    let mut live_spend_acknowledgement = None;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--config" => config_path = args.next().map(PathBuf::from),
            "--preserve" => preserve = args.next().map(PathBuf::from),
            "--hubu-mode" => hubu_mode = args.next(),
            "--provider-mode" => provider_mode = args.next(),
            "--max-spend-minor" => maximum_spend_minor = args.next(),
            "--live-spend-ack" => live_spend_acknowledgement = args.next(),
            "--help" | "-h" => {
                println!(
                    "Usage: gongbu-sandbox --config <profile.json> \
                     [--hubu-mode <mock|real>] [--provider-mode <mock|real>] \
                     [--max-spend-minor <integer>] [--live-spend-ack <value>] \
                     [--preserve <diagnostics-dir>]"
                );
                return Ok(());
            }
            _ => return Err(format!("unknown argument: {argument}").into()),
        }
    }
    let config_path = config_path.ok_or("--config is required")?;
    let mut config = SandboxConfig::load(config_path)?;
    config.apply_environment_overrides()?;
    if let Some(mode) = hubu_mode {
        config.hubu.mode = mode.parse::<BoundaryMode>()?;
    }
    if let Some(mode) = provider_mode {
        config.provider.mode = mode.parse::<BoundaryMode>()?;
    }
    if let Some(value) = maximum_spend_minor {
        config.provider.maximum_spend_minor = Some(
            value
                .parse()
                .map_err(|_| "--max-spend-minor must be an integer")?,
        );
    }
    if let Some(value) = live_spend_acknowledgement {
        config.provider.live_spend_acknowledgement = Some(value);
    }
    config.validate()?;
    let _wiring = SandboxWiring::from_config(&config)?;
    let run = SandboxRun::start(&config)?;
    println!("{}", serde_json::to_string_pretty(run.manifest())?);
    if let Some(destination) = preserve {
        let destination = run.preserve(destination)?;
        eprintln!("diagnostics preserved at {}", destination.display());
    }
    Ok(())
}
