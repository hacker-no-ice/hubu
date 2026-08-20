//! Safe, non-secret release and compatibility metadata shared by Gongbu binaries.

use serde::{Deserialize, Serialize};

pub const API_SCHEMA_VERSION: u32 = 2;
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
pub const MCP_SCHEMA_VERSION: u32 = 2;
pub const SERVER_CONFIG_SCHEMA_VERSION: u32 = 2;
pub const HUBU_EXECUTOR_CONTRACT: &str = "hubu-spend-executor-v4.2";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BuildInfo {
    pub product_version: String,
    pub source_commit: String,
    pub build_id: String,
    pub api_schema_version: u32,
    pub mcp_protocol_version: String,
    pub mcp_schema_version: u32,
    pub server_config_schema_version: u32,
    pub hubu_executor_contract: String,
}

pub fn build_info() -> BuildInfo {
    BuildInfo {
        product_version: option_env!("GONGBU_PRODUCT_VERSION")
            .unwrap_or(env!("CARGO_PKG_VERSION"))
            .into(),
        source_commit: option_env!("GONGBU_SOURCE_COMMIT")
            .unwrap_or("unknown")
            .into(),
        build_id: option_env!("GONGBU_BUILD_ID").unwrap_or("local").into(),
        api_schema_version: API_SCHEMA_VERSION,
        mcp_protocol_version: MCP_PROTOCOL_VERSION.into(),
        mcp_schema_version: MCP_SCHEMA_VERSION,
        server_config_schema_version: SERVER_CONFIG_SCHEMA_VERSION,
        hubu_executor_contract: HUBU_EXECUTOR_CONTRACT.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_metadata_is_complete() {
        let info = build_info();
        assert!(!info.product_version.is_empty());
        assert!(!info.source_commit.is_empty());
        assert!(!info.build_id.is_empty());
        assert_eq!(info.api_schema_version, 2);
        assert_eq!(info.mcp_schema_version, 2);
        assert_eq!(info.server_config_schema_version, 2);
        assert_eq!(info.hubu_executor_contract, "hubu-spend-executor-v4.2");
    }
}
