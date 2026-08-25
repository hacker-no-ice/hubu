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
        validate_safe_absolute(&self.database_path, "database_path")?;
        if let Some(path) = &self.log_file {
            validate_safe_absolute(path, "log_file")?;
        }
        for (path, name) in [
            (&self.auth_token_file, "auth_token_file"),
            (&self.approval_token_file, "approval_token_file"),
            (&self.reconciliation_token_file, "reconciliation_token_file"),
        ] {
            validate_safe_absolute(path, name)?;
        }
        if let Some(path) = &self.lease_config {
            validate_safe_absolute(path, "lease_config")?;
            if !path.is_file() {
                return Err(invalid("lease_config must name an existing regular file"));
            }
        }
        let mut resources = vec![
            (
                "database_path",
                resolve_credential_destination(&self.database_path, "database_path")?,
            ),
            (
                "auth_token_file",
                resolve_credential_destination(&self.auth_token_file, "auth_token_file")?,
            ),
            (
                "approval_token_file",
                resolve_credential_destination(&self.approval_token_file, "approval_token_file")?,
            ),
            (
                "reconciliation_token_file",
                resolve_credential_destination(
                    &self.reconciliation_token_file,
                    "reconciliation_token_file",
                )?,
            ),
        ];
        if let Some(path) = &self.log_file {
            resources.push((
                "log_file",
                resolve_credential_destination(path, "log_file")?,
            ));
        }
        if let Some(path) = &self.lease_config {
            resources.push((
                "lease_config",
                resolve_credential_destination(path, "lease_config")?,
            ));
        }
        for index in 0..resources.len() {
            for other in &resources[index + 1..] {
                if same_file_resource(&resources[index].1, &other.1)? {
                    return Err(invalid(format!(
                        "{} and {} must identify distinct file resources",
                        resources[index].0, other.0
                    )));
                }
            }
        }
        Ok(())
    }
}

fn validate_safe_absolute(path: &Path, name: &str) -> Result<(), LaunchConfigError> {
    if !path.is_absolute()
        || path == Path::new("/")
        || path.components().any(|part| part == Component::ParentDir)
    {
        return Err(invalid(format!("{name} must be a safe absolute path")));
    }
    Ok(())
}

fn resolve_credential_destination(path: &Path, name: &str) -> Result<PathBuf, LaunchConfigError> {
    let mut existing = path;
    let mut suffix = Vec::new();
    loop {
        match fs::symlink_metadata(existing) {
            Ok(metadata) => {
                if suffix.is_empty() {
                    if !metadata.file_type().is_file() {
                        return Err(invalid(format!(
                            "{name} must name a regular file when it already exists"
                        )));
                    }
                } else if !metadata.file_type().is_dir() {
                    return Err(invalid(format!(
                        "{name} must have an existing directory ancestor"
                    )));
                }
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let leaf = existing
                    .file_name()
                    .ok_or_else(|| invalid(format!("{name} has no file name")))?;
                suffix.push(leaf.to_os_string());
                existing = existing
                    .parent()
                    .ok_or_else(|| invalid(format!("{name} has no existing ancestor")))?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    let mut resolved = fs::canonicalize(existing)?;
    for part in suffix.into_iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}

fn same_file_resource(left: &Path, right: &Path) -> Result<bool, LaunchConfigError> {
    #[cfg(unix)]
    if left.exists() && right.exists() {
        use std::os::unix::fs::MetadataExt;
        let left = fs::metadata(left)?;
        let right = fs::metadata(right)?;
        if left.dev() == right.dev() && left.ino() == right.ino() {
            return Ok(true);
        }
    }
    Ok(left == right)
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
    fn accepts_distinct_missing_managed_credential_destinations_without_creating_them() {
        let root = tempdir().unwrap();
        let config = config(root.path());
        for path in [
            &config.auth_token_file,
            &config.approval_token_file,
            &config.reconciliation_token_file,
        ] {
            fs::remove_file(path).unwrap();
        }

        config.validate().unwrap();

        assert!(!config.auth_token_file.exists());
        assert!(!config.approval_token_file.exists());
        assert!(!config.reconciliation_token_file.exists());
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

        let mut launch = config(root.path());
        launch.log_file = Some(launch.auth_token_file.clone());
        assert!(launch
            .validate()
            .unwrap_err()
            .to_string()
            .contains("auth_token_file and log_file"));

        let mut launch = config(root.path());
        launch.database_path = launch.approval_token_file.clone();
        assert!(launch
            .validate()
            .unwrap_err()
            .to_string()
            .contains("database_path and approval_token_file"));
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
