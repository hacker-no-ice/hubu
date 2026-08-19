use std::{fmt::Write as _, fs, path::Path};

use anyhow::{anyhow, bail, Context, Result};

const MANAGED_BEGIN: &str = "# >>> hubu managed codex mcp";
const MANAGED_END: &str = "# <<< hubu managed codex mcp";

pub(crate) struct UnifiedConfig<'a> {
    pub mcp_server: &'a Path,
    pub hubu_endpoint: &'a str,
    pub hubu_token_file: &'a Path,
    pub reconciliation_token_file: &'a Path,
    pub gongbu: Option<(&'a str, &'a Path)>,
    pub trust_client_approval: bool,
}

pub(crate) struct StandaloneConfig<'a> {
    pub mcp_server: &'a Path,
    pub hubu_endpoint: &'a str,
    pub hubu_token_file: &'a Path,
    pub approval_token_file: &'a Path,
    pub reconciliation_token_file: &'a Path,
    pub trust_client_approval: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum UpdateMode {
    Unified,
    MigrateStandalone { gongbu_configured: bool },
    StandaloneCompatibility,
}

pub(crate) fn write_config(
    config_path: &Path,
    block: &str,
    force: bool,
    mode: UpdateMode,
) -> Result<()> {
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create Codex config directory `{}`", parent.display()))?;
    }
    let existing = match fs::read_to_string(config_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).with_context(|| format!("read `{}`", config_path.display()))
        }
    };
    let updated = upsert(&existing, block, force, mode)?;
    fs::write(config_path, updated)
        .with_context(|| format!("write Codex config `{}`", config_path.display()))
}

pub(crate) fn standalone_block(config: StandaloneConfig<'_>) -> String {
    let mut block = format!(
        "{MANAGED_BEGIN}\n\
         [mcp_servers.hubu]\n\
         command = \"{}\"\n\
         startup_timeout_sec = 10\n\
         tool_timeout_sec = 60\n\n\
         [mcp_servers.hubu.env]\n\
         HUBU_URL = \"{}\"\n\
         HUBU_AUTH_TOKEN_FILE = \"{}\"\n\
         HUBU_APPROVAL_TOKEN_FILE = \"{}\"\n\
         HUBU_RECONCILIATION_TOKEN_FILE = \"{}\"\n",
        toml_string(&config.mcp_server.display().to_string()),
        toml_string(config.hubu_endpoint),
        toml_string(&config.hubu_token_file.display().to_string()),
        toml_string(&config.approval_token_file.display().to_string()),
        toml_string(&config.reconciliation_token_file.display().to_string()),
    );
    finish_block(&mut block, config.trust_client_approval);
    block
}

pub(crate) fn unified_block(config: UnifiedConfig<'_>) -> String {
    let mut block = format!(
        "{MANAGED_BEGIN}\n\
         [mcp_servers.hubu]\n\
         command = \"{}\"\n\
         startup_timeout_sec = 10\n\
         tool_timeout_sec = 60\n\n\
         [mcp_servers.hubu.env]\n\
         HUBU_UNIFIED_HUBU_ENDPOINT = \"{}\"\n\
         HUBU_UNIFIED_HUBU_BEARER_TOKEN_FILE = \"{}\"\n\
         HUBU_RECONCILIATION_TOKEN_FILE = \"{}\"\n",
        toml_string(&config.mcp_server.display().to_string()),
        toml_string(config.hubu_endpoint),
        toml_string(&config.hubu_token_file.display().to_string()),
        toml_string(&config.reconciliation_token_file.display().to_string()),
    );
    if let Some((endpoint, token_file)) = config.gongbu {
        let _ = writeln!(
            block,
            "HUBU_UNIFIED_GONGBU_ENDPOINT = \"{}\"\nHUBU_UNIFIED_GONGBU_BEARER_TOKEN_FILE = \"{}\"",
            toml_string(endpoint),
            toml_string(&token_file.display().to_string()),
        );
    }
    finish_block(&mut block, config.trust_client_approval);
    block
}

fn finish_block(block: &mut String, trust_client_approval: bool) {
    if trust_client_approval {
        let _ = writeln!(block, "HUBU_MCP_TRUST_CLIENT_APPROVAL = \"1\"");
    }
    block.push_str(
        "\n[mcp_servers.hubu.tools.hubu_authorize_spend]\n\
         approval_mode = \"approve\"\n\n\
         [mcp_servers.hubu.tools.hubu_submit_spend]\n\
         approval_mode = \"approve\"\n",
    );
    let _ = writeln!(block, "{MANAGED_END}");
}

