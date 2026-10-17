use serde::Serialize;

/// The executor contract this build speaks: exact amounts with an explicit
/// `scale`.
pub const EXECUTOR_CONTRACT: &str = "hubu-spend-executor-v4.5";
/// The previous contract, still accepted during the transition. Requests
/// without a `contract_version` use it and carry whole cents.
pub const LEGACY_EXECUTOR_CONTRACT: &str = "hubu-spend-executor-v4.4";
/// Every executor contract version Hubu accepts, newest first.
pub const SUPPORTED_EXECUTOR_CONTRACTS: [&str; 2] = [EXECUTOR_CONTRACT, LEGACY_EXECUTOR_CONTRACT];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BuildInfo {
    pub product_version: &'static str,
    pub source_commit: &'static str,
    pub executor_contract: &'static str,
}

pub const fn build_info() -> BuildInfo {
    BuildInfo {
        product_version: match option_env!("HUBU_PRODUCT_VERSION") {
            Some(version) => version,
            None => env!("CARGO_PKG_VERSION"),
        },
        source_commit: match option_env!("HUBU_SOURCE_COMMIT") {
            Some(commit) => commit,
            None => "unknown",
        },
        executor_contract: EXECUTOR_CONTRACT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_info_is_safe_and_complete() {
        let info = build_info();

        assert!(!info.product_version.is_empty());
        assert!(!info.source_commit.is_empty());
        assert_eq!(info.executor_contract, "hubu-spend-executor-v4.5");
    }
}
