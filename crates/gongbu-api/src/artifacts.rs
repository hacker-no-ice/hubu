//! Gongbu-owned artifact normalization and storage.
use crate::persistence::{Artifact, CreateArtifactParams, Repository};
use image::ImageFormat;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Cursor, Write},
    path::{Component, Path, PathBuf},
};
use thiserror::Error;
use uuid::Uuid;

pub const LOCAL_FS_BACKEND: &str = "local_fs";

#[derive(Clone, Debug)]
pub struct ArtifactLimits {
    pub max_artifacts_per_execution: u64,
    pub max_encoded_bytes: u64,
    pub max_decoded_bytes: u64,
    pub max_width: u32,
    pub max_height: u32,
}

impl Default for ArtifactLimits {
    fn default() -> Self {
        Self {
            max_artifacts_per_execution: 16,
            max_encoded_bytes: 20 * 1024 * 1024,
            max_decoded_bytes: 100 * 1024 * 1024,
            max_width: 16_384,
            max_height: 16_384,
        }
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("artifact storage: {0}")]
    Io(#[from] io::Error),
    #[error("artifact persistence: {0}")]
    Persistence(#[from] crate::persistence::Error),
    #[error("unsupported artifact media type")]
    UnsupportedMediaType,
    #[error("artifact media type does not match its content")]
    MediaTypeMismatch,
    #[error("artifact limit exceeded: {0}")]
    Limit(&'static str),
    #[error("invalid storage key")]
    InvalidStorageKey,
    #[error("invalid image content")]
    InvalidImage,
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug)]
pub struct LocalFsStorage {
    root: PathBuf,
}

impl LocalFsStorage {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Must be called before provider invocation crosses its irreversible boundary.
    pub fn preflight(&self) -> Result<()> {
        fs::create_dir_all(&self.root)?;
        let probe = self
            .root
            .join(format!(".gongbu-preflight-{}", Uuid::new_v4()));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&probe)?;
            file.write_all(b"gongbu")?;
            file.sync_all()?;
            Ok(())
        })();
        let _ = fs::remove_file(&probe);
        result
    }

    fn path_for(&self, key: &str) -> Result<PathBuf> {
        validate_storage_key(key)?;
        Ok(self.root.join(key))
    }

    fn read(&self, key: &str) -> Result<Vec<u8>> {
        Ok(fs::read(self.path_for(key)?)?)
    }

    fn remove(&self, key: &str) {
        if let Ok(path) = self.path_for(key) {
            let _ = fs::remove_file(path);
        }
    }

    fn write_atomic(&self, key: &str, bytes: &[u8]) -> Result<()> {
        self.write_atomic_using(key, |file| file.write_all(bytes))
    }

    fn write_atomic_using<F>(&self, key: &str, write: F) -> Result<()>
    where
        F: FnOnce(&mut File) -> io::Result<()>,
    {
        let final_path = self.path_for(key)?;
        let parent = final_path.parent().ok_or(Error::InvalidStorageKey)?;
        fs::create_dir_all(parent)?;
        let temp_path = parent.join(format!(".{}.tmp", Uuid::new_v4()));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp_path)?;
            write(&mut file)?;
            file.sync_all()?;
            fs::rename(&temp_path, &final_path)?;
            File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        result
    }
}

#[derive(Clone)]
pub struct ArtifactService {
    repository: Repository,
    storage: LocalFsStorage,
    limits: ArtifactLimits,
}

#[derive(Debug, PartialEq)]
pub struct RetrievedArtifact {
    pub artifact: Artifact,
    pub bytes: Vec<u8>,
}

impl ArtifactService {
    pub fn new(repository: Repository, storage: LocalFsStorage, limits: ArtifactLimits) -> Self {
        Self {
            repository,
            storage,
            limits,
        }
    }

    pub fn preflight(&self) -> Result<()> {
        self.storage.preflight()
    }

