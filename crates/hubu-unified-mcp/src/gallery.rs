//! Native local artifact export. No MCP server, worker, provider call or model
//! byte round-trip participates: all remote operations below are bounded GETs.
use crate::{operation_registry, BackendClient, BackendClients, Config};
use anyhow::{anyhow, bail, ensure, Result};
use reqwest::header;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

const IMAGE_LIMIT: u64 = 64 * 1024 * 1024;
const JSON_LIMIT: u64 = 16 * 1024 * 1024;
const MAX_PAGES: usize = 1000;
// The shared MCP client's 3s budget is for small JSON calls, not image bytes.
const ARTIFACT_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Deserialize, Serialize, Clone, PartialEq, Eq)]
struct Entry {
    execution_id: String,
    artifact_id: String,
    authorization_id: String,
    settlement_id: String,
    ledger_transaction_id: String,
    provider: String,
    size: String,
    // Gongbu-recorded pixel dimensions; `size` is the caller's preset label.
    width: Option<u64>,
    height: Option<u64>,
    tier: String,
    settled_cost: Value,
    sha256: String,
    cost_semantics: String,
    budget_charge_cents: i64,
    sequence: u64,
    filename: String,
}

struct Options {
    handle: String,
    output: PathBuf,
    size: String,
    tier: String,
}

pub fn run_gallery_from_env(arguments: Vec<String>) -> Result<Value> {
    let options = parse_options(arguments)?;
    export(Config::from_env()?, options)
}

fn parse_options(arguments: Vec<String>) -> Result<Options> {
    ensure!(arguments.first().map(String::as_str) == Some("export"),
        "usage: gallery export --operation-handle HANDLE --output ABSOLUTE_DIR --size 2k --tier draft");
    let mut values = std::collections::HashMap::new();
    for pair in arguments[1..].chunks(2) {
        ensure!(
            pair.len() == 2
                && matches!(
                    pair[0].as_str(),
                    "--operation-handle" | "--output" | "--size" | "--tier"
                ),
            "invalid gallery option"
        );
        ensure!(
            values.insert(pair[0].clone(), pair[1].clone()).is_none(),
            "duplicate gallery option"
        );
    }
    let mut required = |key| {
        values
            .remove(key)
            .ok_or_else(|| anyhow!("missing gallery option {key}"))
    };
    let options = Options {
        handle: required("--operation-handle")?,
        output: required("--output")?.into(),
        size: required("--size")?.to_ascii_lowercase(),
        tier: required("--tier")?,
    };
    ensure!(
        matches!(options.tier.as_str(), "draft" | "final"),
        "gallery tier must be draft or final"
    );
    ensure!(
        matches!(options.size.as_str(), "512" | "1k" | "2k" | "4k" | "custom"),
        "unsupported gallery size label"
    );
    ensure!(
        options.output.is_absolute(),
        "gallery output directory must be absolute"
    );
    Ok(options)
}

fn read_json(client: &BackendClient, path: &str, query: &[(&str, &str)]) -> Result<Value> {
    let mut url = client
        .endpoint()
        .join(path.trim_start_matches('/'))
        .map_err(|_| anyhow!("invalid gallery read route"))?;
    if !query.is_empty() {
        url.query_pairs_mut().extend_pairs(query.iter().copied());
    }
    let response = client
        .http_client()
        .get(url)
        .send()
        .map_err(|_| anyhow!("gallery backend read unavailable"))?;
    ensure!(
        response.status().is_success(),
        "gallery backend read rejected (HTTP {})",
        response.status().as_u16()
    );
    let mut bytes = Vec::new();
    response
        .take(JSON_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow!("gallery backend response unavailable"))?;
    ensure!(
        bytes.len() as u64 <= JSON_LIMIT,
        "gallery JSON response exceeds limit"
    );
    serde_json::from_slice(&bytes).map_err(|_| anyhow!("invalid gallery backend JSON"))
}

fn string<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    value
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing gallery evidence field {name}"))
}

