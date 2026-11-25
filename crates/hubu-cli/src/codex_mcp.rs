use std::{fmt::Write as _, fs, io::Write as _, path::Path};

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
    if updated != existing {
        replace_config(config_path, &updated)?;
    }
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

fn replace_config(path: &Path, contents: &str) -> Result<()> {
    // Follow an operator-managed symlink and replace its target atomically.
    let path = match fs::canonicalize(path) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => path.to_path_buf(),
        Err(error) => {
            return Err(error).with_context(|| format!("resolve Codex config `{}`", path.display()))
        }
    };
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("stage Codex config `{}`", path.display()))?;
    temporary.write_all(contents.as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(&path)
        .with_context(|| format!("replace Codex config `{}`", path.display()))?;
    Ok(())
}

const BEARER_FILE: &str = "HUBU_UNIFIED_HUBU_BEARER_TOKEN_FILE";
const STATE_PATH: &str = "HUBU_UNIFIED_OPERATION_STATE_PATH";
const PROMPT_TOOL: &str = "hubu_prompt_spend_approval";
const RETIRED_APPROVAL_TOOL: &str = "hubu_resolve_spend_approval";
const RETIRED_ENV: &[&str] = &[
    "HUBU_UNIFIED_HUBU_BEARER_TOKEN",
    "HUBU_AUTH_TOKEN",
    "HUBU_AUTH_TOKEN_FILE",
    "HUBU_APPROVAL_TOKEN",
    "HUBU_APPROVAL_TOKEN_FILE",
    "HUBU_RECONCILIATION_TOKEN",
    "HUBU_RECONCILIATION_TOKEN_FILE",
    "HUBU_MCP_TRUST_CLIENT_APPROVAL",
    "HUBU_MCP_TRUST_SPEND_APPROVAL",
];

fn stale_profile_block(
    existing: &str,
    handoff: &crate::stack::CodexHandoff,
) -> Result<Option<toml::Table>> {
    let Some((start, end)) = managed_block_range(existing)? else {
        return Ok(None);
    };
    let block = existing
        .lines()
        .skip(start + 1)
        .take(end - start - 1)
        .collect::<Vec<_>>();
    let managed = toml::from_str::<toml::Table>(&join_lines(&block))
        .map_err(|_| anyhow!("the Hubu-managed Codex configuration is invalid TOML"))?;
    let Some(server) = managed
        .get("mcp_servers")
        .and_then(|servers| servers.get("hubu"))
    else {
        return Ok(None);
    };
    let Some(env) = server.get("env") else {
        return Ok(None);
    };
    let references = |key: &str, path: &Path| {
        env.get(key)
            .and_then(toml::Value::as_str)
            .map(Path::new)
            .is_some_and(|configured| {
                configured == path
                    || fs::canonicalize(configured).is_ok_and(|resolved| {
                        fs::canonicalize(path).is_ok_and(|expected| expected == resolved)
                    })
            })
    };
    let matches_profile = if env.get(STATE_PATH).is_some() {
        references(STATE_PATH, &handoff.operation_state_path)
    } else {
        references(BEARER_FILE, &handoff.hubu_token_file)
            || handoff
                .agent_token_file
                .as_deref()
                .is_some_and(|path| references(BEARER_FILE, path))
    };
    if !matches_profile {
        return Ok(None);
    }
    let agent = handoff.agent_bearer_file()?;
    let has_prompt = server
        .get("tools")
        .and_then(|tools| tools.get(PROMPT_TOOL))
        .is_some();
    let stale = !references(BEARER_FILE, agent)
        || !has_prompt
        || RETIRED_ENV.iter().any(|key| env.get(*key).is_some());
    Ok(stale.then_some(managed))
}

pub(crate) fn profile_config_is_stale(
    path: &Path,
    handoff: &crate::stack::CodexHandoff,
) -> Result<bool> {
    Ok(stale_profile_block(&read_config(path)?, handoff)?.is_some())
}