    pub fn store_image(
        &self,
        execution_id: &str,
        attempt_id: Option<&str>,
        media_type: &str,
        bytes: &[u8],
        created_at: &str,
    ) -> Result<Artifact> {
        if self
            .repository
            .count_artifacts_for_execution(execution_id)?
            >= self.limits.max_artifacts_per_execution
        {
            return Err(Error::Limit("artifact count"));
        }
        if bytes.len() as u64 > self.limits.max_encoded_bytes {
            return Err(Error::Limit("encoded size"));
        }
        let (format, extension) = match media_type {
            "image/png" => (ImageFormat::Png, "png"),
            "image/jpeg" => (ImageFormat::Jpeg, "jpg"),
            _ => return Err(Error::UnsupportedMediaType),
        };
        if image::guess_format(bytes).ok() != Some(format) {
            return Err(Error::MediaTypeMismatch);
        }
        let (width, height) = image::io::Reader::with_format(Cursor::new(bytes), format)
            .into_dimensions()
            .map_err(|_| Error::InvalidImage)?;
        if width > self.limits.max_width || height > self.limits.max_height {
            return Err(Error::Limit("dimensions"));
        }
        let decoded_size = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(Error::Limit("decoded size"))?;
        if decoded_size > self.limits.max_decoded_bytes {
            return Err(Error::Limit("decoded size"));
        }
        let mut decoder = image::io::Reader::with_format(Cursor::new(bytes), format);
        let mut decoder_limits = image::io::Limits::default();
        decoder_limits.max_image_width = Some(self.limits.max_width);
        decoder_limits.max_image_height = Some(self.limits.max_height);
        decoder_limits.max_alloc = Some(self.limits.max_decoded_bytes);
        decoder.limits(decoder_limits);
        decoder.decode().map_err(|_| Error::InvalidImage)?;

        let artifact_id = Uuid::new_v4().to_string();
        let storage_key = format!("executions/{execution_id}/{artifact_id}.{extension}");
        validate_storage_key(&storage_key)?;
        self.storage.write_atomic(&storage_key, bytes)?;

        let persisted = match self.storage.read(&storage_key) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.storage.remove(&storage_key);
                return Err(error);
            }
        };
        let size_bytes = persisted.len() as i64;
        let sha256 = hex_sha256(&persisted);
        if persisted != bytes {
            self.storage.remove(&storage_key);
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "persisted artifact verification failed",
            )));
        }
        let params = CreateArtifactParams {
            artifact_id,
            execution_id: execution_id.to_owned(),
            provider_attempt_id: attempt_id.map(str::to_owned),
            kind: "image".into(),
            storage_backend: LOCAL_FS_BACKEND.into(),
            storage_key: storage_key.clone(),
            media_type: media_type.into(),
            size_bytes,
            sha256,
            metadata: json!({"height": height, "width": width}),
            metadata_schema_version: 1,
            created_at: created_at.into(),
        };
        match self
            .repository
            .create_artifact_with_limit(&params, self.limits.max_artifacts_per_execution)
        {
            Ok(artifact) => Ok(artifact),
            Err(crate::persistence::Error::Limit("artifact count")) => {
                self.storage.remove(&storage_key);
                Err(Error::Limit("artifact count"))
            }
            Err(error) => {
                self.storage.remove(&storage_key);
                Err(error.into())
            }
        }
    }

    pub fn list_for_account(&self, execution_id: &str, account_id: &str) -> Result<Vec<Artifact>> {
        Ok(self
            .repository
            .list_artifacts_for_account(execution_id, account_id)?)
    }

    pub fn retrieve_for_account(
        &self,
        artifact_id: &str,
        account_id: &str,
    ) -> Result<RetrievedArtifact> {
        let artifact = self
            .repository
            .get_artifact_for_account(artifact_id, account_id)?;
        if artifact.storage_backend != LOCAL_FS_BACKEND {
            return Err(Error::InvalidStorageKey);
        }
        let bytes = self.storage.read(&artifact.storage_key)?;
        if bytes.len() as i64 != artifact.size_bytes || hex_sha256(&bytes) != artifact.sha256 {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "artifact metadata verification failed",
            )));
        }
        Ok(RetrievedArtifact { artifact, bytes })
    }

    pub fn receipt_reference(artifact: &Artifact) -> String {
        format!("artifact://{}", artifact.artifact_id)
    }
}

