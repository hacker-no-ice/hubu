const UNSUPPORTED_NOTICE: &str = "WARNING: hubu-mcp-server is deprecated and unsupported. Migrate to the only supported agent-facing surface, hubu-unified-mcp: run `hubu init codex --migrate-standalone` or see docs/unified-mcp-migration.md. Standalone source remains only for HUB-98 removal staging.";

fn main() {
    eprintln!("{UNSUPPORTED_NOTICE}");

    if std::env::args()
        .nth(1)
        .is_some_and(|argument| matches!(argument.as_str(), "help" | "--help" | "-h"))
    {
        println!(
            "hubu-mcp-server (unsupported)\n\nUse hubu-unified-mcp instead.\nMigration: hubu init codex --migrate-standalone\nGuide: docs/unified-mcp-migration.md"
        );
        return;
    }

    if std::env::args()
        .nth(1)
        .is_some_and(|argument| matches!(argument.as_str(), "version" | "--version" | "-V"))
    {
        println!(
            "{}",
            serde_json::to_string_pretty(&hubu_common::build::build_info())
                .expect("build metadata should serialize")
        );
        return;
    }

    if let Err(error) = hubu_mcp::run_stdio_from_env() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::UNSUPPORTED_NOTICE;

    #[test]
    fn unsupported_notice_is_actionable_and_contains_no_configuration_values() {
        assert!(UNSUPPORTED_NOTICE.contains("deprecated and unsupported"));
        assert!(UNSUPPORTED_NOTICE.contains("hubu-unified-mcp"));
        assert!(UNSUPPORTED_NOTICE.contains("--migrate-standalone"));
        assert!(!UNSUPPORTED_NOTICE.contains("HUBU_"));
        assert!(!UNSUPPORTED_NOTICE.contains("token"));
    }
}
