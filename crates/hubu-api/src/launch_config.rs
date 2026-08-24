use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    net::SocketAddr,
    path::{Component, Path, PathBuf},
};
use thiserror::Error;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HubuLaunchConfig {
    pub schema_version: u32,
    pub listen: SocketAddr,
    pub database_path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_file: Option<PathBuf>,
    pub auth_token_file: PathBuf,
    pub approval_token_file: PathBuf,
    pub reconciliation_token_file: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_config: Option<PathBuf>,
}

#[derive(Debug, Error)]
pub enum LaunchConfigError {
    #[error("hubu-server launch configuration: {0}")]
    Invalid(String),
    #[error("hubu-server launch configuration IO: {0}")]
    Io(#[from] io::Error),
    #[error("hubu-server launch configuration JSON: {0}")]
    Json(#[from] serde_json::Error),
}

impl HubuLaunchConfig {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, LaunchConfigError> {
        let path = path.as_ref();
        if !path.is_absolute() {
            return Err(invalid("--config must be an absolute path"));
        }
        let config: Self = serde_json::from_slice(&fs::read(path)?)?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), LaunchConfigError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(invalid("unsupported schema_version"));
        }
        if !self.listen.ip().is_loopback() {
            return Err(invalid("listen address must be loopback"));
        }
        validate_safe_absolute(&self.database_path, "database_path", false)?;
        if let Some(path) = &self.log_file {
            validate_safe_absolute(path, "log_file", false)?;
        }
        for (path, name) in [
            (&self.auth_token_file, "auth_token_file"),
            (&self.approval_token_file, "approval_token_file"),
            (&self.reconciliation_token_file, "reconciliation_token_file"),
        ] {
            validate_safe_absolute(path, name, true)?;
        }
        if let Some(path) = &self.lease_config {
            validate_safe_absolute(path, "lease_config", true)?;
        }
        let auth = fs::canonicalize(&self.auth_token_file)?;
        let approval = fs::canonicalize(&self.approval_token_file)?;
        let reconciliation = fs::canonicalize(&self.reconciliation_token_file)?;
        if auth == approval || auth == reconciliation || approval == reconciliation {
            return Err(invalid("credential-file references must be distinct"));
        }
        Ok(())
    }
}

fn validate_safe_absolute(
    path: &Path,
    name: &str,
    require_file: bool,
) -> Result<(), LaunchConfigError> {
    if !path.is_absolute()
        || path == Path::new("/")
        || path.components().any(|part| part == Component::ParentDir)
    {
        return Err(invalid(format!("{name} must be a safe absolute path")));
    }
    if require_file && !path.is_file() {
        return Err(invalid(format!(
            "{name} must name an existing regular file"
        )));
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> LaunchConfigError {
    LaunchConfigError::Invalid(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn config(root: &Path) -> HubuLaunchConfig {
        for name in ["auth", "approval", "reconciliation"] {
            fs::write(root.join(name), format!("{name}-secret")).unwrap();
        }
        HubuLaunchConfig {
            schema_version: SCHEMA_VERSION,
            listen: "127.0.0.1:8787".parse().unwrap(),
            database_path: root.join("state/hubu.sqlite3"),
            log_file: Some(root.join("logs/hubu.jsonl")),
            auth_token_file: root.join("auth"),
            approval_token_file: root.join("approval"),
            reconciliation_token_file: root.join("reconciliation"),
            lease_config: None,
        }
    }

    #[test]
    fn validates_strict_side_effect_free_launch_input() {
        let root = tempdir().unwrap();
        let config = config(root.path());
        config.validate().unwrap();
        assert!(!root.path().join("state").exists());

        let mut value = serde_json::to_value(&config).unwrap();
        value["unknown"] = serde_json::json!(true);
        assert!(serde_json::from_value::<HubuLaunchConfig>(value).is_err());
    }

    #[test]
    fn rejects_unsafe_paths_and_reused_capabilities() {
        let root = tempdir().unwrap();
        let mut launch = config(root.path());
        launch.database_path = PathBuf::from("hubu.sqlite3");
        assert!(launch.validate().is_err());

        let mut launch = config(root.path());
        launch.approval_token_file = launch.auth_token_file.clone();
        assert!(launch.validate().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_distinct_symlinks_to_the_same_capability() {
        use std::os::unix::fs::symlink;

        let root = tempdir().unwrap();
        let shared = root.path().join("shared");
        fs::write(&shared, "secret").unwrap();
        let auth = root.path().join("auth-link");
        let approval = root.path().join("approval-link");
        symlink(&shared, &auth).unwrap();
        symlink(&shared, &approval).unwrap();
        let mut launch = config(root.path());
        launch.auth_token_file = auth;
        launch.approval_token_file = approval;
        assert!(launch.validate().is_err());
    }
}
