use crate::secrets::{SecretError, SecretReference};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, env, fs, path::Path};
use thiserror::Error;

const PROVIDER_CONFIG_ENV: &str = "GONGBU_PROVIDER_CONFIG";

#[derive(Debug, Error)]
pub enum Error {
    #[error("{PROVIDER_CONFIG_ENV} must name an operator-managed file")]
    MissingConfigPath,
    #[error("read provider target config {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("parse provider target config {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("provider target config must define at least one version")]
    Empty,
    #[error("provider target identifiers cannot be empty")]
    EmptyIdentifier,
    #[error("duplicate provider target definition")]
    DuplicateTarget,
    #[error("duplicate provider config version")]
    DuplicateVersion,
    #[error("requested provider target is disabled")]
    Disabled,
    #[error("requested provider target is not configured")]
    NotConfigured,
    #[error("requested provider target is ambiguous")]
    Ambiguous,
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfigVersion {
    pub provider_config_version: String,
    pub workload_type: String,
    pub provider: String,
    pub adapter: String,
    pub model: String,
    pub secret_service: String,
    pub secret_account: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

fn enabled_by_default() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderTargetConfig {
    pub provider_configs: Vec<ProviderConfigVersion>,
}

impl ProviderTargetConfig {
    pub fn from_env() -> Result<Self> {
        let path = env::var(PROVIDER_CONFIG_ENV).map_err(|_| Error::MissingConfigPath)?;
        Self::from_path(Path::new(&path))
    }

    pub fn from_path(path: &Path) -> Result<Self> {
        let display = path.display().to_string();
        let contents = fs::read_to_string(path).map_err(|source| Error::Read {
            path: display.clone(),
            source,
        })?;
        let config: Self = serde_json::from_str(&contents).map_err(|source| Error::Parse {
            path: display,
            source,
        })?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.provider_configs.is_empty() {
            return Err(Error::Empty);
        }
        let mut targets = BTreeSet::new();
        let mut versions = BTreeSet::new();
        for target in &self.provider_configs {
            if [
                &target.provider_config_version,
                &target.workload_type,
                &target.provider,
                &target.adapter,
                &target.model,
                &target.secret_service,
                &target.secret_account,
            ]
            .iter()
            .any(|value| value.trim().is_empty())
            {
                return Err(Error::EmptyIdentifier);
            }
            if !targets.insert((
                &target.workload_type,
                &target.provider,
                &target.adapter,
                &target.model,
            )) {
                return Err(Error::DuplicateTarget);
            }
            if !versions.insert(&target.provider_config_version) {
                return Err(Error::DuplicateVersion);
            }
        }
        Ok(())
    }

    pub fn resolve(
        &self,
        workload_type: &str,
        provider: &str,
        adapter: &str,
        model: &str,
    ) -> Result<&ProviderConfigVersion> {
        let matches: Vec<_> = self
            .provider_configs
            .iter()
            .filter(|target| {
                target.workload_type == workload_type
                    && target.provider == provider
                    && target.adapter == adapter
                    && target.model == model
            })
            .collect();
        match matches.as_slice() {
            [target] if target.enabled => Ok(target),
            [_] => Err(Error::Disabled),
            [] => Err(Error::NotConfigured),
            _ => Err(Error::Ambiguous),
        }
    }
}

impl ProviderConfigVersion {
    pub fn secret_reference(&self) -> std::result::Result<SecretReference, SecretError> {
        SecretReference::new(self.secret_service.clone(), self.secret_account.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> ProviderTargetConfig {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn resolves_exact_allowlisted_target_to_version() {
        let config = parse(
            r#"{"provider_configs":[{"provider_config_version":"pcv-1","workload_type":"image_generation","provider":"example","adapter":"fixture","model":"image-v1","secret_service":"gongbu.example","secret_account":"local"}]}"#,
        );
        config.validate().unwrap();
        assert_eq!(
            config
                .resolve("image_generation", "example", "fixture", "image-v1")
                .unwrap()
                .provider_config_version,
            "pcv-1"
        );
    }

    #[test]
    fn rejects_disabled_and_unknown_targets() {
        let config = parse(
            r#"{"provider_configs":[{"provider_config_version":"pcv-1","workload_type":"image_generation","provider":"example","adapter":"fixture","model":"image-v1","secret_service":"gongbu.example","secret_account":"local","enabled":false}]}"#,
        );
        assert!(matches!(
            config.resolve("image_generation", "example", "fixture", "image-v1"),
            Err(Error::Disabled)
        ));
        assert!(matches!(
            config.resolve("image_generation", "other", "fixture", "image-v1"),
            Err(Error::NotConfigured)
        ));
    }

    #[test]
    fn rejects_duplicate_targets_and_versions() {
        let duplicate_target = parse(
            r#"{"provider_configs":[{"provider_config_version":"pcv-1","workload_type":"image_generation","provider":"example","adapter":"fixture","model":"image-v1","secret_service":"gongbu.example","secret_account":"local"},{"provider_config_version":"pcv-2","workload_type":"image_generation","provider":"example","adapter":"fixture","model":"image-v1","secret_service":"gongbu.example","secret_account":"local"}]}"#,
        );
        assert!(matches!(
            duplicate_target.validate(),
            Err(Error::DuplicateTarget)
        ));

        let duplicate_version = parse(
            r#"{"provider_configs":[{"provider_config_version":"pcv-1","workload_type":"image_generation","provider":"example","adapter":"fixture","model":"image-v1","secret_service":"gongbu.example","secret_account":"local"},{"provider_config_version":"pcv-1","workload_type":"image_generation","provider":"other","adapter":"fixture","model":"image-v1","secret_service":"gongbu.other","secret_account":"local"}]}"#,
        );
        assert!(matches!(
            duplicate_version.validate(),
            Err(Error::DuplicateVersion)
        ));
    }

    #[test]
    fn rejects_empty_and_unknown_fields() {
        assert!(matches!(
            parse(r#"{"provider_configs":[]}"#).validate(),
            Err(Error::Empty)
        ));
        assert!(serde_json::from_str::<ProviderTargetConfig>(
            r#"{"provider_configs":[],"endpoint":"https://caller.example"}"#
        )
        .is_err());
        assert!(serde_json::from_str::<ProviderTargetConfig>(
            r#"{"provider_configs":[],"authorization":"Bearer caller","retry_policy":{"max_retries":99}}"#
        ).is_err());
    }
}
