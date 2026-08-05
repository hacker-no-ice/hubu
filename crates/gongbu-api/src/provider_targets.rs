use crate::image_provider::ImageProviderConfig;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, env, fs, path::Path};

const PROVIDER_CONFIG_ENV: &str = "GONGBU_PROVIDER_CONFIG";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfigVersion {
    pub provider_config_version: String,
    pub workload_type: String,
    pub provider: String,
    pub adapter: String,
    pub model: String,
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
    pub fn from_env(image_provider: &ImageProviderConfig) -> Result<Self> {
        let path = env::var(PROVIDER_CONFIG_ENV)
            .with_context(|| format!("{PROVIDER_CONFIG_ENV} must name an operator-managed file"))?;
        Self::from_path(Path::new(&path), image_provider)
    }
    pub fn from_path(path: &Path, image_provider: &ImageProviderConfig) -> Result<Self> {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("read provider target config {}", path.display()))?;
        let config: Self = serde_json::from_str(&contents)
            .with_context(|| format!("parse provider target config {}", path.display()))?;
        config.validate(image_provider)?;
        Ok(config)
    }
    pub fn validate(&self, image_provider: &ImageProviderConfig) -> Result<()> {
        if self.provider_configs.is_empty() {
            return Err(anyhow!(
                "provider target config must define at least one version"
            ));
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
            ]
            .iter()
            .any(|value| value.trim().is_empty())
            {
                return Err(anyhow!("provider target identifiers cannot be empty"));
            }
            if !targets.insert((
                &target.workload_type,
                &target.provider,
                &target.adapter,
                &target.model,
            )) {
                return Err(anyhow!("duplicate provider target definition"));
            }
            if !versions.insert(&target.provider_config_version) {
                return Err(anyhow!("duplicate provider config version"));
            }
        }
        let wired = self
            .provider_configs
            .iter()
            .filter(|target| {
                target.enabled
                    && target.workload_type == "image_generation"
                    && target.provider == image_provider.provider
                    && target.adapter == image_provider.adapter_kind.label()
                    && target.model == image_provider.model
            })
            .count();
        if wired != 1 {
            return Err(anyhow!("provider target configuration is missing or ambiguous for the wired image provider"));
        }
        image_provider.adapter().map_err(|error| {
            let redacted = crate::image_provider::redact_image_provider_error_message(
                &error.to_string(),
                image_provider,
            );
            anyhow!("wired provider target is unavailable: {redacted}")
        })?;
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
            [_] => Err(anyhow!("requested provider target is disabled")),
            [] => Err(anyhow!("requested provider target is not configured")),
            _ => Err(anyhow!("requested provider target is ambiguous")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_provider::{HttpJsonImageProviderFields, ImageProviderAdapterKind};
    use std::path::PathBuf;
    fn provider() -> ImageProviderConfig {
        ImageProviderConfig {
            provider: "local-mock".into(),
            model: "mock-image-v1".into(),
            merchant: "gongbu.image".into(),
            api_key: None,
            endpoint: None,
            price_cents: 500,
            timeout_ms: 1,
            max_retries: 0,
            http_json_fields: HttpJsonImageProviderFields::defaults(),
            output_dir: PathBuf::from("/tmp"),
            adapter_kind: ImageProviderAdapterKind::Mock,
        }
    }
    fn parse(json: &str) -> ProviderTargetConfig {
        serde_json::from_str(json).unwrap()
    }
    #[test]
    fn resolves_exact_allowlisted_target_to_version() {
        let c = parse(
            r#"{"provider_configs":[{"provider_config_version":"pcv-1","workload_type":"image_generation","provider":"local-mock","adapter":"mock","model":"mock-image-v1"}]}"#,
        );
        c.validate(&provider()).unwrap();
        assert_eq!(
            c.resolve("image_generation", "local-mock", "mock", "mock-image-v1")
                .unwrap()
                .provider_config_version,
            "pcv-1"
        );
    }
    #[test]
    fn rejects_disabled_and_unknown_targets() {
        let c = parse(
            r#"{"provider_configs":[{"provider_config_version":"pcv-1","workload_type":"image_generation","provider":"local-mock","adapter":"mock","model":"mock-image-v1","enabled":false}]}"#,
        );
        assert!(c
            .resolve("image_generation", "local-mock", "mock", "mock-image-v1")
            .unwrap_err()
            .to_string()
            .contains("disabled"));
        assert!(c
            .resolve("image_generation", "other", "mock", "mock-image-v1")
            .unwrap_err()
            .to_string()
            .contains("not configured"));
    }
    #[test]
    fn duplicate_target_fails_startup() {
        let c = parse(
            r#"{"provider_configs":[{"provider_config_version":"pcv-1","workload_type":"image_generation","provider":"local-mock","adapter":"mock","model":"mock-image-v1"},{"provider_config_version":"pcv-2","workload_type":"image_generation","provider":"local-mock","adapter":"mock","model":"mock-image-v1"}]}"#,
        );
        assert!(c
            .validate(&provider())
            .unwrap_err()
            .to_string()
            .contains("duplicate provider target"));
    }
    #[test]
    fn missing_secret_fails_startup_without_leaking_endpoint() {
        let mut p = provider();
        p.provider = "vendor".into();
        p.model = "image-v1".into();
        p.adapter_kind = ImageProviderAdapterKind::HttpJson;
        p.endpoint = Some("https://secret.example/v1?signature=sensitive".into());
        let c = parse(
            r#"{"provider_configs":[{"provider_config_version":"pcv-1","workload_type":"image_generation","provider":"vendor","adapter":"http-json","model":"image-v1"}]}"#,
        );
        let error = c.validate(&p).unwrap_err().to_string();
        assert!(error.contains("requires GONGBU_IMAGE_PROVIDER_API_KEY"));
        assert!(!error.contains("sensitive"));
    }
}
