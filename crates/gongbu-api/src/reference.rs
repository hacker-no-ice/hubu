//! Operator-allowed reference images: local files an edit may send by name.
//!
//! A reference image is never copied into the artifact store, which holds
//! only governed outputs. The operator names one folder; an edit names a file
//! in it together with the SHA-256 it expects, as
//! `reference:<name>@sha256:<hex>`. The digest is part of the approved,
//! hashed input, and the file is read and checked against it again before
//! anything is sent, so a file that changed after approval is never sent.
use crate::{
    artifact::{
        hex_sha256, sniff_image_media_type, validate_image, AccountInputImages, ArtifactLimits,
    },
    provider::contract::{ContractError, InputImage, InputImageSource, INPUT_ARTIFACT_UNAVAILABLE},
};
use serde::Serialize;
use std::{
    fs, io,
    io::Read,
    path::{Component, Path, PathBuf},
};

/// The prefix that marks a reference image in an edit's input images.
pub const REFERENCE_PREFIX: &str = "reference:";
const DIGEST_SEPARATOR: &str = "@sha256:";
/// Bounds on what a listing walks, so a large folder can't stall a request.
const MAX_LISTED_IMAGES: usize = 500;
const MAX_LISTING_DEPTH: usize = 4;
const MAX_NAME_BYTES: usize = 512;

/// A reference image an edit names: a path relative to the reference folder
/// and the SHA-256 of the bytes it expects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReferenceToken {
    pub name: String,
    pub sha256: String,
}

impl ReferenceToken {
    /// Parse `reference:<name>@sha256:<64 lowercase hex>`.
    pub fn parse(value: &str) -> Option<Self> {
        let rest = value.strip_prefix(REFERENCE_PREFIX)?;
        let (name, sha256) = rest.rsplit_once(DIGEST_SEPARATOR)?;
        (valid_name(name)
            && sha256.len() == 64
            && sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        .then(|| Self {
            name: name.to_owned(),
            sha256: sha256.to_owned(),
        })
    }

    pub fn to_input(&self) -> String {
        format!(
            "{REFERENCE_PREFIX}{}{DIGEST_SEPARATOR}{}",
            self.name, self.sha256
        )
    }
}

/// A reference name: a relative path of ordinary, non-hidden components.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_BYTES
        && !name.contains(['\\', '\0'])
        && !name.starts_with('/')
        && name.split('/').all(|part| {
            !part.is_empty() && !part.starts_with('.') && !part.chars().any(char::is_control)
        })
        && Path::new(name)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

/// One image a listing shows, with the token an edit sends to use it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReferenceImage {
    pub name: String,
    pub reference: String,
    pub sha256: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub width: u32,
    pub height: u32,
}

/// The operator's reference folder.
#[derive(Clone, Debug)]
pub struct ReferenceLibrary {
    root: PathBuf,
    limits: ArtifactLimits,
}

