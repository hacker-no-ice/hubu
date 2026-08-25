//! Narrow credential bootstrap used only by the launcher-managed local stack.

use crate::{
    hubu::HubuClient,
    secrets::{MANAGED_CALLER_ACCOUNT, MANAGED_HUBU_ACCOUNT, MANAGED_STACK_SERVICE},
    server::{SecretReferenceConfig, ServerConfig, ServerError},
};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Component, Path},
};
use uuid::Uuid;

struct SecretBytes(Vec<u8>);

impl SecretBytes {
    fn read(path: &Path) -> Result<Self, ServerError> {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(path).map_err(|_| credential_error())?;
        validate_private_file(&file).map_err(|_| credential_error())?;
        let mut bytes = Vec::new();
        file.take(4097)
            .read_to_end(&mut bytes)
            .map_err(|_| credential_error())?;
        if bytes.len() > 4096 {
            bytes.fill(0);
            return Err(credential_error());
        }
        while bytes
            .last()
            .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
        {
            bytes.pop();
        }
        if bytes.is_empty()
            || bytes
                .iter()
                .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
        {
            bytes.fill(0);
            return Err(credential_error());
        }
        Ok(Self(bytes))
    }

    fn generated_caller() -> Self {
        Self(format!("gongbu_caller_{}", Uuid::new_v4().simple()).into_bytes())
    }

    fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for SecretBytes {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

pub fn bootstrap_managed(
    config_path: &Path,
    hubu_token_file: &Path,
    caller_token_file: &Path,
    secret_dir: &Path,
) -> Result<&'static str, ServerError> {
    let config = ServerConfig::from_path(config_path)?;
    config.validate()?;
    require_reference(&config.hubu.credential_reference, MANAGED_HUBU_ACCOUNT)?;
    require_reference(
        &config.authentication.bearer_credential_reference,
        MANAGED_CALLER_ACCOUNT,
    )?;
    validate_safe_absolute(hubu_token_file)?;
    validate_safe_absolute(caller_token_file)?;
    validate_safe_absolute(secret_dir)?;
    prepare_private_directory(secret_dir)?;
    let expected_caller = secret_dir.join("caller");
    if caller_token_file != expected_caller {
        return Err(ServerError::Invalid(
            "managed caller capability does not match its private stack destination".into(),
        ));
    }

    let hubu = SecretBytes::read(hubu_token_file)?;
    HubuClient::new(&config.hubu.endpoint)
        .with_bearer_token(hubu.expose().to_vec())
        .check_credential()
        .map_err(|_| {
            ServerError::Invalid(
                "managed Hubu credential was rejected by its protected endpoint".into(),
            )
        })?;

    persist_refresh(&secret_dir.join("hubu-executor"), hubu.expose())?;
    let caller = if caller_token_file.exists() {
        let caller = SecretBytes::read(caller_token_file)?;
        validate_managed_caller(&caller)?;
        caller
    } else {
        SecretBytes::generated_caller()
    };
    if caller.expose() == hubu.expose() {
        return Err(ServerError::Invalid(
            "managed credential classes must use distinct material".into(),
        ));
    }
    persist_exact(caller_token_file, caller.expose())?;
    Ok("managed Gongbu credential handoff ready")
}

fn require_reference(reference: &SecretReferenceConfig, account: &str) -> Result<(), ServerError> {
    if reference.service != MANAGED_STACK_SERVICE || reference.account != account {
        return Err(ServerError::Invalid(
            "managed credential handoff requires launcher-owned opaque references".into(),
        ));
    }
    Ok(())
}

fn validate_safe_absolute(path: &Path) -> Result<(), ServerError> {
    if !path.is_absolute()
        || path == Path::new("/")
        || path
            .components()
            .any(|component| component == Component::ParentDir)
    {
        return Err(ServerError::Invalid(
            "managed credential paths must be safe and absolute".into(),
        ));
    }
    Ok(())
}

fn prepare_private_directory(path: &Path) -> Result<(), ServerError> {
    let created = !path.exists();
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    if created {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() {
        return Err(credential_error());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(credential_error());
        }
    }
    Ok(())
}

fn persist_exact(path: &Path, value: &[u8]) -> Result<(), ServerError> {
    let temporary = write_staged_private(path, value)?;
    let result = match fs::hard_link(&temporary, path) {
        Ok(()) => {
            sync_parent(path);
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing = SecretBytes::read(path)?;
            if existing.expose() == value {
                Ok(())
            } else {
                Err(ServerError::Invalid(
                    "managed credential state conflicts with the active stack".into(),
                ))
            }
        }
        Err(_) => Err(credential_error()),
    };
    let _ = fs::remove_file(temporary);
    result
}

fn persist_refresh(path: &Path, value: &[u8]) -> Result<(), ServerError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_file() {
                return Err(credential_error());
            }
            let existing = SecretBytes::read(path)?;
            if existing.expose() == value {
                return Ok(());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return persist_exact(path, value);
        }
        Err(_) => return Err(credential_error()),
    }
    let temporary = write_staged_private(path, value)?;
    let result = fs::rename(&temporary, path).map_err(|_| credential_error());
    if result.is_ok() {
        sync_parent(path);
    }
    let _ = fs::remove_file(temporary);
    result
}

fn write_staged_private(path: &Path, value: &[u8]) -> Result<std::path::PathBuf, ServerError> {
    let parent = path.parent().ok_or_else(credential_error)?;
    let leaf = path.file_name().ok_or_else(credential_error)?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        leaf.to_string_lossy(),
        Uuid::new_v4().simple()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let result = (|| {
        let mut file = options.open(&temporary).map_err(|_| credential_error())?;
        file.write_all(value).map_err(|_| credential_error())?;
        file.write_all(b"\n").map_err(|_| credential_error())?;
        file.sync_all().map_err(|_| credential_error())?;
        Ok(temporary.clone())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn sync_parent(path: &Path) {
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        if let Ok(directory) = fs::File::open(parent) {
            let _ = directory.sync_all();
        }
    }
}

fn validate_managed_caller(caller: &SecretBytes) -> Result<(), ServerError> {
    let value = std::str::from_utf8(caller.expose()).map_err(|_| credential_error())?;
    let suffix = value
        .strip_prefix("gongbu_caller_")
        .ok_or_else(credential_error)?;
    if suffix.len() != 32
        || !suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
        || Uuid::parse_str(suffix).is_err()
    {
        return Err(ServerError::Invalid(
            "managed caller credential is incomplete or invalid".into(),
        ));
    }
    Ok(())
}

fn validate_private_file(file: &fs::File) -> std::io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "credential file is not private",
            ));
        }
    }
    Ok(())
}

fn credential_error() -> ServerError {
    ServerError::Invalid("managed credential state is unavailable or unsafe".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[cfg(unix)]
    #[test]
    fn private_files_are_idempotent_and_conflicts_fail_closed() {
        let root = tempdir().unwrap();
        let directory = root.path().join("credentials");
        prepare_private_directory(&directory).unwrap();
        let path = directory.join("caller");
        persist_exact(&path, b"caller-one").unwrap();
        persist_exact(&path, b"caller-one").unwrap();
        assert!(persist_exact(&path, b"caller-two").is_err());
        persist_refresh(&path, b"caller-two").unwrap();
        assert_eq!(SecretBytes::read(&path).unwrap().expose(), b"caller-two");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(fs::read_dir(&directory).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));

        let partial = SecretBytes(b"gongbu_caller_".to_vec());
        assert!(validate_managed_caller(&partial).is_err());
        let complete = SecretBytes::generated_caller();
        validate_managed_caller(&complete).unwrap();
    }
}