fn upsert(existing: &str, block: &str, force: bool, mode: UpdateMode) -> Result<String> {
    if let UpdateMode::MigrateStandalone { gongbu_configured } = mode {
        if contains_table(existing, is_gongbu_table) && !gongbu_configured {
            bail!(
                "Codex config contains a standalone [mcp_servers.gongbu] table; pass --gongbu-endpoint and --gongbu-token-file with --migrate-standalone"
            );
        }
        let without_managed = remove_managed_block(existing)?;
        let without_standalone = remove_tables(&without_managed, is_standalone_table);
        return Ok(append_block(&without_standalone, block));
    }
    if let Some((start, end)) = managed_block_range(existing)? {
        let lines = existing.lines().collect::<Vec<_>>();
        let mut updated = Vec::new();
        updated.extend(lines[..start].iter().copied());
        updated.extend(block.trim_end_matches('\n').lines());
        updated.extend(lines[end + 1..].iter().copied());
        let updated = join_lines(&updated);
        if matches!(mode, UpdateMode::Unified) {
            reject_standalone_gongbu(&updated)?;
        }
        return Ok(updated);
    }

    let existing = if contains_table(existing, is_hubu_table) {
        if !force {
            bail!(
                "Codex config already contains an unmanaged [mcp_servers.hubu] table; pass --force to replace it"
            );
        }
        remove_tables(existing, is_hubu_table)
    } else {
        existing.to_string()
    };
    if matches!(mode, UpdateMode::Unified) {
        reject_standalone_gongbu(&existing)?;
    }
    Ok(append_block(&existing, block))
}

fn reject_standalone_gongbu(config: &str) -> Result<()> {
    if contains_table(config, is_gongbu_table) {
        bail!(
            "Codex config contains a standalone [mcp_servers.gongbu] table; pass --migrate-standalone to replace both standalone entries"
        );
    }
    Ok(())
}

fn append_block(existing: &str, block: &str) -> String {
    let mut updated = existing.trim_end().to_string();
    if !updated.is_empty() {
        updated.push_str("\n\n");
    }
    updated.push_str(block.trim_end_matches('\n'));
    updated.push('\n');
    updated
}

fn remove_managed_block(existing: &str) -> Result<String> {
    let Some((start, end)) = managed_block_range(existing)? else {
        return Ok(existing.to_string());
    };
    let lines = existing.lines().collect::<Vec<_>>();
    let mut kept = Vec::new();
    kept.extend(lines[..start].iter().copied());
    kept.extend(lines[end + 1..].iter().copied());
    Ok(join_lines(&kept))
}

fn managed_block_range(existing: &str) -> Result<Option<(usize, usize)>> {
    let lines = existing.lines().collect::<Vec<_>>();
    let Some(start) = lines.iter().position(|line| line.trim() == MANAGED_BEGIN) else {
        return Ok(None);
    };
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find_map(|(index, line)| (line.trim() == MANAGED_END).then_some(index))
        .ok_or_else(|| anyhow!("Codex config has a Hubu managed block without an end marker"))?;
    Ok(Some((start, end)))
}

fn join_lines(lines: &[&str]) -> String {
    let mut value = lines.join("\n");
    value.push('\n');
    value
}

fn contains_table(config: &str, matches: fn(&str) -> bool) -> bool {
    config.lines().filter_map(table_name).any(matches)
}

fn remove_tables(config: &str, matches: fn(&str) -> bool) -> String {
    let mut kept = Vec::new();
    let mut skipping = false;
    for line in config.lines() {
        if let Some(table) = table_name(line) {
            skipping = matches(table);
        }
        if !skipping {
            kept.push(line);
        }
    }
    join_lines(&kept)
}

fn table_name(line: &str) -> Option<&str> {
    let trimmed = line
        .split_once('#')
        .map(|(before_comment, _)| before_comment)
        .unwrap_or(line)
        .trim();
    if trimmed.starts_with("[[") || !trimmed.starts_with('[') || !trimmed.ends_with(']') {
        return None;
    }
    Some(trimmed.trim_start_matches('[').trim_end_matches(']').trim())
}

fn is_hubu_table(table: &str) -> bool {
    table == "mcp_servers.hubu" || table.starts_with("mcp_servers.hubu.")
}

fn is_gongbu_table(table: &str) -> bool {
    table == "mcp_servers.gongbu" || table.starts_with("mcp_servers.gongbu.")
}

fn is_standalone_table(table: &str) -> bool {
    is_hubu_table(table) || is_gongbu_table(table)
}