fn history_record(client: &BackendClient, id: &str) -> Result<Value> {
    let value = read_json(
        client,
        "spend/authorizations/show",
        &[("authorization_id", id)],
    )?;
    ensure!(
        value["schema_version"] == "hubu-history-v1",
        "unsupported authorization history schema"
    );
    let record = &value["authorization_record"];
    ensure!(
        record["authorization_id"] == id && record["status"] == "settled",
        "authorization is mismatched or not settled"
    );
    Ok(record.clone())
}

fn exact_cents(cost: &Value) -> Result<String> {
    ensure!(
        cost["currency"] == "usd",
        "demo filenames require canonical usd"
    );
    let raw = string(cost, "amount")?;
    ensure!(
        !raw.is_empty() && raw.len() <= 39 && raw.bytes().all(|b| b.is_ascii_digit()),
        "invalid exact cost coefficient"
    );
    let amount: u128 = raw
        .parse()
        .map_err(|_| anyhow!("invalid exact cost coefficient"))?;
    let scale = cost["scale"]
        .as_u64()
        .filter(|s| *s <= 18)
        .ok_or_else(|| anyhow!("invalid exact cost scale"))? as u32;
    if scale <= 2 {
        return amount
            .checked_mul(10_u128.pow(2 - scale))
            .map(|a| a.to_string())
            .ok_or_else(|| anyhow!("exact cost overflow"));
    }
    let divisor = 10_u128.pow(scale - 2);
    let whole = amount / divisor;
    let fraction = amount % divisor;
    if fraction == 0 {
        return Ok(whole.to_string());
    }
    let fraction = format!("{fraction:0width$}", width = (scale - 2) as usize);
    Ok(format!("{whole}.{}", fraction.trim_end_matches('0')))
}

fn ledger_rows(client: &BackendClient, record: &Value) -> Result<Vec<Value>> {
    let agent = string(record, "agent_id")?;
    let account = string(record, "account_id")?;
    let mut cursor = String::new();
    let mut seen = std::collections::HashSet::new();
    let mut rows = Vec::new();
    // Complete this cursor snapshot. A fresh authorization read below detects
    // corrections newer than its upper bound, rather than mixing page snapshots.
    for _ in 0..MAX_PAGES {
        let mut query = vec![
            ("agent_id", agent),
            ("account_id", account),
            ("limit", "100"),
        ];
        if !cursor.is_empty() {
            query.push(("cursor", &cursor));
        }
        let page = read_json(client, "ledger/transactions", &query)?;
        ensure!(
            page["schema_version"] == "hubu-history-v1",
            "unsupported ledger history schema"
        );
        let transactions = page["transactions"]
            .as_array()
            .ok_or_else(|| anyhow!("invalid ledger page"))?;
        ensure!(
            transactions.len() <= 100,
            "ledger page exceeds advertised maximum"
        );
        rows.extend(transactions.iter().cloned());
        ensure!(
            rows.len() <= 10_000,
            "gallery ledger snapshot exceeds limit"
        );
        match page.get("next_cursor") {
            None | Some(Value::Null) => return Ok(rows),
            Some(Value::String(next)) if !next.is_empty() && seen.insert(next.clone()) => {
                cursor = next.clone()
            }
            _ => bail!("invalid or repeating ledger cursor"),
        }
    }
    bail!("gallery ledger pagination limit exceeded")
}

