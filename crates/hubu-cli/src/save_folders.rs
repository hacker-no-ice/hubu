//! Human-managed allowlist of folders agents may save artifacts into.
//!
//! Agents name a folder; only the human maps names to filesystem paths here.
//! The unified MCP server reads the same file through
//! `HUBU_UNIFIED_SAVE_FOLDERS_FILE`, which `hubu init codex` and `hubu artifact
//! save` point at `$HUBU_HOME/save-folders.json`.
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub(crate) const FILE_NAME: &str = "save-folders.json";
pub(crate) const ENV: &str = "HUBU_UNIFIED_SAVE_FOLDERS_FILE";
const MAX_FOLDERS: usize = 32;

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
struct Allowlist {
    schema_version: u32,
    folders: Vec<Folder>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
struct Folder {
    name: String,
    path: PathBuf,
}

pub(crate) fn file_path(hubu_home: &Path) -> Result<PathBuf> {
    let home = if hubu_home.is_absolute() {
        hubu_home.to_path_buf()
    } else {
        std::env::current_dir()?.join(hubu_home)
    };
    Ok(home.join(FILE_NAME))
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_'))
}

fn read(path: &Path) -> Result<Allowlist> {
    match fs::read(path) {
        Ok(bytes) => {
            let allowlist: Allowlist = serde_json::from_slice(&bytes)
                .with_context(|| format!("parse save folders `{}`", path.display()))?;
            ensure!(
                allowlist.schema_version == 1,
                "unsupported save folders schema in `{}`",
                path.display()
            );
            Ok(allowlist)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Allowlist {
            schema_version: 1,
            folders: Vec::new(),
        }),
        Err(error) => Err(error).with_context(|| format!("read `{}`", path.display())),
    }
}

fn write(path: &Path, allowlist: &Allowlist) -> Result<()> {
    let parent = path.parent().context("invalid save folders path")?;
    fs::create_dir_all(parent).with_context(|| format!("create `{}`", parent.display()))?;
    let temporary = parent.join(format!(".{FILE_NAME}.{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(allowlist)?)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.with_context(|| format!("write `{}`", path.display()))
}

fn allow(path: &Path, name: &str, folder: &Path) -> Result<PathBuf> {
    ensure!(
        valid_name(name),
        "folder name must be 1-64 lowercase letters, digits, '-' or '_', starting with a letter or digit"
    );
    let canonical = fs::canonicalize(folder)
        .with_context(|| format!("folder `{}` must exist", folder.display()))?;
    ensure!(
        canonical.is_dir(),
        "`{}` is not a directory",
        canonical.display()
    );
    let mut allowlist = read(path)?;
    if let Some(existing) = allowlist.folders.iter_mut().find(|f| f.name == name) {
        existing.path.clone_from(&canonical);
    } else {
        ensure!(
            allowlist.folders.len() < MAX_FOLDERS,
            "at most {MAX_FOLDERS} save folders may be allowlisted"
        );
        allowlist.folders.push(Folder {
            name: name.into(),
            path: canonical.clone(),
        });
    }
    write(path, &allowlist)?;
    Ok(canonical)
}

fn remove(path: &Path, name: &str) -> Result<bool> {
    let mut allowlist = read(path)?;
    let before = allowlist.folders.len();
    allowlist.folders.retain(|folder| folder.name != name);
    if allowlist.folders.len() == before {
        return Ok(false);
    }
    write(path, &allowlist)?;
    Ok(true)
}

const USAGE: &str = "Usage:
  hubu folders allow NAME PATH   Allowlist an existing folder under a short name
  hubu folders list [--json]     Show allowlisted save folders
  hubu folders remove NAME       Stop allowing saves into a folder

Agents save governed artifacts only into these folders, by NAME
(hubu_save_artifact over MCP, or `hubu artifact save --folder NAME`). They never
choose a filesystem path. The allowlist is stored in $HUBU_HOME/save-folders.json.";

pub(crate) fn command(hubu_home: &Path, args: Vec<String>) -> Result<()> {
    let path = file_path(hubu_home)?;
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] | ["-h" | "--help" | "help"] => println!("{USAGE}"),
        ["allow", name, folder] => {
            let canonical = allow(&path, name, Path::new(folder))?;
            println!("Allowlisted save folder `{name}`: {}", canonical.display());
        }
        ["list"] => {
            let allowlist = read(&path)?;
            if allowlist.folders.is_empty() {
                println!(
                    "No save folders are allowlisted. Add one with `hubu folders allow NAME PATH`."
                );
            }
            for folder in allowlist.folders {
                println!("{}\t{}", folder.name, folder.path.display());
            }
        }
        ["list", "--json"] => println!("{}", serde_json::to_string_pretty(&read(&path)?)?),
        ["remove", name] => {
            if remove(&path, name)? {
                println!("Removed save folder `{name}`");
            } else {
                bail!("save folder `{name}` is not allowlisted");
            }
        }
        _ => bail!("invalid folders command\n\n{USAGE}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_canonicalizes_replaces_and_removes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("home").join(FILE_NAME);
        let brand = root.path().join("brand");
        fs::create_dir(&brand).unwrap();
        let canonical = allow(&path, "brand", &brand.join("..").join("brand")).unwrap();
        assert_eq!(canonical, fs::canonicalize(&brand).unwrap());
        allow(&path, "brand", &brand).unwrap();
        let written: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(written["schema_version"], 1);
        assert_eq!(written["folders"].as_array().unwrap().len(), 1);
        assert!(allow(&path, "Brand", &brand).is_err());
        assert!(allow(&path, "../x", &brand).is_err());
        assert!(allow(&path, "missing", &root.path().join("missing")).is_err());
        assert!(remove(&path, "brand").unwrap());
        assert!(!remove(&path, "brand").unwrap());
        assert!(read(&path).unwrap().folders.is_empty());
    }
}