fn toml_string(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character => escaped.push(character),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unified_block_contains_one_server_and_file_credentials() {
        let block = unified_block(UnifiedConfig {
            mcp_server: Path::new("/tmp/hubu \"dev\"/hubu-unified-mcp"),
            hubu_endpoint: "http://127.0.0.1:8787",
            hubu_token_file: Path::new("/tmp/hubu\\token"),
            reconciliation_token_file: Path::new("/tmp/hubu\\reconciliation-token"),
            gongbu: Some(("http://127.0.0.1:8788", Path::new("/tmp/gongbu-token"))),
            trust_client_approval: false,
        });
        assert_eq!(block.matches("[mcp_servers.hubu]").count(), 1);
        assert!(!block.contains("[mcp_servers.gongbu]"));
        assert!(block.contains("command = \"/tmp/hubu \\\"dev\\\"/hubu-unified-mcp\""));
        assert!(block.contains("HUBU_UNIFIED_HUBU_BEARER_TOKEN_FILE = \"/tmp/hubu\\\\token\""));
        assert!(block.contains("HUBU_UNIFIED_GONGBU_ENDPOINT"));
    }

    #[test]
    fn explicit_migration_replaces_both_standalone_namespaces() {
        let existing = "model = \"gpt-5.5\"\n\n[mcp_servers.hubu]\ncommand = \"hubu-mcp-server\"\n[mcp_servers.hubu.env]\nHUBU_URL = \"old\"\n\n[mcp_servers.gongbu]\ncommand = \"gongbu-mcp\"\n[mcp_servers.gongbu.env]\nGONGBU_MCP_ENDPOINT = \"old\"\n\n[mcp_servers.other]\ncommand = \"keep\"\n";
        let block = format!(
            "{MANAGED_BEGIN}\n[mcp_servers.hubu]\ncommand = \"hubu-unified-mcp\"\n{MANAGED_END}\n"
        );
        let updated = upsert(
            existing,
            &block,
            false,
            UpdateMode::MigrateStandalone {
                gongbu_configured: true,
            },
        )
        .unwrap();
        assert!(updated.contains("command = \"hubu-unified-mcp\""));
        assert!(updated.contains("[mcp_servers.other]"));
        assert!(!updated.contains("hubu-mcp-server"));
        assert!(!updated.contains("gongbu-mcp"));
        assert!(!updated.contains("GONGBU_MCP_ENDPOINT"));
    }

    #[test]
    fn migration_preserves_standalone_gongbu_when_replacement_is_missing() {
        let existing = "[mcp_servers.hubu]\ncommand = \"hubu-mcp-server\"\n\n[mcp_servers.gongbu]\ncommand = \"gongbu-mcp\"\n";
        let block = format!(
            "{MANAGED_BEGIN}\n[mcp_servers.hubu]\ncommand = \"hubu-unified-mcp\"\n{MANAGED_END}\n"
        );
        let error = upsert(
            existing,
            &block,
            false,
            UpdateMode::MigrateStandalone {
                gongbu_configured: false,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("--gongbu-endpoint"));
        assert!(error.to_string().contains("--gongbu-token-file"));
        assert!(existing.contains("command = \"gongbu-mcp\""));
    }

    #[test]
    fn default_update_rejects_a_remaining_gongbu_entry() {
        let existing = "[mcp_servers.gongbu]\ncommand = \"gongbu-mcp\"\n";
        let block = format!(
            "{MANAGED_BEGIN}\n[mcp_servers.hubu]\ncommand = \"hubu-unified-mcp\"\n{MANAGED_END}\n"
        );
        assert!(upsert(existing, &block, false, UpdateMode::Unified)
            .unwrap_err()
            .to_string()
            .contains("--migrate-standalone"));
    }

    #[test]
    fn standalone_block_remains_an_explicit_compatibility_surface() {
        let block = standalone_block(StandaloneConfig {
            mcp_server: Path::new("/tmp/hubu-mcp-server"),
            hubu_endpoint: "http://127.0.0.1:8787",
            hubu_token_file: Path::new("/tmp/hubu-token"),
            approval_token_file: Path::new("/tmp/approval-token"),
            reconciliation_token_file: Path::new("/tmp/reconciliation-token"),
            trust_client_approval: true,
        });
        assert!(block.contains("command = \"/tmp/hubu-mcp-server\""));
        assert!(block.contains("HUBU_MCP_TRUST_CLIENT_APPROVAL = \"1\""));

        let existing = "[mcp_servers.gongbu]\ncommand = \"gongbu-mcp\"\n";
        let updated = upsert(existing, &block, false, UpdateMode::StandaloneCompatibility).unwrap();
        assert!(updated.contains("[mcp_servers.gongbu]"));
    }
}