fn validate_accounting(
    record: &Value,
    fresh: &Value,
    rows: &[Value],
    authorization_id: &str,
) -> Result<(Value, String, String, i64)> {
    for field in [
        "authorization_id",
        "status",
        "provider",
        "receipt",
        "ledger_transaction_ids",
    ] {
        ensure!(
            record[field] == fresh[field],
            "authorization accounting changed during export; refresh evidence"
        );
    }
    let receipt = &record["receipt"];
    let settlement = string(receipt, "settlement_id")?;
    let links = record["ledger_transaction_ids"]
        .as_array()
        .ok_or_else(|| anyhow!("missing ledger links"))?;
    ensure!(
        links.len() == 1,
        "corrected/multiple ledger postings require gallery reconciliation"
    );
    let transaction_id = links[0]
        .as_str()
        .ok_or_else(|| anyhow!("invalid ledger link"))?;
    let related: Vec<_> = rows
        .iter()
        .filter(|r| r["authorization_id"] == authorization_id || r["settlement_id"] == settlement)
        .collect();
    ensure!(
        related.len() == 1 && related[0]["id"] == transaction_id,
        "missing or unlisted settlement posting/correction"
    );
    let row = related[0];
    ensure!(
        row["authorization_id"] == authorization_id && row["settlement_id"] == settlement,
        "ledger settlement identity mismatch"
    );
    ensure!(
        row["cost_semantics"] == "original_total"
            && row["effective_cost"] == receipt["actual_vendor_cost"],
        "ledger/receipt exact cost mismatch"
    );
    ensure!(
        row["provider"] == record["provider"],
        "ledger provider identity mismatch"
    );
    exact_cents(&row["effective_cost"])?;
    let budget_charge = receipt["budget_charge_cents"]
        .as_i64()
        .ok_or_else(|| anyhow!("invalid settled budget charge"))?;
    Ok((
        row["effective_cost"].clone(),
        transaction_id.into(),
        settlement.into(),
        budget_charge,
    ))
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 255
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

fn artifact_bytes(
    client: &BackendClient,
    artifact: &Value,
    execution: &str,
) -> Result<(Vec<u8>, String)> {
    ensure!(
        artifact["execution_id"] == execution && artifact["kind"] == "image",
        "artifact execution/type mismatch"
    );
    let id = string(artifact, "artifact_id")?;
    ensure!(safe_id(id), "invalid artifact identity");
    let media = string(artifact, "media_type")?;
    ensure!(
        matches!(media, "image/png" | "image/jpeg"),
        "unsupported gallery image format"
    );
    let expected = artifact["size_bytes"]
        .as_u64()
        .filter(|s| *s > 0 && *s <= IMAGE_LIMIT)
        .ok_or_else(|| anyhow!("invalid gallery artifact size"))?;
    let digest = string(artifact, "sha256")?;
    ensure!(
        digest.len() == 64
            && digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid artifact digest"
    );
    let url = client
        .endpoint()
        .join(&format!("v1/artifacts/{id}"))
        .map_err(|_| anyhow!("invalid artifact read route"))?;
    let response = client
        .http_client()
        .get(url)
        .timeout(ARTIFACT_TIMEOUT)
        .send()
        .map_err(|_| anyhow!("artifact backend unavailable"))?;
    ensure!(
        response.status().is_success(),
        "artifact retrieval rejected (HTTP {})",
        response.status().as_u16()
    );
    let response_media = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|s| s.to_str().ok())
        .and_then(|s| s.split(';').next());
    ensure!(
        response_media == Some(media),
        "artifact response media type mismatch"
    );
    let mut bytes = Vec::new();
    response
        .take(expected + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow!("artifact read failed"))?;
    ensure!(
        bytes.len() as u64 == expected && format!("{:x}", Sha256::digest(&bytes)) == digest,
        "artifact byte count/digest mismatch"
    );
    let signature = if media == "image/png" {
        bytes.starts_with(b"\x89PNG\r\n\x1a\n")
    } else {
        bytes.starts_with(b"\xff\xd8\xff")
    };
    ensure!(signature, "artifact signature mismatch");
    Ok((
        bytes,
        if media == "image/png" { "png" } else { "jpg" }.into(),
    ))
}

