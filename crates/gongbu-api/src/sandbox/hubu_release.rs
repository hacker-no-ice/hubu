use super::SandboxError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

pub const EXECUTOR_CONTRACT: &str = "hubu-spend-executor-v4";
const DEFAULT_REPOSITORY: &str = "hacker-no-ice/hubu";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HubuReleaseConfig {
    pub version: String,
    #[serde(default = "default_repository")]
    pub repository: String,
    #[serde(default)]
    pub cache_dir: Option<PathBuf>,
}

fn default_repository() -> String {
    DEFAULT_REPOSITORY.into()
}

impl HubuReleaseConfig {
    pub fn pinned(version: impl Into<String>) -> Self {
        Self {
            version: version.into(),
            repository: default_repository(),
            cache_dir: None,
        }
    }

    pub fn validate(&self) -> Result<(), SandboxError> {
        validate_version(&self.version)?;
        let Some((owner, repo)) = self.repository.split_once('/') else {
            return Err(SandboxError::Invalid(
                "Hubu release repository must use owner/name format".into(),
            ));
        };
        if !safe_name(owner) || !safe_name(repo) || self.repository.matches('/').count() != 1 {
            return Err(SandboxError::Invalid(
                "Hubu release repository must use safe owner/name format".into(),
            ));
        }
        if self
            .cache_dir
            .as_ref()
            .is_some_and(|path| !path.is_absolute())
        {
            return Err(SandboxError::Invalid(
                "Hubu release cache_dir must be absolute".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HubuProvenance {
    pub product_version: String,
    pub source_commit: String,
    pub executor_contract: String,
    pub target: String,
    pub repository: String,
    pub workflow_run: String,
    pub dependencies: String,
}

#[derive(Clone, Debug)]
pub struct HubuRelease {
    pub version: String,
    pub source_commit: String,
    pub executor_contract: String,
    pub target: String,
    pub artifact_checksum: String,
    pub server_binary: PathBuf,
    pub cli_binary: PathBuf,
    pub provenance_path: PathBuf,
}

impl HubuRelease {
    pub fn resolve(config: &HubuReleaseConfig) -> Result<Self, SandboxError> {
        config.validate()?;
        let target = platform_target()?;
        let cache = config.cache_dir.clone().unwrap_or_else(default_cache_dir);
        fs::create_dir_all(&cache)?;
        let install = cache.join(&config.version).join(&target);
        if install.exists() {
            return validate_install(config, &target, &install);
        }

        let staging = tempfile::Builder::new()
            .prefix("hubu-release-")
            .tempdir_in(&cache)?;
        let archive_name = format!("hubu-{}-{target}.tar.gz", config.version);
        run(
            Command::new("gh")
                .args([
                    "release",
                    "download",
                    &config.version,
                    "--repo",
                    &config.repository,
                    "--pattern",
                    &archive_name,
                    "--pattern",
                    "SHA256SUMS",
                    "--dir",
                ])
                .arg(staging.path()),
            "download pinned Hubu release",
        )?;
        let archive = staging.path().join(&archive_name);
        let expected = checksum_for(
            &fs::read_to_string(staging.path().join("SHA256SUMS"))?,
            &archive_name,
        )?;
        let actual = sha256_file(&archive)?;
        if actual != expected {
            return Err(SandboxError::Invalid(format!(
                "Hubu artifact checksum mismatch: expected {expected}, got {actual}"
            )));
        }
        run(
            Command::new("tar")
                .args(["-xzf"])
                .arg(&archive)
                .arg("-C")
                .arg(staging.path()),
            "extract pinned Hubu release",
        )?;
        let extracted = staging
            .path()
            .join(format!("hubu-{}-{target}", config.version));
        if !extracted.is_dir() {
            return Err(SandboxError::Invalid(
                "Hubu release archive did not contain the expected directory".into(),
            ));
        }
        fs::write(extracted.join("ARTIFACT_SHA256"), format!("{actual}\n"))?;
        fs::write(
            extracted.join("HUBU_SERVER_SHA256"),
            format!("{}\n", sha256_file(&extracted.join("hubu-server"))?),
        )?;
        fs::write(
            extracted.join("HUBU_CLI_SHA256"),
            format!("{}\n", sha256_file(&extracted.join("hubu"))?),
        )?;
        fs::create_dir_all(install.parent().expect("version cache parent"))?;
        match fs::rename(&extracted, &install) {
            Ok(()) => {}
            Err(error) if install.exists() => {
                let _ = error;
            }
            Err(error) => return Err(error.into()),
        }
        validate_install(config, &target, &install)
    }
}

fn validate_install(
    config: &HubuReleaseConfig,
    target: &str,
    install: &Path,
) -> Result<HubuRelease, SandboxError> {
    let provenance_path = install.join("PROVENANCE.json");
    let provenance: HubuProvenance = serde_json::from_slice(&fs::read(&provenance_path)?)?;
    if provenance.product_version != config.version
        || provenance.target != target
        || provenance.repository != config.repository
        || provenance.executor_contract != EXECUTOR_CONTRACT
        || provenance.source_commit.len() != 40
        || !provenance
            .source_commit
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(SandboxError::Invalid(
            "cached Hubu release provenance does not match the requested immutable release".into(),
        ));
    }
    let checksum = fs::read_to_string(install.join("ARTIFACT_SHA256"))?
        .trim()
        .to_owned();
    if checksum.len() != 64 || !checksum.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(SandboxError::Invalid(
            "cached Hubu release artifact checksum is invalid".into(),
        ));
    }
    let server_binary = install.join("hubu-server");
    let cli_binary = install.join("hubu");
    if !server_binary.is_file() || !cli_binary.is_file() {
        return Err(SandboxError::Invalid(
            "cached Hubu release is missing hubu-server or hubu".into(),
        ));
    }
    validate_cached_binary(&server_binary, &install.join("HUBU_SERVER_SHA256"))?;
    validate_cached_binary(&cli_binary, &install.join("HUBU_CLI_SHA256"))?;
    Ok(HubuRelease {
        version: provenance.product_version,
        source_commit: provenance.source_commit,
        executor_contract: provenance.executor_contract,
        target: provenance.target,
        artifact_checksum: format!("sha256:{checksum}"),
        server_binary,
        cli_binary,
        provenance_path,
    })
}

fn validate_cached_binary(binary: &Path, digest_file: &Path) -> Result<(), SandboxError> {
    let expected = fs::read_to_string(digest_file)?.trim().to_owned();
    let actual = sha256_file(binary)?;
    if expected.len() != 64
        || !expected.bytes().all(|byte| byte.is_ascii_hexdigit())
        || expected != actual
    {
        return Err(SandboxError::Invalid(format!(
            "cached Hubu binary checksum mismatch for {}",
            binary.display()
        )));
    }
    Ok(())
}

fn validate_version(version: &str) -> Result<(), SandboxError> {
    let Some(rest) = version.strip_prefix('v') else {
        return Err(SandboxError::Invalid(
            "Hubu release version must be an exact vMAJOR.MINOR.PATCH tag".into(),
        ));
    };
    let core = rest.split_once('-').map(|(core, _)| core).unwrap_or(rest);
    let parts = core.split('.').collect::<Vec<_>>();
    if parts.len() != 3
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
        || !rest
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    {
        return Err(SandboxError::Invalid(
            "Hubu release version must be an exact vMAJOR.MINOR.PATCH tag".into(),
        ));
    }
    Ok(())
}

fn safe_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn platform_target() -> Result<String, SandboxError> {
    let arch = match env::consts::ARCH {
        "aarch64" => "aarch64",
        "x86_64" => "x86_64",
        other => {
            return Err(SandboxError::Invalid(format!(
                "Hubu releases do not support architecture {other}"
            )))
        }
    };
    let os = match env::consts::OS {
        "macos" => "apple-darwin",
        "linux" => "unknown-linux-gnu",
        other => {
            return Err(SandboxError::Invalid(format!(
                "Hubu releases do not support operating system {other}"
            )))
        }
    };
    Ok(format!("{arch}-{os}"))
}

fn default_cache_dir() -> PathBuf {
    if let Some(path) = env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(path).join("gongbu/hubu");
    }
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
        .join(".cache/gongbu/hubu")
}

fn checksum_for(contents: &str, artifact: &str) -> Result<String, SandboxError> {
    contents
        .lines()
        .filter_map(|line| line.split_once(char::is_whitespace))
        .find_map(|(checksum, name)| {
            (name.trim_start_matches('*').trim() == artifact).then(|| checksum.to_owned())
        })
        .ok_or_else(|| SandboxError::Invalid(format!("SHA256SUMS does not contain {artifact}")))
}

fn sha256_file(path: &Path) -> Result<String, SandboxError> {
    let bytes = fs::read(path)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn run(command: &mut Command, action: &str) -> Result<(), SandboxError> {
    let output = command.output()?;
    if output.status.success() {
        return Ok(());
    }
    Err(SandboxError::Invalid(format!(
        "could not {action}: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_exact_and_mutable_aliases_are_rejected() {
        assert!(validate_version("v0.1.0").is_ok());
        assert!(validate_version("v0.2.0-rc.1").is_ok());
        for value in ["latest", "main", "0.1.0", "v0.1", "v0.1.0/../../x"] {
            assert!(validate_version(value).is_err(), "{value}");
        }
    }

    #[test]
    fn checksum_manifest_selects_the_exact_artifact() {
        let sums = "aaaa  other.tar.gz\nbbbb  hubu-v0.1.0-aarch64-apple-darwin.tar.gz\n";
        assert_eq!(
            checksum_for(sums, "hubu-v0.1.0-aarch64-apple-darwin.tar.gz").unwrap(),
            "bbbb"
        );
    }

    #[test]
    fn corrupted_cached_binary_is_rejected_without_replacement() {
        let cache = tempfile::tempdir().unwrap();
        let config = HubuReleaseConfig {
            version: "v0.1.0".into(),
            repository: "hacker-no-ice/hubu".into(),
            cache_dir: Some(cache.path().to_path_buf()),
        };
        let install = cache.path().join("install");
        fs::create_dir(&install).unwrap();
        fs::write(install.join("hubu-server"), b"server").unwrap();
        fs::write(install.join("hubu"), b"cli").unwrap();
        fs::write(install.join("ARTIFACT_SHA256"), "a".repeat(64)).unwrap();
        fs::write(install.join("HUBU_SERVER_SHA256"), "b".repeat(64)).unwrap();
        fs::write(
            install.join("HUBU_CLI_SHA256"),
            format!("{}\n", sha256_file(&install.join("hubu")).unwrap()),
        )
        .unwrap();
        fs::write(
            install.join("PROVENANCE.json"),
            serde_json::to_vec(&HubuProvenance {
                product_version: "v0.1.0".into(),
                source_commit: "1".repeat(40),
                executor_contract: EXECUTOR_CONTRACT.into(),
                target: "aarch64-apple-darwin".into(),
                repository: "hacker-no-ice/hubu".into(),
                workflow_run: "test".into(),
                dependencies: "Cargo.lock".into(),
            })
            .unwrap(),
        )
        .unwrap();

        let error = validate_install(&config, "aarch64-apple-darwin", &install).unwrap_err();
        assert!(error.to_string().contains("binary checksum mismatch"));
        assert_eq!(fs::read(install.join("hubu-server")).unwrap(), b"server");
    }
}
