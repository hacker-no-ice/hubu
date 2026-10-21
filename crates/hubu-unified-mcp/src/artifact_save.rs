//! Save one governed artifact into a human-allowlisted folder with a receipt.
//!
//! The agent names an allowlisted folder; it never supplies a filesystem path.
//! Folders are configured by the human (`hubu folders allow`) in the JSON file
//! named by `HUBU_UNIFIED_SAVE_FOLDERS_FILE`. Artifact bytes are fetched from
//! Gongbu by artifact ID and verified against the listed digest; the receipt
//! carries Hubu-settled accounting evidence and never a Gongbu storage path or
//! endpoint. All backend operations are bounded GETs.
use crate::{
    error_response,
    gallery::{
        artifact_bytes, atomic_write, history_record, ledger_rows, reject_symlink, safe_id, string,
        validate_accounting,
    },
    operation_registry, success_response, tool_availability, BackendClients, BackendOwner,
    CapabilitySnapshot, Config, Server, ToolCall,
};
use anyhow::{anyhow, bail, ensure, Result};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub(crate) const TOOL_NAME: &str = "hubu_save_artifact";
const FOLDERS_FILE_LIMIT: u64 = 64 * 1024;
const MAX_FOLDERS: usize = 32;
const COST_SEMANTICS: &str = "operation_total_not_per_artifact";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SaveFolder {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
struct Request {
    handle: String,
    artifact_id: Option<String>,
    folder: String,
    name: Option<String>,
}

fn valid_folder_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_'))
}

fn valid_file_stem(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && !name.contains("..")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// Reads the human-managed allowlist. A missing file is an empty allowlist.
pub(crate) fn load_folders(path: &Path) -> Result<Vec<SaveFolder>> {
    ensure!(path.is_absolute(), "save folders file must be absolute");
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => bail!("save folders file is unreadable"),
    };
    let mut bytes = Vec::new();
    file.take(FOLDERS_FILE_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow!("save folders file is unreadable"))?;
    ensure!(
        bytes.len() as u64 <= FOLDERS_FILE_LIMIT,
        "save folders file exceeds limit"
    );
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| anyhow!("invalid save folders file"))?;
    ensure!(
        value["schema_version"] == 1,
        "unsupported save folders schema"
    );
    let entries = value["folders"]
        .as_array()
        .filter(|entries| entries.len() <= MAX_FOLDERS)
        .ok_or_else(|| anyhow!("invalid save folders list"))?;
    let mut folders: Vec<SaveFolder> = Vec::new();
    for entry in entries {
        let name = string(entry, "name")?;
        let path = PathBuf::from(string(entry, "path")?);
        ensure!(valid_folder_name(name), "invalid save folder name");
        ensure!(path.is_absolute(), "save folder paths must be absolute");
        ensure!(
            folders.iter().all(|folder| folder.name != name),
            "duplicate save folder name"
        );
        folders.push(SaveFolder {
            name: name.into(),
            path,
        });
    }
    Ok(folders)
}

fn configured_folders(config_path: Option<&Path>) -> Result<Vec<SaveFolder>> {
    let path = config_path.ok_or_else(|| {
        anyhow!(
            "no save folders are allowlisted; ask the human to run `hubu folders allow NAME PATH`"
        )
    })?;
    load_folders(path)
}

fn request_from_arguments(arguments: &Value) -> Result<Request> {
    let arguments = arguments
        .as_object()
        .ok_or_else(|| anyhow!("Invalid params"))?;
    ensure!(
        arguments.keys().all(|key| matches!(
            key.as_str(),
            "operation_handle" | "artifact_id" | "folder" | "name"
        )),
        "Invalid params"
    );
    let required = |key: &str| {
        arguments
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("Invalid params"))
    };
    let optional = |key: &str| match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(anyhow!("Invalid params")),
    };
    let request = Request {
        handle: required("operation_handle")?,
        artifact_id: optional("artifact_id")?,
        folder: required("folder")?,
        name: optional("name")?,
    };
    validate_request(&request)?;
    Ok(request)
}

