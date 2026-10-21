//! `hubu artifact save`: the CLI form of the `hubu_save_artifact` MCP tool.
use crate::{gallery, CliContext};
use anyhow::{bail, Context, Result};

const USAGE: &str = "Usage: hubu artifact save [--stack-profile PATH] --operation-handle HANDLE --folder NAME [--artifact-id ID] [--name FILE_STEM]

Saves one artifact of a succeeded governed operation into an allowlisted save
folder (see `hubu folders`), with a FILE.receipt.json sidecar carrying the
artifact ID, SHA-256 digest and settled cost. --artifact-id may be omitted when
the operation produced exactly one artifact. Never overwrites a different file;
does not start or resume provider work.";

pub(crate) fn command(context: &CliContext, mut args: Vec<String>) -> Result<()> {
    if args.is_empty() || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        return Ok(());
    }
    if args[0] != "save" {
        bail!("unknown artifact command `{}`\n\n{USAGE}", args[0]);
    }
    if context.has_explicit_base_url() {
        bail!("artifact save uses a stack profile; use hubu-unified-mcp artifact save for manual backend configuration");
    }
    let handoff = gallery::profile_handoff(context, &mut args, "artifact save")?;
    let mut native_args = vec!["artifact".to_owned()];
    native_args.extend(args);
    let status = gallery::native_command(&handoff, &context.hubu_home, &native_args)?
        .status()
        .context("could not start native artifact saver")?;
    if !status.success() {
        bail!("artifact was not saved ({status})");
    }
    Ok(())
}
