use std::io::Read;

use anyhow::{bail, Context, Result};

const HELP: &str = "Send feedback (local preparation only)

  hubu feedback                 Print destinations and structured guidance
  hubu feedback prepare         Read a JSON report from stdin and print its preview

Required JSON fields: trying (What were you trying to do?), happened (What happened?).
Optional: kind (bug, idea, private), diagnostics (operation_handle, error_code).
Use private for billing/sensitive reports. Never paste prompts, credentials or logs.
Review the exact destination and content before authorizing an external submission.
Manual route: https://github.com/hacker-no-ice/hubu/blob/main/docs/feedback.md";

pub(super) fn command(args: Vec<String>) -> Result<()> {
    let output = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] | ["guidance"] => hubu_feedback::guidance(),
        ["help" | "--help" | "-h"] => {
            println!("{HELP}");
            return Ok(());
        }
        ["prepare"] => {
            let mut bytes = Vec::new();
            std::io::stdin()
                .lock()
                .take(32_769)
                .read_to_end(&mut bytes)
                .context("Could not read feedback JSON; use the manual feedback route")?;
            if bytes.len() > 32_768 {
                bail!("Feedback input exceeds 32 KiB; use compact descriptions, without logs.");
            }
            let input = serde_json::from_slice(&bytes).map_err(|_| {
                anyhow::anyhow!("Invalid feedback JSON; run hubu feedback for the schema.")
            })?;
            hubu_feedback::prepare(
                input,
                hubu_common::build::build_info().product_version,
                &format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
            )
            .map_err(anyhow::Error::msg)?
        }
        _ => bail!("{HELP}"),
    };
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}