fn export(config: Config, options: Options) -> Result<Value> {
    let state = config
        .operation_state_path
        .as_ref()
        .ok_or_else(|| anyhow!("gallery requires configured unified operation state"))?;
    let (authorization_id, execution) = operation_registry::gallery_context(state, &options.handle)
        .map_err(|_| anyhow!("gallery operation context unavailable or not succeeded"))?;
    ensure!(safe_id(&execution), "invalid bound execution identity");
    let clients = BackendClients::new(config)?; // Constructs clients only: no Server, probes or workers.
    let hubu = clients
        .hubu
        .as_ref()
        .ok_or_else(|| anyhow!("gallery Hubu backend missing"))?;
    let gongbu = clients
        .gongbu
        .as_ref()
        .ok_or_else(|| anyhow!("gallery Gongbu backend missing"))?;
    let record = history_record(hubu, &authorization_id)?;
    let provider = match record["provider"]["id"].as_str() {
        Some("provider:black-forest-labs:flux") => "flux",
        Some("provider:google:gemini-developer") => "gemini",
        _ => bail!("unsupported gallery provider"),
    };
    let rows = ledger_rows(hubu, &record)?;
    let listing = read_json(gongbu, &format!("v1/executions/{execution}/artifacts"), &[])?;
    ensure!(
        listing["schema_version"] == 1 && listing["execution_id"] == execution,
        "artifact list identity/schema mismatch"
    );
    let artifacts = listing["artifacts"]
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= 64)
        .ok_or_else(|| anyhow!("missing or oversized gallery artifact list"))?;
    let mut images = Vec::new();
    let mut total_bytes = 0_u64;
    let mut ids = std::collections::HashSet::new();
    for artifact in artifacts {
        let id = string(artifact, "artifact_id")?;
        ensure!(
            ids.insert(id.to_owned()),
            "duplicate gallery artifact identity"
        );
        let (bytes, extension) = artifact_bytes(gongbu, artifact, &execution)?;
        total_bytes += bytes.len() as u64;
        ensure!(
            total_bytes <= 128 * 1024 * 1024,
            "gallery operation artifact bytes exceed limit"
        );
        images.push((artifact.clone(), bytes, extension));
    }
    // Final read occurs after all artifact fetches so slow multi-MB delivery is
    // also covered by the correction race check. No output exists on refusal.
    let fresh = history_record(hubu, &authorization_id)?;
    let (cost, transaction, settlement, budget_charge) =
        validate_accounting(&record, &fresh, &rows, &authorization_id)?;
    let mut results = Vec::new();
    for (artifact, bytes, extension) in images {
        let entry = Entry {
            execution_id: execution.clone(),
            artifact_id: string(&artifact, "artifact_id")?.into(),
            authorization_id: authorization_id.clone(),
            settlement_id: settlement.clone(),
            ledger_transaction_id: transaction.clone(),
            provider: provider.into(),
            size: options.size.clone(),
            width: artifact["metadata"]["width"].as_u64(),
            height: artifact["metadata"]["height"].as_u64(),
            tier: options.tier.clone(),
            settled_cost: cost.clone(),
            sha256: string(&artifact, "sha256")?.into(),
            cost_semantics: "operation_total_at_export_not_per_image".into(),
            budget_charge_cents: budget_charge,
            sequence: 0,
            filename: String::new(),
        };
        results.push(write_image(&options.output, entry, &bytes, &extension)?);
    }
    Ok(
        json!({"schema_version":1,"images":results,"settled_cost":cost,"cost_semantics":"operation_total_at_export_not_per_image"}),
    )
}

fn reject_symlink(path: &Path) -> Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        ensure!(
            !metadata.file_type().is_symlink(),
            "gallery refuses symlink output"
        );
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    reject_symlink(path)?;
    let mut temporary = tempfile::NamedTempFile::new_in(
        path.parent()
            .ok_or_else(|| anyhow!("invalid gallery path"))?,
    )?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|_| anyhow!("atomic gallery write failed"))?;
    Ok(())
}

#[cfg(unix)]
fn lock_file(path: &Path) -> Result<File> {
    use std::os::unix::{fs::OpenOptionsExt, io::AsRawFd};
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600)
        .open(path)?;
    // SAFETY: the descriptor remains owned by file and flock changes only its lock.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    ensure!(result == 0, "gallery lock failed");
    Ok(file)
}
#[cfg(not(unix))]
fn lock_file(_path: &Path) -> Result<File> {
    bail!("gallery export currently requires macOS/Linux file locking")
}