fn validate_request(request: &Request) -> Result<()> {
    ensure!(
        request.handle.starts_with("hubu:public-operation:v1:"),
        "operation_handle must be a public operation handle"
    );
    ensure!(
        request.artifact_id.as_deref().is_none_or(safe_id),
        "invalid artifact_id"
    );
    ensure!(
        valid_folder_name(&request.folder),
        "folder must be an allowlisted folder name, not a path"
    );
    ensure!(
        request.name.as_deref().is_none_or(valid_file_stem),
        "name must be a plain file name (letters, digits, '-', '_', '.')"
    );
    Ok(())
}

fn resolve_folder<'a>(folders: &'a [SaveFolder], name: &str) -> Result<&'a SaveFolder> {
    folders.iter().find(|folder| folder.name == name).ok_or_else(|| {
        if folders.is_empty() {
            anyhow!("no save folders are allowlisted; ask the human to run `hubu folders allow NAME PATH`")
        } else {
            let names: Vec<_> = folders.iter().map(|folder| folder.name.as_str()).collect();
            anyhow!(
                "folder `{name}` is not allowlisted; allowlisted folders: {}",
                names.join(", ")
            )
        }
    })
}

/// Native CLI entry: `artifact save --operation-handle H --folder NAME
/// [--artifact-id ID] [--name STEM]`.
pub fn run_artifact_from_env(arguments: Vec<String>) -> Result<Value> {
    const USAGE: &str = "usage: artifact save --operation-handle HANDLE --folder NAME [--artifact-id ID] [--name FILE_STEM]";
    ensure!(arguments.first().map(String::as_str) == Some("save"), USAGE);
    let mut values = Map::new();
    for pair in arguments[1..].chunks(2) {
        let key = match pair {
            [flag, _] => match flag.as_str() {
                "--operation-handle" => "operation_handle",
                "--folder" => "folder",
                "--artifact-id" => "artifact_id",
                "--name" => "name",
                _ => bail!("invalid artifact save option; {USAGE}"),
            },
            _ => bail!("invalid artifact save option; {USAGE}"),
        };
        ensure!(
            values.insert(key.into(), json!(pair[1])).is_none(),
            "duplicate artifact save option"
        );
    }
    let request = request_from_arguments(&Value::Object(values))
        .map_err(|error| anyhow!("{error}; {USAGE}"))?;
    let config = Config::from_env()?;
    let folders = configured_folders(config.save_folders_path.as_deref())?;
    let folder = resolve_folder(&folders, &request.folder)?.clone();
    let state = config
        .operation_state_path
        .as_ref()
        .ok_or_else(|| anyhow!("artifact save requires configured unified operation state"))?;
    let context = operation_registry::gallery_context(state, &request.handle)
        .map_err(|_| anyhow!("operation context unavailable or not succeeded"))?;
    let clients = BackendClients::new(config)?; // Clients only: no server, probes or workers.
    save(&clients, context, &folder, &request)
}