impl ReferenceLibrary {
    /// Open the folder at `root`, which must exist.
    pub fn open(root: impl AsRef<Path>, limits: ArtifactLimits) -> io::Result<Self> {
        let root = fs::canonicalize(root)?;
        if !root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "reference image root is not a directory",
            ));
        }
        Ok(Self { root, limits })
    }

    /// Every valid image in the folder, by name. Hidden entries, links that
    /// leave the folder, and files that aren't valid images are left out.
    pub fn list(&self) -> Vec<ReferenceImage> {
        let mut images = Vec::new();
        let mut pending = vec![(self.root.clone(), String::new(), 0)];
        while let Some((dir, prefix, depth)) = pending.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                let Some(file_name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                let name = format!("{prefix}{file_name}");
                if !valid_name(&name) {
                    continue;
                }
                let Some(path) = self.contained(&name) else {
                    continue;
                };
                if path.is_dir() {
                    if depth + 1 < MAX_LISTING_DEPTH {
                        pending.push((path, format!("{name}/"), depth + 1));
                    }
                } else if let Ok(image) = self.inspect(&name, &path) {
                    images.push(image);
                    if images.len() >= MAX_LISTED_IMAGES {
                        images.sort_by(|a, b| a.name.cmp(&b.name));
                        return images;
                    }
                }
            }
        }
        images.sort_by(|a, b| a.name.cmp(&b.name));
        images
    }

    /// Load the image `token` names, only if its bytes still have the pinned
    /// digest and are a valid image.
    pub fn load(&self, token: &ReferenceToken) -> Result<InputImage, ContractError> {
        let unavailable = || ContractError::Provider {
            code: INPUT_ARTIFACT_UNAVAILABLE.into(),
        };
        let path = self.contained(&token.name).ok_or_else(unavailable)?;
        let bytes = self.read(&path).map_err(|_| unavailable())?;
        if hex_sha256(&bytes) != token.sha256 {
            return Err(unavailable());
        }
        let media_type = sniff_image_media_type(&bytes).ok_or_else(unavailable)?;
        validate_image(&bytes, media_type, &self.limits).map_err(|_| unavailable())?;
        Ok(InputImage {
            media_type: media_type.to_owned(),
            bytes,
        })
    }

    /// The path `name` resolves to, only if it stays inside the folder once
    /// every link is followed.
    fn contained(&self, name: &str) -> Option<PathBuf> {
        if !valid_name(name) {
            return None;
        }
        let path = fs::canonicalize(self.root.join(name)).ok()?;
        (path.starts_with(&self.root) && path != self.root).then_some(path)
    }

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        let file = fs::File::open(path)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a file"));
        }
        let mut bytes = Vec::new();
        file.take(self.limits.max_encoded_bytes + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > self.limits.max_encoded_bytes {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "too large"));
        }
        Ok(bytes)
    }

    fn inspect(&self, name: &str, path: &Path) -> Result<ReferenceImage, ()> {
        let bytes = self.read(path).map_err(drop)?;
        let media_type = sniff_image_media_type(&bytes).ok_or(())?;
        let (width, height) = validate_image(&bytes, media_type, &self.limits).map_err(drop)?;
        let sha256 = hex_sha256(&bytes);
        let token = ReferenceToken {
            name: name.to_owned(),
            sha256: sha256.clone(),
        };
        Ok(ReferenceImage {
            name: name.to_owned(),
            reference: token.to_input(),
            sha256,
            media_type: media_type.to_owned(),
            size_bytes: bytes.len() as u64,
            width,
            height,
        })
    }
}

/// The input images an edit may send: this account's governed artifacts by
/// ID, and the operator's reference images by pinned token.
pub struct EditInputImages<'a> {
    pub artifacts: AccountInputImages<'a>,
    pub references: Option<&'a ReferenceLibrary>,
}