pub(crate) fn migrate_profile_config(
    path: &Path,
    handoff: &crate::stack::CodexHandoff,
    save_folders_file: &Path,
) -> Result<bool> {
    let existing = read_config(path)?;
    let Some(mut managed) = stale_profile_block(&existing, handoff)? else {
        return Ok(false);
    };
    let server = managed
        .get_mut("mcp_servers")
        .and_then(|servers| servers.get_mut("hubu"))
        .and_then(toml::Value::as_table_mut)
        .ok_or_else(|| anyhow!("the Hubu-managed Codex server is not a table"))?;
    // Only update Hubu-owned connection settings. Keep an operator's
    // explicit disabled state, wrappers, timeouts, and other tool settings.
    if server
        .get("command")
        .and_then(toml::Value::as_str)
        .is_some_and(|value| {
            Path::new(value)
                .file_name()
                .is_some_and(|name| name == "hubu-unified-mcp")
        })
    {
        server.insert(
            "command".into(),
            handoff.mcp_server.display().to_string().into(),
        );
    }
    let env = server
        .get_mut("env")
        .and_then(toml::Value::as_table_mut)
        .ok_or_else(|| anyhow!("the Hubu-managed Codex server environment is not a table"))?;
    for key in RETIRED_ENV {
        env.remove(*key);
    }
    for (key, value) in [
        (
            BEARER_FILE,
            handoff.agent_bearer_file()?.display().to_string(),
        ),
        (
            STATE_PATH,
            handoff.operation_state_path.display().to_string(),
        ),
        ("HUBU_UNIFIED_HUBU_ENDPOINT", handoff.hubu_endpoint.clone()),
    ] {
        env.insert(key.into(), value.into());
    }
    env.entry("HUBU_UNIFIED_SAVE_FOLDERS_FILE".to_owned())
        .or_insert_with(|| save_folders_file.display().to_string().into());
    let gongbu_endpoint = "HUBU_UNIFIED_GONGBU_ENDPOINT";
    let gongbu_bearer = "HUBU_UNIFIED_GONGBU_BEARER_TOKEN_FILE";
    if let Some((endpoint, token)) = handoff
        .gongbu_endpoint
        .as_ref()
        .zip(handoff.gongbu_token_file.as_ref())
    {
        env.insert(gongbu_endpoint.into(), endpoint.clone().into());
        env.insert(gongbu_bearer.into(), token.display().to_string().into());
    } else {
        env.remove(gongbu_endpoint);
        env.remove(gongbu_bearer);
    }

    let tools = server
        .entry("tools".to_owned())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .ok_or_else(|| anyhow!("the Hubu-managed Codex tools configuration is not a table"))?;
    tools.remove(RETIRED_APPROVAL_TOOL);
    tools.entry(PROMPT_TOOL.to_owned()).or_insert_with(|| {
        toml::Value::Table(toml::Table::from_iter([(
            "approval_mode".to_owned(),
            "approve".into(),
        )]))
    });
    let managed =
        toml::to_string(&managed).context("serialize the updated Hubu-managed Codex block")?;
    let block = format!("{MANAGED_BEGIN}\n{managed}{MANAGED_END}\n");
    replace_config(path, &upsert(&existing, &block, false)?)?;
    Ok(true)
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
    fn upgrades_only_the_started_profiles_managed_block() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config.toml");
        let existing = append_block(r#"model = "keep-me""#, &format!(
            "{MANAGED_BEGIN}\n[mcp_servers.hubu]\ncommand = \"old-mcp\"\nenabled = false\nargs = [\"--wrapper\"]\ntool_timeout_sec = 99\n\n[mcp_servers.hubu.env]\n{BEARER_FILE} = \"/profiles/alpha/owner\"\nHUBU_MCP_TRUST_CLIENT_APPROVAL = \"1\"\nHUBU_APPROVAL_TOKEN_FILE = \"/profiles/alpha/retired\"\nHUBU_UNIFIED_OPERATION_KEY_DB = \"/custom/operation-keys.sqlite3\"\n[mcp_servers.hubu.tools.custom]\napproval_mode = \"prompt\"\n{MANAGED_END}\n"));
        fs::write(&config, &existing).unwrap();
        let handoff = |name: &str| {
            serde_json::from_value(serde_json::json!({
                "schema_version": 2,
                "mcp_server": "/new/hubu-unified-mcp",
                "hubu_endpoint": "http://127.0.0.1:8787",
                "hubu_token_file": format!("/profiles/{name}/owner"),
                "agent_token_file": format!("/profiles/{name}/agent"),
                "operation_state_path": format!("/profiles/{name}/operations.sqlite3"),
            }))
            .unwrap()
        };
        let save_folders = Path::new("/custom/save-folders.json");
        assert!(!migrate_profile_config(&config, &handoff("beta"), save_folders).unwrap());
        assert_eq!(read_config(&config).unwrap(), existing);

        assert!(profile_config_is_stale(&config, &handoff("alpha")).unwrap());
        assert!(migrate_profile_config(&config, &handoff("alpha"), save_folders).unwrap());
        let upgraded = read_config(&config).unwrap();
        assert!(upgraded.contains(r#"model = "keep-me""#));
        assert!(
            upgraded.contains(r#"HUBU_UNIFIED_HUBU_BEARER_TOKEN_FILE = "/profiles/alpha/agent""#)
        );
        assert!(upgraded
            .contains(r#"HUBU_UNIFIED_OPERATION_KEY_DB = "/custom/operation-keys.sqlite3""#));
        let parsed = toml::from_str::<toml::Value>(&upgraded).unwrap();
        let hubu = &parsed["mcp_servers"]["hubu"];
        assert_eq!(hubu["enabled"].as_bool(), Some(false));
        assert_eq!(hubu["command"].as_str(), Some("old-mcp"));
        assert_eq!(
            hubu["args"].as_array().unwrap()[0].as_str(),
            Some("--wrapper")
        );
        assert_eq!(hubu["tool_timeout_sec"].as_integer(), Some(99));
        assert_eq!(
            hubu["tools"]["custom"]["approval_mode"].as_str(),
            Some("prompt")
        );
        assert!(upgraded.contains("hubu_prompt_spend_approval"));
        assert!(!upgraded.contains("/profiles/alpha/owner"));
        assert!(!upgraded.contains("HUBU_APPROVAL_TOKEN_FILE"));
        assert!(!upgraded.contains("HUBU_MCP_TRUST_CLIENT_APPROVAL"));
        assert!(!profile_config_is_stale(&config, &handoff("alpha")).unwrap());
        assert!(!migrate_profile_config(&config, &handoff("alpha"), save_folders).unwrap());
        assert_eq!(read_config(&config).unwrap(), upgraded);
        // A literal bearer takes precedence over the file in unified MCP.
        let shadowed = upgraded.replace(
            "[mcp_servers.hubu.env]",
            "[mcp_servers.hubu.env]\nHUBU_UNIFIED_HUBU_BEARER_TOKEN = \"owner-bearer-override\"",
        );
        fs::write(&config, shadowed).unwrap();
        assert!(profile_config_is_stale(&config, &handoff("alpha")).unwrap());
        assert!(migrate_profile_config(&config, &handoff("alpha"), save_folders).unwrap());
        assert_eq!(read_config(&config).unwrap(), upgraded);
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