fn save(
    clients: &BackendClients,
    (authorization_id, execution): (String, String),
    folder: &SaveFolder,
    request: &Request,
) -> Result<Value> {
    ensure!(safe_id(&execution), "invalid bound execution identity");
    let metadata = fs::symlink_metadata(&folder.path)
        .map_err(|_| anyhow!("allowlisted folder `{}` does not exist", folder.name))?;
    ensure!(
        metadata.is_dir(),
        "allowlisted folder `{}` is not a directory",
        folder.name
    );
    let hubu = clients
        .hubu
        .as_ref()
        .ok_or_else(|| anyhow!("artifact save requires the Hubu backend"))?;
    let gongbu = clients
        .gongbu
        .as_ref()
        .ok_or_else(|| anyhow!("artifact save requires the Gongbu backend"))?;
    let record = history_record(hubu, &authorization_id)?;
    let provider = string(&record["provider"], "id")?.to_owned();
    let rows = ledger_rows(hubu, &record)?;
    let listing =
        crate::gallery::read_json(gongbu, &format!("v1/executions/{execution}/artifacts"), &[])?;
    ensure!(
        listing["schema_version"] == 1 && listing["execution_id"] == execution,
        "artifact list identity/schema mismatch"
    );
    let artifacts = listing["artifacts"]
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= 64)
        .ok_or_else(|| anyhow!("operation has no listable artifacts"))?;
    let artifact = match request.artifact_id.as_deref() {
        Some(id) => {
            let matching: Vec<_> = artifacts
                .iter()
                .filter(|artifact| artifact["artifact_id"] == id)
                .collect();
            ensure!(
                matching.len() == 1,
                "artifact_id is not an artifact of this operation"
            );
            matching[0]
        }
        None if artifacts.len() == 1 => &artifacts[0],
        None => {
            let ids: Vec<_> = artifacts
                .iter()
                .filter_map(|artifact| artifact["artifact_id"].as_str())
                .collect();
            bail!(
                "operation produced {} artifacts; pass artifact_id (one of: {})",
                artifacts.len(),
                ids.join(", ")
            )
        }
    };
    let artifact_id = string(artifact, "artifact_id")?.to_owned();
    let (bytes, extension) = artifact_bytes(gongbu, artifact, &execution)?;
    // The final authorization read follows the byte fetch so slow delivery is
    // covered by the correction race check. Nothing is written on refusal.
    let fresh = history_record(hubu, &authorization_id)?;
    let (cost, transaction, settlement, budget_charge) =
        validate_accounting(&record, &fresh, &rows, &authorization_id)?;
    let sha256 = string(artifact, "sha256")?.to_owned();
    let filename = file_name(request.name.as_deref().unwrap_or(&artifact_id), &extension);
    let receipt = json!({
        "schema_version": 1,
        "kind": "hubu_saved_artifact_receipt",
        "file": filename,
        "artifact_id": artifact_id,
        "media_type": artifact["media_type"],
        "size_bytes": bytes.len(),
        "sha256": sha256,
        "width": artifact["metadata"]["width"].as_u64(),
        "height": artifact["metadata"]["height"].as_u64(),
        "operation_handle": request.handle,
        "execution_id": execution,
        "authorization_id": authorization_id,
        "settlement_id": settlement,
        "ledger_transaction_id": transaction,
        "provider": provider,
        "settled_cost": cost,
        "cost_semantics": COST_SEMANTICS,
        "operation_artifact_count": artifacts.len(),
        "budget_charge_cents": budget_charge,
    });
    let (path, receipt_path, already_saved) = write_files(folder, &filename, &bytes, &receipt)?;
    Ok(json!({
        "schema_version": 1,
        "status": if already_saved { "already_saved" } else { "saved" },
        "folder": folder.name,
        "path": path,
        "receipt_path": receipt_path,
        "artifact_id": artifact_id,
        "sha256": sha256,
        "settled_cost": cost,
        "cost_semantics": COST_SEMANTICS,
    }))
}

/// Appends the verified media extension, dropping a duplicate one the agent
/// may already have typed (`logo.png` stays `logo.png`).
fn file_name(stem: &str, extension: &str) -> String {
    let lower = stem.to_ascii_lowercase();
    let duplicate = match extension {
        "png" => lower.ends_with(".png"),
        _ => lower.ends_with(".jpg") || lower.ends_with(".jpeg"),
    };
    if duplicate {
        stem.to_owned()
    } else {
        format!("{stem}.{extension}")
    }
}

/// What may happen to the receipt sidecar of a save.
enum ReceiptSlot {
    /// No receipt exists yet; write one without replacing anything.
    Absent,
    /// This saved artifact's own receipt; refreshing it loses nothing.
    Ours,
}

/// A receipt may only be refreshed when it demonstrably describes this same
/// saved artifact: a Hubu receipt for the same file, artifact ID and digest.
/// Anything else at that path belongs to someone else and is never replaced.
fn receipt_slot(receipt_path: &Path, receipt: &Value) -> Result<ReceiptSlot> {
    let existing = match fs::read(receipt_path) {
        Ok(existing) => existing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ReceiptSlot::Absent)
        }
        Err(_) => bail!("existing receipt could not be read"),
    };
    let ours = serde_json::from_slice::<Value>(&existing).is_ok_and(|existing| {
        existing["kind"] == "hubu_saved_artifact_receipt"
            && ["file", "artifact_id", "sha256"]
                .iter()
                .all(|field| existing[*field] == receipt[*field] && !receipt[*field].is_null())
    });
    ensure!(
        ours,
        "`{}` already exists and is not this artifact's receipt; choose another name",
        receipt_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("receipt")
    );
    Ok(ReceiptSlot::Ours)
}