pub fn validate_storage_key(key: &str) -> Result<()> {
    let path = Path::new(key);
    if key.is_empty()
        || path.is_absolute()
        || key.contains('\\')
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || !key.starts_with("executions/")
    {
        return Err(Error::InvalidStorageKey);
    }
    Ok(())
}

fn hex_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{CreateExecutionParams, HubuTokenReference};
    use image::{DynamicImage, ImageOutputFormat, RgbaImage};
    use serde_json::json;
    use std::io::Cursor;
    use tempfile::tempdir;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            width,
            height,
            image::Rgba([1, 2, 3, 255]),
        ))
        .write_to(&mut Cursor::new(&mut bytes), ImageOutputFormat::Png)
        .unwrap();
        bytes
    }

    fn execution(repository: &Repository, account: &str, operation: &str) -> String {
        repository
            .create_execution(&CreateExecutionParams {
                account_id: account.into(),
                operation_key: operation.into(),
                hubu_authorization_id: "auth".into(),
                hubu_claim_id: Some("claim".into()),
                hubu_token_reference: HubuTokenReference::new("secret-ref").unwrap(),
                authorized_minor: 100,
                authorization_currency: "USD".into(),
                normalized_input: json!({}),
                input_hash: "hash".into(),
                input_schema_version: 1,
                target: "provider/model".into(),
                config_version: "v1".into(),
                pricing_snapshot: json!({}),
                pricing_schema_version: 1,
                created_at: "2026-08-05T20:00:00Z".into(),
            })
            .unwrap()
            .execution_id
    }

    fn service(root: &Path, repository: Repository, limits: ArtifactLimits) -> ArtifactService {
        ArtifactService::new(repository, LocalFsStorage::new(root), limits)
    }

    #[test]
    fn restart_retrieval_is_authorized_and_metadata_is_exact() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("gongbu.sqlite3");
        let root = directory.path().join("artifacts");
        let repository = Repository::open(&database).unwrap();
        let execution_id = execution(&repository, "owner", "restart");
        let bytes = png(2, 3);
        let first = service(&root, repository, ArtifactLimits::default());
        first.preflight().unwrap();
        let artifact = first
            .store_image(
                &execution_id,
                None,
                "image/png",
                &bytes,
                "2026-08-05T20:01:00Z",
            )
            .unwrap();
        assert_eq!(artifact.storage_backend, "local_fs");
        assert_eq!(
            artifact.storage_key,
            format!("executions/{execution_id}/{}.png", artifact.artifact_id)
        );
        assert!(!artifact
            .storage_key
            .contains(directory.path().to_str().unwrap()));
        assert_eq!(artifact.size_bytes, bytes.len() as i64);
        assert_eq!(artifact.sha256, hex_sha256(&bytes));
        assert_eq!(artifact.metadata, json!({"height": 3, "width": 2}));
        assert_eq!(
            ArtifactService::receipt_reference(&artifact),
            format!("artifact://{}", artifact.artifact_id)
        );

        let restarted = service(
            &root,
            Repository::open(&database).unwrap(),
            ArtifactLimits::default(),
        );
        assert_eq!(
            restarted
                .retrieve_for_account(&artifact.artifact_id, "owner")
                .unwrap()
                .bytes,
            bytes
        );
        assert!(matches!(
            restarted.retrieve_for_account(&artifact.artifact_id, "intruder"),
            Err(Error::Persistence(crate::persistence::Error::NotFound))
        ));
        assert_eq!(
            restarted
                .list_for_account(&execution_id, "owner")
                .unwrap()
                .len(),
            1
        );
        assert!(restarted
            .list_for_account(&execution_id, "intruder")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn rejects_media_mismatch_and_all_limits_without_final_files() {
        let directory = tempdir().unwrap();
        let repository = Repository::in_memory().unwrap();
        let execution_id = execution(&repository, "owner", "limits");
        let bytes = png(2, 3);
        let limits = ArtifactLimits {
            max_encoded_bytes: bytes.len() as u64 - 1,
            ..ArtifactLimits::default()
        };
        assert!(matches!(
            service(directory.path(), repository.clone(), limits).store_image(
                &execution_id,
                None,
                "image/png",
                &bytes,
                "now"
            ),
            Err(Error::Limit("encoded size"))
        ));
        assert!(matches!(
            service(
                directory.path(),
                repository.clone(),
                ArtifactLimits::default()
            )
            .store_image(&execution_id, None, "image/jpeg", &bytes, "now"),
            Err(Error::MediaTypeMismatch)
        ));
        assert!(matches!(
            service(
                directory.path(),
                repository.clone(),
                ArtifactLimits::default()
            )
            .store_image(&execution_id, None, "image/svg+xml", &bytes, "now"),
            Err(Error::UnsupportedMediaType)
        ));
        let dimensions = ArtifactLimits {
            max_width: 1,
            ..ArtifactLimits::default()
        };
        assert!(matches!(
            service(directory.path(), repository.clone(), dimensions).store_image(
                &execution_id,
                None,
                "image/png",
                &bytes,
                "now"
            ),
            Err(Error::Limit("dimensions"))
        ));
        let decoded = ArtifactLimits {
            max_decoded_bytes: 23,
            ..ArtifactLimits::default()
        };
        assert!(matches!(
            service(directory.path(), repository.clone(), decoded).store_image(
                &execution_id,
                None,
                "image/png",
                &bytes,
                "now"
            ),
            Err(Error::Limit("decoded size"))
        ));
        let count = ArtifactLimits {
            max_artifacts_per_execution: 0,
            ..ArtifactLimits::default()
        };
        assert!(matches!(
            service(directory.path(), repository, count).store_image(
                &execution_id,
                None,
                "image/png",
                &bytes,
                "now"
            ),
            Err(Error::Limit("artifact count"))
        ));
        assert!(!directory.path().join("executions").exists());
    }

    #[test]
    fn rejects_decoded_size_from_headers_before_full_decode() {
        let directory = tempdir().unwrap();
        let repository = Repository::in_memory().unwrap();
        let execution_id = execution(&repository, "owner", "header-limit");
        let mut corrupt = png(4, 4);
        let idat = corrupt
            .windows(4)
            .position(|window| window == b"IDAT")
            .unwrap();
        corrupt[idat + 4] ^= 0xff;
        let limits = ArtifactLimits {
            max_decoded_bytes: 63,
            ..ArtifactLimits::default()
        };
        let result = service(directory.path(), repository, limits).store_image(
            &execution_id,
            None,
            "image/png",
            &corrupt,
            "now",
        );
        assert!(
            matches!(result, Err(Error::Limit("decoded size"))),
            "unexpected result: {result:?}"
        );
    }

    #[test]
    fn traversal_and_caller_selected_paths_are_rejected() {
        for key in [
            "/tmp/output.png",
            "../output.png",
            "executions/../output.png",
            "executions\\output.png",
            "provider.png",
        ] {
            assert!(matches!(
                validate_storage_key(key),
                Err(Error::InvalidStorageKey)
            ));
        }
    }

    #[test]
    fn partial_writes_leave_no_temp_or_final_artifact() {
        let directory = tempdir().unwrap();
        let storage = LocalFsStorage::new(directory.path());
        let key = "executions/execution/artifact.png";
        let error = storage
            .write_atomic_using(key, |file| {
                file.write_all(b"partial")?;
                Err(io::Error::other("injected failure"))
            })
            .unwrap_err();
        assert!(matches!(error, Error::Io(_)));
        assert!(!directory.path().join(key).exists());
        assert!(fs::read_dir(directory.path().join("executions/execution"))
            .unwrap()
            .next()
            .is_none());
    }
}