fn write_image(output: &Path, mut entry: Entry, bytes: &[u8], extension: &str) -> Result<Value> {
    reject_symlink(output)?;
    fs::create_dir_all(output)?;
    let _lock = lock_file(&output.join(".gallery.lock"))?;
    let manifest_path = output.join(".gallery.json");
    reject_symlink(&manifest_path)?;
    let mut manifest: Vec<Entry> = if manifest_path.exists() {
        serde_json::from_slice(&fs::read(&manifest_path)?)?
    } else {
        Vec::new()
    };
    let prior: Vec<_> = manifest
        .iter()
        .filter(|old| {
            old.execution_id == entry.execution_id && old.artifact_id == entry.artifact_id
        })
        .collect();
    ensure!(prior.len() <= 1, "duplicate manifest identity");
    if let Some(old) = prior.first() {
        entry.sequence = old.sequence;
        entry.filename.clone_from(&old.filename);
        ensure!(
            **old == entry,
            "existing gallery evidence/labels changed; reconcile explicitly"
        );
    } else {
        entry.sequence = manifest
            .iter()
            .map(|e| e.sequence)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| anyhow!("gallery sequence overflow"))?;
        entry.filename = format!(
            "{:02}-{}-{}-{}-{}c.{}",
            entry.sequence,
            entry.provider,
            entry.size,
            entry.tier,
            exact_cents(&entry.settled_cost)?,
            extension
        );
        ensure!(
            !output.join(&entry.filename).exists(),
            "gallery refuses unrelated existing image"
        );
        manifest.push(entry.clone());
        atomic_write(&manifest_path, &serde_json::to_vec_pretty(&manifest)?)?;
    }
    ensure!(
        Path::new(&entry.filename)
            .file_name()
            .and_then(|n| n.to_str())
            == Some(&entry.filename),
        "invalid gallery manifest filename"
    );
    let image_path = output.join(&entry.filename);
    reject_symlink(&image_path)?;
    if image_path.exists() {
        ensure!(
            format!("{:x}", Sha256::digest(fs::read(&image_path)?)) == entry.sha256,
            "existing gallery image changed"
        );
    } else {
        atomic_write(&image_path, bytes)?;
    }
    let receipt_path = output.join(format!("{}.receipt.json", entry.filename));
    atomic_write(&receipt_path, &serde_json::to_vec_pretty(&entry)?)?;
    Ok(
        json!({"path":image_path,"receipt_path":receipt_path,"provider":entry.provider,"size":entry.size,"tier":entry.tier,"settled_cost":entry.settled_cost}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fractional_cost_is_exact() {
        for (amount, scale, expected) in [
            ("1", 3, "0.1"),
            ("6", 2, "6"),
            ("6", 0, "600"),
            ("0", 18, "0"),
        ] {
            assert_eq!(
                exact_cents(&json!({"amount":amount,"scale":scale,"currency":"usd"})).unwrap(),
                expected
            );
        }
        assert!(exact_cents(&json!({"amount":"1","scale":19,"currency":"usd"})).is_err());
    }

    #[test]
    fn manifest_reservation_repairs_interruption_and_refuses_changed_bytes() {
        let root = tempfile::tempdir().unwrap();
        let bytes = b"\x89PNG\r\n\x1a\nlocal";
        let entry = Entry {
            execution_id: "execution-1".into(),
            artifact_id: "artifact-1".into(),
            authorization_id: "decision-1".into(),
            settlement_id: "settlement-1".into(),
            ledger_transaction_id: "ledger-1".into(),
            provider: "flux".into(),
            size: "2k".into(),
            width: Some(2048),
            height: Some(2048),
            tier: "draft".into(),
            settled_cost: json!({"amount":"6","scale":2,"currency":"usd"}),
            sha256: format!("{:x}", Sha256::digest(bytes)),
            cost_semantics: "operation_total_at_export_not_per_image".into(),
            budget_charge_cents: 6,
            sequence: 1,
            filename: "01-flux-2k-draft-6c.png".into(),
        };
        // Simulate an interruption after the durable reservation but before image write.
        atomic_write(
            &root.path().join(".gallery.json"),
            &serde_json::to_vec(&vec![entry.clone()]).unwrap(),
        )
        .unwrap();
        let value = write_image(root.path(), entry.clone(), bytes, "png").unwrap();
        assert_eq!(fs::read(value["path"].as_str().unwrap()).unwrap(), bytes);
        assert!(Path::new(value["receipt_path"].as_str().unwrap()).exists());
        fs::write(value["path"].as_str().unwrap(), b"changed").unwrap();
        assert!(write_image(root.path(), entry, bytes, "png").is_err());
        assert_eq!(
            fs::read(value["path"].as_str().unwrap()).unwrap(),
            b"changed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn output_symlinks_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let link = root.path().join("link");
        std::os::unix::fs::symlink(root.path(), &link).unwrap();
        assert!(reject_symlink(&link).is_err());
    }
}