/// Never overwrites a different file: not the artifact, and not a receipt
/// that belongs to another file or artifact. Every check runs before anything
/// is written. Re-saving identical bytes is idempotent and refreshes this
/// artifact's own receipt.
fn write_files(
    folder: &SaveFolder,
    filename: &str,
    bytes: &[u8],
    receipt: &Value,
) -> Result<(PathBuf, PathBuf, bool)> {
    ensure!(
        Path::new(filename).file_name().and_then(|n| n.to_str()) == Some(filename),
        "invalid destination file name"
    );
    let path = folder.path.join(filename);
    let receipt_path = folder.path.join(format!("{filename}.receipt.json"));
    reject_symlink(&path)?;
    reject_symlink(&receipt_path)?;
    let slot = receipt_slot(&receipt_path, receipt)?;
    let mut temporary = tempfile::NamedTempFile::new_in(&folder.path)
        .map_err(|_| anyhow!("allowlisted folder `{}` is not writable", folder.name))?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    let already_saved = match temporary.persist_noclobber(&path) {
        Ok(_) => false,
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            reject_symlink(&path)?;
            let digest = format!("{:x}", Sha256::digest(fs::read(&path)?));
            ensure!(
                Some(digest.as_str()) == receipt["sha256"].as_str(),
                "`{filename}` already exists in folder `{}` with different content; choose another name",
                folder.name
            );
            true
        }
        Err(_) => bail!("atomic artifact write failed"),
    };
    let receipt_bytes = serde_json::to_vec_pretty(receipt)?;
    match slot {
        ReceiptSlot::Ours => atomic_write(&receipt_path, &receipt_bytes)?,
        ReceiptSlot::Absent => {
            // A receipt that appeared since the check is never replaced.
            let mut temporary = tempfile::NamedTempFile::new_in(&folder.path)?;
            temporary.write_all(&receipt_bytes)?;
            temporary.as_file().sync_all()?;
            match temporary.persist_noclobber(&receipt_path) {
                Ok(_) => {}
                Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                    reject_symlink(&receipt_path)?;
                    if let ReceiptSlot::Ours = receipt_slot(&receipt_path, receipt)? {
                        atomic_write(&receipt_path, &receipt_bytes)?;
                    }
                }
                Err(_) => bail!("atomic receipt write failed"),
            }
        }
    }
    Ok((path, receipt_path, already_saved))
}

pub(crate) fn backend_availability(snapshot: &CapabilitySnapshot) -> Result<(), &'static str> {
    tool_availability(
        "hubu_get_authorization_record",
        BackendOwner::Hubu,
        snapshot,
    )
    .and_then(|()| tool_availability("gongbu_get_artifact", BackendOwner::Gongbu, snapshot))
    .map_err(|rejection| rejection.reason_code())
}

/// `(available, reason_code)` for the router capability entry.
pub(crate) fn availability(
    server_registry_available: bool,
    snapshot: &CapabilitySnapshot,
    folders: &Result<Vec<SaveFolder>>,
) -> (bool, Option<&'static str>) {
    if !server_registry_available {
        return (false, Some("operation_registry_unavailable"));
    }
    if let Err(reason) = backend_availability(snapshot) {
        return (false, Some(reason));
    }
    match folders {
        Ok(folders) if !folders.is_empty() => (true, None),
        Ok(_) => (false, Some("save_folders_unconfigured")),
        Err(_) => (false, Some("save_folders_invalid")),
    }
}

pub(crate) fn folders_for(config_path: Option<&Path>) -> Result<Vec<SaveFolder>> {
    match config_path {
        Some(path) => load_folders(path),
        None => Ok(Vec::new()),
    }
}

