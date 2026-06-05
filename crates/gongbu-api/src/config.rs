use std::env;

use anyhow::Result;

use crate::image_provider::ImageProviderConfig;
use crate::secrets::SecretProviderConfig;

const DEFAULT_BIND_ADDR: &str = "127.0.0.1:8790";
const DEFAULT_HUBU_BASE_URL: &str = "http://127.0.0.1:8787";

#[derive(Debug, Clone)]
pub struct Config {
    pub bind_addr: String,
    pub hubu_base_url: String,
    pub image_provider: ImageProviderConfig,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let secret_provider = SecretProviderConfig::from_env()?;
        let image_provider_api_key = secret_provider.load_image_provider_api_key()?;
        Ok(Self {
            bind_addr: env::var("GONGBU_BIND_ADDR").unwrap_or_else(|_| DEFAULT_BIND_ADDR.into()),
            hubu_base_url: env::var("HUBU_BASE_URL")
                .unwrap_or_else(|_| DEFAULT_HUBU_BASE_URL.into()),
            image_provider: ImageProviderConfig::from_env(image_provider_api_key)?,
        })
    }
}
