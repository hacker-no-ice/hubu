use std::{
    env,
    fs::{File, OpenOptions},
    io::Write,
    path::Path,
    sync::{Mutex, OnceLock},
};

use chrono::Utc;
use serde_json::{json, Value};

static LOG_FILE: OnceLock<Mutex<File>> = OnceLock::new();

pub fn configure_file_logging(path: impl AsRef<Path>) -> std::io::Result<()> {
    if LOG_FILE.get().is_some() {
        return Ok(());
    }

    let file = OpenOptions::new().create(true).append(true).open(path)?;
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
                if let Err(error) = writeln!(file, "{line}") {
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
