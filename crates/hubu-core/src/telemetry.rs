use std::{
    env,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use chrono::Utc;
use serde_json::{json, Value};

const LOG_FILE_MAX_BYTES: u64 = 10 * 1024 * 1024;
const LOG_FILE_RETAINED_GENERATIONS: usize = 4;

static LOG_FILE: OnceLock<Mutex<RotatingFile>> = OnceLock::new();

struct RotatingFile {
    path: PathBuf,
    file: Option<File>,
    size: u64,
    max_bytes: u64,
    retained_generations: usize,
}

impl RotatingFile {
    fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        Self::open_with_policy(path, LOG_FILE_MAX_BYTES, LOG_FILE_RETAINED_GENERATIONS)
    }

    fn open_with_policy(
        path: impl AsRef<Path>,
        max_bytes: u64,
        retained_generations: usize,
    ) -> std::io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = open_secure_append(&path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            path,
            file: Some(file),
            size,
            max_bytes,
            retained_generations,
        })
    }

    fn write_line(&mut self, line: &str) -> std::io::Result<()> {
        self.ensure_open()?;
        let line_bytes = u64::try_from(line.len().saturating_add(1)).unwrap_or(u64::MAX);
        if self.size > 0 && self.size.saturating_add(line_bytes) > self.max_bytes {
            self.rotate()?;
        }
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| std::io::Error::other("rotating log file is unavailable"))?;
        writeln!(file, "{line}")?;
        self.size = self.size.saturating_add(line_bytes);
        Ok(())
    }

    fn rotate(&mut self) -> std::io::Result<()> {
        drop(self.file.take());
        let rotation_result = self.rotate_files();
        self.ensure_open()?;
        rotation_result
    }

    fn ensure_open(&mut self) -> std::io::Result<()> {
        if self.file.is_some() {
            return Ok(());
        }
        let file = open_secure_append(&self.path)?;
        self.size = file.metadata()?.len();
        self.file = Some(file);
        Ok(())
    }

    fn rotate_files(&self) -> std::io::Result<()> {
        if self.retained_generations > 0 {
            for generation in (1..self.retained_generations).rev() {
                let source = rotated_path(&self.path, generation);
                if source.exists() {
                    if source.metadata()?.len() > self.max_bytes {
                        fs::remove_file(source)?;
                        continue;
                    }
                    let destination = rotated_path(&self.path, generation + 1);
                    if destination.exists() {
                        fs::remove_file(&destination)?;
                    }
                    fs::rename(source, destination)?;
                }
            }
            let first = rotated_path(&self.path, 1);
            if first.exists() {
                fs::remove_file(&first)?;
            }
            if self.path.exists() {
                if self.path.metadata()?.len() <= self.max_bytes {
                    fs::rename(&self.path, first)?;
                } else {
                    fs::remove_file(&self.path)?;
                }
            }
        } else if self.path.exists() {
            fs::remove_file(&self.path)?;
        }
        Ok(())
    }
}

fn open_secure_append(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn rotated_path(path: &Path, generation: usize) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(format!(".{generation}"));
    PathBuf::from(value)
}

pub fn configure_file_logging(path: impl AsRef<Path>) -> std::io::Result<()> {
    if LOG_FILE.get().is_some() {
        return Ok(());
    }

    let file = RotatingFile::open(path)?;
    let _ = LOG_FILE.set(Mutex::new(file));
    Ok(())
}

pub fn log_event(level: &str, event: &str, fields: Value) {
    let line = json!({
        "timestamp": Utc::now().to_rfc3339(),
        "level": level,
        "event": event,
        "fields": fields,
    })
    .to_string();

    if stderr_enabled() {
        eprintln!("{line}");
    }

    if let Some(file) = LOG_FILE.get() {
        match file.lock() {
            Ok(mut file) => {
                if let Err(error) = file.write_line(&line) {
                    eprintln!("failed to write Hubu log event: {error}");
                }
            }
            Err(_) => eprintln!("failed to write Hubu log event: log file lock poisoned"),
        }
    }
}

fn stderr_enabled() -> bool {
    !matches!(
        env::var("HUBU_LOG_STDERR").as_deref(),
        Ok("0") | Ok("false") | Ok("FALSE") | Ok("off") | Ok("OFF")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_before_crossing_limit_and_bounds_retention() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("hubu.jsonl");
        let mut log = RotatingFile::open_with_policy(&path, 11, 2).unwrap();

        for value in ["one", "two", "three", "four", "five"] {
            log.write_line(value).unwrap();
        }
        drop(log);

        assert_eq!(fs::read_to_string(&path).unwrap(), "five\n");
        assert_eq!(
            fs::read_to_string(rotated_path(&path, 1)).unwrap(),
            "three\nfour\n"
        );
        assert_eq!(
            fs::read_to_string(rotated_path(&path, 2)).unwrap(),
            "one\ntwo\n"
        );
        assert!(!rotated_path(&path, 3).exists());
    }

    #[cfg(unix)]
    #[test]
    fn reopened_active_log_preserves_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("hubu.jsonl");
        let mut log = RotatingFile::open_with_policy(&path, 4, 1).unwrap();

        log.write_line("one").unwrap();
        log.write_line("two").unwrap();
        drop(log);

        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(rotated_path(&path, 1))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn rotates_an_existing_full_log_on_the_next_write() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("hubu.jsonl");
        fs::write(&path, "12345678").unwrap();

        let mut log = RotatingFile::open_with_policy(&path, 8, 1).unwrap();
        log.write_line("new").unwrap();
        drop(log);

        assert_eq!(fs::read_to_string(&path).unwrap(), "new\n");
        assert_eq!(
            fs::read_to_string(rotated_path(&path, 1)).unwrap(),
            "12345678"
        );
    }

    #[test]
    fn discards_an_existing_oversized_log_to_enforce_the_total_bound() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("hubu.jsonl");
        fs::write(&path, "123456789").unwrap();

        let mut log = RotatingFile::open_with_policy(&path, 8, 1).unwrap();
        log.write_line("new").unwrap();
        drop(log);

        assert_eq!(fs::read_to_string(&path).unwrap(), "new\n");
        assert!(!rotated_path(&path, 1).exists());
    }
}