pub(crate) fn tool_definition(folders: &[SaveFolder]) -> Value {
    let names: Vec<_> = folders.iter().map(|folder| folder.name.clone()).collect();
    json!({
        "name": TOOL_NAME,
        "description": "Save one artifact of a succeeded governed operation into a folder the human allowlisted, with a `<file>.receipt.json` sidecar holding the artifact ID, SHA-256 digest and Hubu-settled cost. Pass an allowlisted folder name, never a path. Omit artifact_id only when the operation produced exactly one artifact. Never overwrites a different file; re-saving the same bytes is idempotent.",
        "inputSchema": {
            "type": "object",
            "additionalProperties": false,
            "required": ["operation_handle", "folder"],
            "properties": {
                "operation_handle": {
                    "type": "string",
                    "pattern": "^hubu:public-operation:v1:",
                    "maxLength": 160
                },
                "artifact_id": {
                    "type": "string",
                    "pattern": "^[A-Za-z0-9_-]{1,255}$"
                },
                "folder": {
                    "type": "string",
                    "description": "Name of a human-allowlisted save folder.",
                    "enum": names
                },
                "name": {
                    "type": "string",
                    "description": "Optional file name stem; the verified image extension is appended. Defaults to the artifact ID.",
                    "pattern": "^[A-Za-z0-9][A-Za-z0-9._-]{0,99}$"
                }
            }
        },
        "annotations": {
            "readOnlyHint": false,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false,
            "x_hubu_human_approval": "none",
            "x_hubu_client_approval_mode": "auto",
            "x_hubu_runtime_approval": "none"
        }
    })
}

fn tool_error(id: Value, message: &str) -> Value {
    success_response(
        id,
        json!({"isError": true, "content": [{"type": "text", "text": message}]}),
    )
}