impl InputImageSource for EditInputImages<'_> {
    fn load(&self, id: &str) -> Result<InputImage, ContractError> {
        if id.starts_with(REFERENCE_PREFIX) {
            let token = ReferenceToken::parse(id).ok_or_else(|| ContractError::Provider {
                code: INPUT_ARTIFACT_UNAVAILABLE.into(),
            })?;
            return self
                .references
                .ok_or_else(|| ContractError::Provider {
                    code: INPUT_ARTIFACT_UNAVAILABLE.into(),
                })?
                .load(&token);
        }
        self.artifacts.load(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageOutputFormat, RgbaImage};
    use std::io::Cursor;
    use tempfile::tempdir;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        DynamicImage::ImageRgba8(RgbaImage::new(width, height))
            .write_to(&mut Cursor::new(&mut bytes), ImageOutputFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn tokens_parse_only_safe_names_and_full_digests() {
        let digest = "a".repeat(64);
        let token =
            ReferenceToken::parse(&format!("reference:hubu mascot.png@sha256:{digest}")).unwrap();
        assert_eq!(token.name, "hubu mascot.png");
        assert_eq!(
            token.to_input(),
            format!("reference:hubu mascot.png@sha256:{digest}")
        );
        assert_eq!(
            ReferenceToken::parse(&format!("reference:seal/sheet@v2.png@sha256:{digest}"))
                .unwrap()
                .name,
            "seal/sheet@v2.png"
        );
        for bad in [
            format!("reference:../secret.png@sha256:{digest}"),
            format!("reference:/etc/x.png@sha256:{digest}"),
            format!("reference:.hidden.png@sha256:{digest}"),
            format!("reference:a//b.png@sha256:{digest}"),
            format!("reference:a\\b.png@sha256:{digest}"),
            format!("reference:@sha256:{digest}"),
            format!("reference:x.png@sha256:{}", "A".repeat(64)),
            format!("reference:x.png@sha256:{}", "a".repeat(63)),
            "reference:x.png".to_owned(),
            format!("x.png@sha256:{digest}"),
        ] {
            assert!(ReferenceToken::parse(&bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn listing_shows_valid_images_with_their_pinned_tokens() {
        let root = tempdir().unwrap();
        let mascot = png(3, 2);
        fs::write(root.path().join("hubu mascot.png"), &mascot).unwrap();
        fs::create_dir(root.path().join("seal")).unwrap();
        fs::write(root.path().join("seal/sheet.png"), png(1, 1)).unwrap();
        fs::write(root.path().join("notes.txt"), b"not an image").unwrap();
        fs::write(root.path().join(".hidden.png"), png(1, 1)).unwrap();
        let outside = tempdir().unwrap();
        fs::write(outside.path().join("private.png"), png(1, 1)).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            outside.path().join("private.png"),
            root.path().join("link.png"),
        )
        .unwrap();
        let library = ReferenceLibrary::open(root.path(), ArtifactLimits::default()).unwrap();
        let images = library.list();
        let names: Vec<_> = images.iter().map(|image| image.name.as_str()).collect();
        assert_eq!(names, ["hubu mascot.png", "seal/sheet.png"]);
        let first = &images[0];
        assert_eq!(first.sha256, hex_sha256(&mascot));
        assert_eq!((first.width, first.height), (3, 2));
        assert_eq!(first.media_type, "image/png");
        assert_eq!(
            first.reference,
            format!("reference:hubu mascot.png@sha256:{}", first.sha256)
        );
    }

    #[test]
    fn loading_checks_the_pinned_digest_and_never_leaves_the_folder() {
        let root = tempdir().unwrap();
        let mascot = png(2, 2);
        let path = root.path().join("hubu mascot.png");
        fs::write(&path, &mascot).unwrap();
        let library = ReferenceLibrary::open(root.path(), ArtifactLimits::default()).unwrap();
        let token = ReferenceToken::parse(&library.list()[0].reference).unwrap();
        let loaded = library.load(&token).unwrap();
        assert_eq!(
            (loaded.media_type.as_str(), loaded.bytes),
            ("image/png", mascot.clone())
        );

        // A file changed after approval is never sent.
        fs::write(&path, png(4, 4)).unwrap();
        assert!(library.load(&token).is_err());

        // Names that leave the folder, or don't exist, never load.
        let outside = tempdir().unwrap();
        let private = png(1, 1);
        fs::write(outside.path().join("private.png"), &private).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                outside.path().join("private.png"),
                root.path().join("link.png"),
            )
            .unwrap();
            let escaping = ReferenceToken {
                name: "link.png".into(),
                sha256: hex_sha256(&private),
            };
            assert!(library.load(&escaping).is_err());
        }
        let missing = ReferenceToken {
            name: "missing.png".into(),
            sha256: hex_sha256(&private),
        };
        assert!(library.load(&missing).is_err());

        // A file that isn't a valid image never loads, even with its digest.
        fs::write(root.path().join("fake.png"), b"\x89PNG\r\n\x1a\nnot really").unwrap();
        let fake = ReferenceToken {
            name: "fake.png".into(),
            sha256: hex_sha256(b"\x89PNG\r\n\x1a\nnot really"),
        };
        assert!(library.load(&fake).is_err());

        // An oversized file never loads.
        let small = ReferenceLibrary::open(
            root.path(),
            ArtifactLimits {
                max_encoded_bytes: 8,
                ..ArtifactLimits::default()
            },
        )
        .unwrap();
        fs::write(&path, &mascot).unwrap();
        assert!(small.load(&token).is_err());
        assert!(small.list().is_empty());
    }
}
