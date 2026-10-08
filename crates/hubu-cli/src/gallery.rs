//! Local export delegates to the native adapter, using the selected profile.
use crate::{stack, CliContext};
use anyhow::{bail, Context, Result};
use std::{path::PathBuf, process::Command};

pub(crate) fn command(context: &CliContext, mut args: Vec<String>) -> Result<()> {
    if args.is_empty() || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("Usage: hubu gallery export [--stack-profile PATH] --operation-handle HANDLE --output ABSOLUTE_DIR --size 2k --tier draft\n\nUses the selected stack profile. Downloads completed artifacts and verifies settled cost; does not start or resume provider work. For manual backend configuration use hubu-unified-mcp gallery export.");
        return Ok(());
    }
    if context.has_explicit_base_url() {
        bail!("gallery export uses a stack profile; use hubu-unified-mcp gallery export for manual backend configuration");
    }
    let mut profile = None;
    if let Some(index) = args.iter().position(|arg| arg == "--stack-profile") {
        args.remove(index);
        if index >= args.len() {
            bail!("missing value for --stack-profile");
        }
        profile = Some(PathBuf::from(args.remove(index)));
    }
    let handoff = match profile {
        Some(profile) => stack::codex_handoff(&profile, &context.hubu_home)?,
        None => stack::active_client_handoff(&context.hubu_home)?
            .context("gallery export requires an initialized stack profile")?,
    };
    let status = export_command(&handoff, &args)?
        .status()
        .context("could not start native gallery exporter")?;
    if !status.success() {
        bail!("native gallery exporter failed ({status})");
    }
    Ok(())
}

fn export_command(handoff: &stack::CodexHandoff, args: &[String]) -> Result<Command> {
    let gongbu_endpoint = handoff
        .gongbu_endpoint
        .as_ref()
        .context("gallery export requires a local-stack profile with Gongbu")?;
    let gongbu_token = handoff
        .gongbu_token_file
        .as_ref()
        .context("gallery export requires the profile's Gongbu credential")?;
    let mut command = Command::new(&handoff.mcp_server);
    command.arg("gallery").args(args);
    // Profile-bound reads must not inherit credentials or mutation authority from
    // another shell session. The native exporter never starts the MCP worker.
    for name in [
        "HUBU_UNIFIED_HUBU_BEARER_TOKEN",
        "HUBU_UNIFIED_GONGBU_BEARER_TOKEN",
        "HUBU_APPROVAL_TOKEN",
        "HUBU_APPROVAL_TOKEN_FILE",
        "HUBU_RECONCILIATION_TOKEN",
        "HUBU_RECONCILIATION_TOKEN_FILE",
        "HUBU_MCP_TRUST_CLIENT_APPROVAL",
        "HUBU_MCP_TRUST_SPEND_APPROVAL",
        "HUBU_UNIFIED_OPERATION_KEY_DB",
    ] {
        command.env_remove(name);
    }
    command
        .env("HUBU_UNIFIED_HUBU_ENDPOINT", &handoff.hubu_endpoint)
        .env(
            "HUBU_UNIFIED_HUBU_BEARER_TOKEN_FILE",
            &handoff.hubu_token_file,
        )
        .env("HUBU_UNIFIED_GONGBU_ENDPOINT", gongbu_endpoint)
        .env("HUBU_UNIFIED_GONGBU_BEARER_TOKEN_FILE", gongbu_token)
        .env(
            "HUBU_UNIFIED_OPERATION_STATE_PATH",
            &handoff.operation_state_path,
        );
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn export_binds_both_backends_and_registry_without_mutation_authority() {
        let handoff = stack::CodexHandoff {
            schema_version: 1,
            mcp_server: "/profile/bin/hubu-unified-mcp".into(),
            hubu_endpoint: "http://127.0.0.1:8787".into(),
            hubu_token_file: "/profile/hubu-token".into(),
            approval_token_file: "/profile/approval".into(),
            reconciliation_token_file: "/profile/reconciliation".into(),
            operation_state_path: "/profile/operations.sqlite3".into(),
            gongbu_endpoint: Some("http://127.0.0.1:8789".into()),
            gongbu_token_file: Some("/profile/gongbu-token".into()),
        };
        let args = vec![
            "export".into(),
            "--operation-handle".into(),
            "op_example".into(),
        ];
        let command = export_command(&handoff, &args).unwrap();
        assert_eq!(command.get_program(), handoff.mcp_server.as_os_str());
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["gallery", "export", "--operation-handle", "op_example"]
        );
        let environment = command
            .get_envs()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(
            environment[std::ffi::OsStr::new("HUBU_UNIFIED_OPERATION_STATE_PATH")],
            Some(handoff.operation_state_path.as_os_str())
        );
        assert_eq!(
            environment[std::ffi::OsStr::new("HUBU_UNIFIED_HUBU_BEARER_TOKEN_FILE")],
            Some(handoff.hubu_token_file.as_os_str())
        );
        assert_eq!(
            environment[std::ffi::OsStr::new("HUBU_UNIFIED_GONGBU_BEARER_TOKEN_FILE")],
            Some(handoff.gongbu_token_file.as_ref().unwrap().as_os_str())
        );
        assert_eq!(
            environment[std::ffi::OsStr::new("HUBU_UNIFIED_HUBU_BEARER_TOKEN")],
            None
        );
        assert_eq!(
            environment[std::ffi::OsStr::new("HUBU_APPROVAL_TOKEN")],
            None
        );
        assert_eq!(
            environment[std::ffi::OsStr::new("HUBU_MCP_TRUST_SPEND_APPROVAL")],
            None
        );
        let mut hubu_only = handoff;
        hubu_only.gongbu_endpoint = None;
        assert!(export_command(&hubu_only, &args).is_err());
    }
}
