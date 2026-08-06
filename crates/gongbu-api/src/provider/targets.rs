use crate::secrets::{SecretError, SecretReference};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::Path,
};
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
    #[error("invalid provider secret reference")]
    InvalidSecretReference,
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
    #[serde(default)]
    pub gemini_image: Option<GeminiImageConfig>,
    #[serde(default)]
    pub flux2_api: Option<Flux2ApiConfig>,
    #[serde(default)]
    pub ideogram_image: Option<IdeogramImageConfig>,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GeminiImageConfig {
    pub endpoint: String,
    pub api_version: String,
    pub project: String,
    pub location: String,
    pub timeout_ms: u64,
    #[serde(default)]
    pub max_retries: u32,
    #[serde(default)]
    pub approved_artifact_hosts: Vec<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Flux2ApiConfig {
    pub endpoint: String,
    pub api_version: String,
    pub timeout_ms: u64,
    #[serde(default = "default_poll_interval_ms")]
    pub poll_interval_ms: u64,
    #[serde(default)]
    pub max_retries: u32,
    #[serde(default)]
    pub idempotency_header: Option<String>,
    #[serde(default)]
    pub approved_artifact_hosts: Vec<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IdeogramImageConfig {
    pub endpoint: String,
    pub api_version: String,
    pub timeout_ms: u64,
    #[serde(default)]
    pub max_retries: u32,
    #[serde(default)]
    pub approved_artifact_hosts: Vec<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

fn default_poll_interval_ms() -> u64 {
    500
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
            target
                .secret_reference()
                .map_err(|_| Error::InvalidSecretReference)?;
            if target.provider == "google" || target.adapter == "gemini_image" {
                let Some(gemini) = &target.gemini_image else {
                    return Err(Error::EmptyIdentifier);
                };
                if target.provider != "google"
                    || target.adapter != "gemini_image"
                    || [
                        &gemini.endpoint,
                        &gemini.api_version,
                        &gemini.project,
                        &gemini.location,
                    ]
                    .iter()
                    .any(|value| value.trim().is_empty())
                    || gemini.timeout_ms == 0
                    || gemini.max_retries != 0
                    || gemini.headers.iter().any(|(name, value)| {
                        name.trim().is_empty()
                            || value.contains(['\r', '\n'])
                            || name.eq_ignore_ascii_case("authorization")
                            || name.eq_ignore_ascii_case("x-goog-api-key")
                    })
                {
                    return Err(Error::EmptyIdentifier);
                }
            } else if target.gemini_image.is_some() {
                return Err(Error::EmptyIdentifier);
            }
            if target.provider == "flux" || target.adapter == "flux2_api" {
                let Some(flux) = &target.flux2_api else {
                    return Err(Error::EmptyIdentifier);
                };
                if target.provider != "flux"
                    || target.adapter != "flux2_api"
                    || flux.endpoint.trim().is_empty()
                    || flux.api_version.trim().is_empty()
                    || flux.timeout_ms == 0
                    || flux.poll_interval_ms == 0
                    || flux.max_retries != 0
                    || flux.idempotency_header.as_ref().is_some_and(|idempotency| {
                        idempotency.eq_ignore_ascii_case("x-key")
                            || idempotency.eq_ignore_ascii_case("authorization")
                            || flux
                                .headers
                                .keys()
                                .any(|header| header.eq_ignore_ascii_case(idempotency))
                    })
                    || flux.headers.iter().any(|(name, value)| {
                        name.trim().is_empty()
                            || value.contains(['\r', '\n'])
                            || name.eq_ignore_ascii_case("authorization")
                            || name.eq_ignore_ascii_case("x-key")
                    })
                {
                    return Err(Error::EmptyIdentifier);
                }
            } else if target.flux2_api.is_some() {
                return Err(Error::EmptyIdentifier);
            }
            if target.provider == "ideogram" || target.adapter == "ideogram_image" {
                let Some(ideogram) = &target.ideogram_image else {
                    return Err(Error::EmptyIdentifier);
                };
                if target.provider != "ideogram"
                    || target.adapter != "ideogram_image"
                    || [&ideogram.endpoint, &ideogram.api_version]
                        .iter()
                        .any(|value| value.trim().is_empty())
                    || ideogram.timeout_ms == 0
                    || ideogram.max_retries != 0
                    || ideogram.approved_artifact_hosts.is_empty()
                    || ideogram.headers.iter().any(|(name, value)| {
                        name.trim().is_empty()
                            || value.contains(['\r', '\n'])
                            || name.eq_ignore_ascii_case("authorization")
                            || name.eq_ignore_ascii_case("api-key")
                    })
                {
                    return Err(Error::EmptyIdentifier);
                }
            } else if target.ideogram_image.is_some() {
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
    fn validates_only_operator_owned_ideogram_generation_config() {
        let valid = parse(
            r#"{"provider_configs":[{"provider_config_version":"ideogram-v1","workload_type":"image_generation","provider":"ideogram","adapter":"ideogram_image","model":"ideogram-v3","secret_service":"gongbu.ideogram","secret_account":"local","ideogram_image":{"endpoint":"https://api.ideogram.ai","api_version":"v1","timeout_ms":30000,"approved_artifact_hosts":["ideogram.ai"]}}]}"#,
        );
        valid.validate().unwrap();

        let retries = parse(
            r#"{"provider_configs":[{"provider_config_version":"ideogram-v1","workload_type":"image_generation","provider":"ideogram","adapter":"ideogram_image","model":"ideogram-v3","secret_service":"gongbu.ideogram","secret_account":"local","ideogram_image":{"endpoint":"https://api.ideogram.ai","api_version":"v1","timeout_ms":30000,"max_retries":1,"approved_artifact_hosts":["ideogram.ai"]}}]}"#,
        );
        assert!(matches!(retries.validate(), Err(Error::EmptyIdentifier)));

        let credential_header = parse(
            r#"{"provider_configs":[{"provider_config_version":"ideogram-v1","workload_type":"image_generation","provider":"ideogram","adapter":"ideogram_image","model":"ideogram-v3","secret_service":"gongbu.ideogram","secret_account":"local","ideogram_image":{"endpoint":"https://api.ideogram.ai","api_version":"v1","timeout_ms":30000,"approved_artifact_hosts":["ideogram.ai"],"headers":{"Api-Key":"must-not-live-here"}}}]}"#,
        );
        assert!(matches!(
            credential_header.validate(),
            Err(Error::EmptyIdentifier)
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
        let invalid = parse(&format!(
            r#"{{"provider_configs":[{{"provider_config_version":"pcv-1","workload_type":"image_generation","provider":"example","adapter":"fixture","model":"image-v1","secret_service":"{}","secret_account":"local"}}]}}"#,
            "x".repeat(256)
        ));
        assert!(matches!(
            invalid.validate(),
            Err(Error::InvalidSecretReference)
        ));
    }

    #[test]
    fn validates_operator_owned_flux2_configuration() {
        let config = parse(
            r#"{"provider_configs":[{"provider_config_version":"flux-pcv-1","workload_type":"image_generation","provider":"flux","adapter":"flux2_api","model":"flux-2-pro","secret_service":"gongbu.flux","secret_account":"local","flux2_api":{"endpoint":"https://api.bfl.ai","api_version":"v1","timeout_ms":30000,"poll_interval_ms":500,"max_retries":0,"idempotency_header":"x-idempotency-key","approved_artifact_hosts":["cdn.bfl.ai"],"headers":{"x-client":"gongbu"}}}]}"#,
        );
        config.validate().unwrap();
        let target = config
            .resolve("image_generation", "flux", "flux2_api", "flux-2-pro")
            .unwrap();
        assert_eq!(target.flux2_api.as_ref().unwrap().timeout_ms, 30_000);

        let mut retrying = config.clone();
        retrying.provider_configs[0]
            .flux2_api
            .as_mut()
            .unwrap()
            .max_retries = 1;
        assert!(retrying.validate().is_err());
        let mut credential_header = config;
        credential_header.provider_configs[0]
            .flux2_api
            .as_mut()
            .unwrap()
            .headers
            .insert("x-key".into(), "caller-secret".into());
        assert!(credential_header.validate().is_err());
        let mut idempotency_collision = retrying;
        let flux = idempotency_collision.provider_configs[0]
            .flux2_api
            .as_mut()
            .unwrap();
        flux.max_retries = 0;
        flux.idempotency_header = Some("X-CLIENT".into());
        assert!(idempotency_collision.validate().is_err());
    }
}
