use std::{env, ffi::OsString, fs};

use crate::{BackendOwner, ConfigError};

pub(crate) fn from_env(
    owner: BackendOwner,
    token_env: &str,
    token_file_env: &str,
) -> Result<Option<String>, ConfigError> {
    resolve(owner, env::var(token_env).ok(), env::var_os(token_file_env))
}

fn resolve(
    owner: BackendOwner,
    direct_token: Option<String>,
    token_file: Option<OsString>,
) -> Result<Option<String>, ConfigError> {
    if let Some(token) = direct_token.filter(|value| !value.trim().is_empty()) {
        return Ok(Some(token));
    }
    let Some(path) = token_file.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let token = fs::read_to_string(path).map_err(|_| ConfigError::CredentialFile(owner))?;
    let token = token.trim();
    if token.is_empty() {
        return Err(ConfigError::CredentialFile(owner));
    }
    Ok(Some(token.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_file_is_trimmed_without_exposing_its_path_in_errors() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        fs::write(temp.path(), "secret-token\n").unwrap();
        assert_eq!(
            resolve(
                BackendOwner::Hubu,
                None,
                Some(temp.path().as_os_str().to_owned()),
            )
            .unwrap(),
            Some("secret-token".to_string())
        );

        fs::write(temp.path(), "\n").unwrap();
        let error = resolve(
            BackendOwner::Hubu,
            None,
            Some(temp.path().as_os_str().to_owned()),
        )
        .unwrap_err()
        .to_string();
        assert_eq!(
            error,
            "hubu backend credential file could not be read or was empty"
        );
        assert!(!error.contains(temp.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn direct_credential_takes_precedence_over_a_file() {
        assert_eq!(
            resolve(
                BackendOwner::Gongbu,
                Some("direct-token".to_string()),
                Some(OsString::from("/does/not/exist")),
            )
            .unwrap(),
            Some("direct-token".to_string())
        );
    }
}
