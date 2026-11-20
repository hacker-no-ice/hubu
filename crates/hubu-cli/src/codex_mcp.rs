use std::{fmt::Write as _, fs, path::Path};

use anyhow::{anyhow, bail, Context, Result};

const MANAGED_BEGIN: &str = "# >>> hubu managed codex mcp";
const MANAGED_END: &str = "# <<< hubu managed codex mcp";

pub(crate) struct UnifiedConfig<'a> {
    pub mcp_server: &'a Path,
    pub hubu_endpoint: &'a str,
    pub hubu_token_file: &'a Path,
    pub operation_state_path: &'a Path,
    pub operation_key_db: Option<&'a Path>,
    pub save_folders_file: &'a Path,
    pub gongbu: Option<(&'a str, &'a Path)>,
}

pub(crate) fn read_config(config_path: &Path) -> Result<String> {
    match fs::read_to_string(config_path) {
        Ok(contents) => Ok(contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error).with_context(|| format!("read `{}`", config_path.display())),
    }
}

pub(crate) fn write_config(config_path: &Path, block: &str, force: bool) -> Result<()> {
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create Codex config directory `{}`", parent.display()))?;
    }
    let existing = read_config(config_path)?;
    let updated = upsert(&existing, block, force)?;
    fs::write(config_path, &updated)
        .with_context(|| format!("write Codex config `{}`", config_path.display()))?;
    const RETIRED_FLAG: &str = "HUBU_MCP_TRUST_CLIENT_APPROVAL";
    let has_flag = |config: &str| {
        config.lines().any(|line| {
            line.split_once('=')
                .is_some_and(|(key, _)| key.trim() == RETIRED_FLAG)
        })
    };
    if has_flag(&existing) && !has_flag(&updated) {
        eprintln!(
            "{RETIRED_FLAG} is retired and ignored. Removed it from the managed Codex config."
        );
    }
    Ok(())
}

pub(crate) fn unified_block(config: UnifiedConfig<'_>) -> String {
    let mut block = format!(
        "{MANAGED_BEGIN}\n\
         [mcp_servers.hubu]\n\
         command = \"{}\"\n\
         startup_timeout_sec = 10\n\
         tool_timeout_sec = 75\n\n\
         [mcp_servers.hubu.env]\n\
         HUBU_UNIFIED_HUBU_ENDPOINT = \"{}\"\n\
         HUBU_UNIFIED_HUBU_BEARER_TOKEN_FILE = \"{}\"\n\
         HUBU_UNIFIED_OPERATION_STATE_PATH = \"{}\"\n\
         HUBU_UNIFIED_SAVE_FOLDERS_FILE = \"{}\"\n",
        toml_string(&config.mcp_server.display().to_string()),
        toml_string(config.hubu_endpoint),
        toml_string(&config.hubu_token_file.display().to_string()),
        toml_string(&config.operation_state_path.display().to_string()),
        toml_string(&config.save_folders_file.display().to_string()),
    );
    if let Some((endpoint, token_file)) = config.gongbu {
        let _ = writeln!(
            block,
            "HUBU_UNIFIED_GONGBU_ENDPOINT = \"{}\"\nHUBU_UNIFIED_GONGBU_BEARER_TOKEN_FILE = \"{}\"",
            toml_string(endpoint),
            toml_string(&token_file.display().to_string()),
        );
    }
    if let Some(operation_key_db) = config.operation_key_db {
        let _ = writeln!(
            block,
            "HUBU_UNIFIED_OPERATION_KEY_DB = \"{}\"",
            toml_string(&operation_key_db.display().to_string()),
        );
    }
    block.push_str(
        "\n[mcp_servers.hubu.tools.hubu_authorize_spend]\n\
         approval_mode = \"approve\"\n\n\
         [mcp_servers.hubu.tools.hubu_submit_governed_execution]\n\
         approval_mode = \"approve\"\n\n\
         [mcp_servers.hubu.tools.hubu_submit_spend]\n\
         approval_mode = \"approve\"\n\n\
         [mcp_servers.hubu.tools.hubu_prompt_spend_approval]\n\
         approval_mode = \"approve\"\n",
    );
    let _ = writeln!(block, "{MANAGED_END}");
    block
}

fn upsert(existing: &str, block: &str, force: bool) -> Result<String> {
    if let Some((start, end)) = managed_block_range(existing)? {
        let lines = existing.lines().collect::<Vec<_>>();
        let mut updated = Vec::new();
        updated.extend(lines[..start].iter().copied());
        updated.extend(block.trim_end_matches('\n').lines());
        updated.extend(lines[end + 1..].iter().copied());
        return Ok(join_lines(&updated));
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
    Ok(append_block(&existing, block))
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
            operation_state_path: Path::new("/tmp/hubu\\unified-operations.sqlite3"),
            operation_key_db: Some(Path::new("/tmp/hubu\\operation-keys.sqlite3")),
            save_folders_file: Path::new("/tmp/hubu/save-folders.json"),
            gongbu: Some(("http://127.0.0.1:8788", Path::new("/tmp/gongbu-token"))),
        });
        assert_eq!(block.matches("[mcp_servers.hubu]").count(), 1);
        assert!(!block.contains("[mcp_servers.gongbu]"));
        assert!(block.contains("command = \"/tmp/hubu \\\"dev\\\"/hubu-unified-mcp\""));
        assert!(block.contains("HUBU_UNIFIED_HUBU_BEARER_TOKEN_FILE = \"/tmp/hubu\\\\token\""));
        assert!(block.contains("HUBU_UNIFIED_GONGBU_ENDPOINT"));
        assert!(block.contains(
            "HUBU_UNIFIED_OPERATION_STATE_PATH = \"/tmp/hubu\\\\unified-operations.sqlite3\""
        ));
        assert!(block
            .contains("HUBU_UNIFIED_OPERATION_KEY_DB = \"/tmp/hubu\\\\operation-keys.sqlite3\""));
        assert!(block.contains("HUBU_UNIFIED_SAVE_FOLDERS_FILE = \"/tmp/hubu/save-folders.json\""));
        assert!(block.contains(
            "[mcp_servers.hubu.tools.hubu_authorize_spend]\napproval_mode = \"approve\""
        ));
        assert!(block.contains(
            "[mcp_servers.hubu.tools.hubu_submit_governed_execution]\napproval_mode = \"approve\""
        ));
        assert!(block
            .contains("[mcp_servers.hubu.tools.hubu_submit_spend]\napproval_mode = \"approve\""));
        assert!(block.contains(
            "[mcp_servers.hubu.tools.hubu_prompt_spend_approval]\napproval_mode = \"approve\""
        ));
        toml::from_str::<toml::Value>(&block).unwrap();
    }

    #[test]
    fn unified_block_omits_preallocated_key_store_until_explicitly_configured() {
        let block = unified_block(UnifiedConfig {
            mcp_server: Path::new("/tmp/hubu-unified-mcp"),
            hubu_endpoint: "http://127.0.0.1:8787",
            hubu_token_file: Path::new("/tmp/hubu-token"),
            operation_state_path: Path::new("/tmp/unified-operations.sqlite3"),
            operation_key_db: None,
            save_folders_file: Path::new("/tmp/hubu/save-folders.json"),
            gongbu: None,
        });

        assert!(!block.contains("HUBU_UNIFIED_OPERATION_KEY_DB"));
        toml::from_str::<toml::Value>(&block).unwrap();
    }
}