pub(crate) fn call_tool(server: &Server, id: Value, call: ToolCall) -> Value {
    let request = match request_from_arguments(&call.arguments) {
        Ok(request) => request,
        Err(error) if error.to_string() == "Invalid params" => {
            return error_response(id, -32602, "Invalid params")
        }
        Err(error) => return tool_error(id, &error.to_string()),
    };
    let folders = match configured_folders(server.save_folders_path.as_deref()) {
        Ok(folders) => folders,
        Err(error) => return tool_error(id, &error.to_string()),
    };
    let folder = match resolve_folder(&folders, &request.folder) {
        Ok(folder) => folder,
        Err(error) => return tool_error(id, &error.to_string()),
    };
    if let Err(reason) = backend_availability(&server.snapshot()) {
        return tool_error(
            id,
            &format!("artifact save requires available Hubu and Gongbu backends ({reason})"),
        );
    }
    let context = match server.saved_artifact_context(&request.handle) {
        Ok(context) => context,
        Err(_) => {
            return tool_error(
                id,
                "operation is unknown, not yet succeeded, or not a governed execution",
            )
        }
    };
    match save(&server.backends, context, folder, &request) {
        Ok(result) => success_response(
            id,
            json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string_pretty(&result).expect("save result serializes")
                }],
                "structuredContent": result
            }),
        ),
        Err(error) => tool_error(id, &format!("artifact was not saved: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folders_file(root: &Path, body: Value) -> PathBuf {
        let path = root.join("save-folders.json");
        fs::write(&path, serde_json::to_vec(&body).unwrap()).unwrap();
        path
    }

    #[test]
    fn allowlist_parses_and_rejects_unsafe_entries() {
        let root = tempfile::tempdir().unwrap();
        assert!(load_folders(&root.path().join("missing.json"))
            .unwrap()
            .is_empty());
        let path = folders_file(
            root.path(),
            json!({"schema_version":1,"folders":[{"name":"brand","path":"/work/brand"}]}),
        );
        assert_eq!(
            load_folders(&path).unwrap(),
            vec![SaveFolder {
                name: "brand".into(),
                path: "/work/brand".into()
            }]
        );
        for body in [
            json!({"schema_version":2,"folders":[]}),
            json!({"schema_version":1,"folders":[{"name":"brand","path":"relative"}]}),
            json!({"schema_version":1,"folders":[{"name":"../x","path":"/x"}]}),
            json!({"schema_version":1,"folders":[{"name":"a","path":"/a"},{"name":"a","path":"/b"}]}),
        ] {
            let path = folders_file(root.path(), body);
            assert!(load_folders(&path).is_err());
        }
    }

    #[test]
    fn agent_arguments_cannot_name_paths() {
        let handle = "hubu:public-operation:v1:0123456789abcdef0123456789abcdef";
        let ok = request_from_arguments(
            &json!({"operation_handle":handle,"folder":"brand","name":"logo-v2.final"}),
        )
        .unwrap();
        assert_eq!(ok.name.as_deref(), Some("logo-v2.final"));
        for arguments in [
            json!({"operation_handle":handle,"folder":"/tmp"}),
            json!({"operation_handle":handle,"folder":"brand","name":"../escape"}),
            json!({"operation_handle":handle,"folder":"brand","name":"sub/logo"}),
            json!({"operation_handle":handle,"folder":"brand","name":".hidden"}),
            json!({"operation_handle":handle,"folder":"brand","artifact_id":"a/b"}),
            json!({"operation_handle":handle,"folder":"brand","path":"/tmp"}),
            json!({"operation_handle":"op","folder":"brand"}),
        ] {
            assert!(request_from_arguments(&arguments).is_err(), "{arguments}");
        }
        let folders = vec![SaveFolder {
            name: "brand".into(),
            path: "/work/brand".into(),
        }];
        let error = resolve_folder(&folders, "other").unwrap_err().to_string();
        assert!(error.contains("brand") && !error.contains("/work/brand"));
    }

    #[test]
    fn file_names_append_verified_extension_once() {
        assert_eq!(file_name("logo", "png"), "logo.png");
        assert_eq!(file_name("logo.PNG", "png"), "logo.PNG");
        assert_eq!(file_name("logo.png", "jpg"), "logo.png.jpg");
        assert_eq!(file_name("photo.jpeg", "jpg"), "photo.jpeg");
    }

    #[test]
    fn writes_never_clobber_different_files() {
        let root = tempfile::tempdir().unwrap();
        let folder = SaveFolder {
            name: "brand".into(),
            path: root.path().into(),
        };
        let bytes = b"\x89PNG\r\n\x1a\nlocal";
        let receipt = json!({
            "kind": "hubu_saved_artifact_receipt",
            "file": "logo.png",
            "artifact_id": "artifact-1",
            "sha256": format!("{:x}", Sha256::digest(bytes)),
        });
        let (path, receipt_path, again) =
            write_files(&folder, "logo.png", bytes, &receipt).unwrap();
        assert!(!again);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(receipt_path.exists());
        assert!(write_files(&folder, "logo.png", bytes, &receipt).unwrap().2);
        fs::write(&path, b"human edit").unwrap();
        assert!(write_files(&folder, "logo.png", bytes, &receipt).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"human edit");
    }

    #[test]
    fn writes_never_clobber_another_receipt() {
        let root = tempfile::tempdir().unwrap();
        let folder = SaveFolder {
            name: "brand".into(),
            path: root.path().into(),
        };
        let bytes = b"\x89PNG\r\n\x1a\nlocal";
        let receipt = json!({
            "kind": "hubu_saved_artifact_receipt",
            "file": "logo.png",
            "artifact_id": "artifact-1",
            "sha256": format!("{:x}", Sha256::digest(bytes)),
        });

        // An unrelated file already sits where the receipt would go: nothing
        // is written, not even the artifact.
        let receipt_path = root.path().join("logo.png.receipt.json");
        fs::write(&receipt_path, b"human notes").unwrap();
        assert!(write_files(&folder, "logo.png", bytes, &receipt)
            .unwrap_err()
            .to_string()
            .contains("is not this artifact's receipt"));
        assert_eq!(fs::read(&receipt_path).unwrap(), b"human notes");
        assert!(!root.path().join("logo.png").exists());

        // Its own receipt is refreshed on an identical re-save.
        fs::remove_file(&receipt_path).unwrap();
        write_files(&folder, "logo.png", bytes, &receipt).unwrap();
        let mut refreshed = receipt.clone();
        refreshed["ledger_transaction_id"] = json!("txn-2");
        assert!(
            write_files(&folder, "logo.png", bytes, &refreshed)
                .unwrap()
                .2
        );
        let stored: Value = serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
        assert_eq!(stored["ledger_transaction_id"], "txn-2");

        // Same bytes from another artifact must not take over its provenance.
        let mut other = receipt.clone();
        other["artifact_id"] = json!("artifact-2");
        assert!(write_files(&folder, "logo.png", bytes, &other).is_err());
        let stored: Value = serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
        assert_eq!(stored["artifact_id"], "artifact-1");
    }
}
