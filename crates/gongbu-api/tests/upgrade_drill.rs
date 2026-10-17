//! Upgrade drill for the v0.2.4 micro-unit migration on a real Gongbu
//! database. Opt-in; run with:
//!
//! ```text
//! GONGBU_UPGRADE_DRILL_SOURCE=<path to gongbu.sqlite3> \
//!   cargo test -p gongbu-api --test upgrade_drill -- --ignored --nocapture
//! ```
//!
//! The drill never writes to the source: it opens it read-only, copies it
//! with `VACUUM INTO` into a private temporary directory, and migrates only
//! that copy. The source may be a live stack database.

use gongbu_api::{
    execution::{Error as PersistenceError, Repository},
    provider_contract::PricingSnapshot,
    redaction::Redactor,
};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Copy `source` into `directory` through a read-only connection.
fn private_copy(source: &Path, directory: &Path) -> PathBuf {
    let copy = directory.join("gongbu.sqlite3");
    let reader = Connection::open_with_flags(
        source,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .expect("open the source database read-only");
    reader
        .execute("VACUUM INTO ?1", [copy.to_str().expect("utf-8 temp path")])
        .expect("copy the source database");
    copy
}

struct LegacyRow {
    execution_id: String,
    authorized_minor: i64,
    pricing_snapshot: Value,
    snapshot_minor: Option<i64>,
    receipt: Option<(i64, Value)>,
}

fn legacy_rows(path: &Path) -> Vec<LegacyRow> {
    let connection = Connection::open(path).unwrap();
    let mut statement = connection
        .prepare(
            "SELECT e.execution_id, e.authorized_minor, e.pricing_snapshot_json,
                    s.amount_minor, r.settlement_minor, r.pricing_snapshot_json
             FROM executions e
             LEFT JOIN hubu_authorization_snapshots s ON s.execution_id = e.execution_id
             LEFT JOIN receipts r ON r.execution_id = e.execution_id
             ORDER BY e.execution_id",
        )
        .unwrap();
    let rows = statement
        .query_map([], |row| {
            let pricing: String = row.get(2)?;
            let receipt_minor: Option<i64> = row.get(4)?;
            let receipt_snapshot: Option<String> = row.get(5)?;
            Ok(LegacyRow {
                execution_id: row.get(0)?,
                authorized_minor: row.get(1)?,
                pricing_snapshot: serde_json::from_str(&pricing).unwrap(),
                snapshot_minor: row.get(3)?,
                receipt: receipt_minor.map(|minor| {
                    (
                        minor,
                        serde_json::from_str(&receipt_snapshot.unwrap_or_default())
                            .unwrap_or(Value::Null),
                    )
                }),
            })
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    rows
}

fn verify(repository: &Repository, rows: &[LegacyRow]) {
    for row in rows {
        let execution = repository.get_execution(&row.execution_id).unwrap();
        assert_eq!(
            execution.authorized_units,
            row.authorized_minor * 10_000,
            "{}",
            row.execution_id
        );
        // Frozen schema 2 snapshots load and re-serialize byte for byte.
        let snapshot: PricingSnapshot =
            serde_json::from_value(row.pricing_snapshot.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(&snapshot).unwrap(),
            row.pricing_snapshot
        );
        match repository.get_hubu_authorization_snapshot(&row.execution_id) {
            Ok(authorization) => assert_eq!(
                Some(authorization.amount_units),
                row.snapshot_minor.map(|minor| minor * 10_000)
            ),
            Err(PersistenceError::NotFound) => assert!(row.snapshot_minor.is_none()),
            Err(error) => panic!("{}: {error}", row.execution_id),
        }
        if let Some((minor, settlement_snapshot)) = &row.receipt {
            let receipt = repository
                .get_receipt_for_execution(&row.execution_id)
                .unwrap();
            assert_eq!(receipt.settlement_units, minor * 10_000);
            // The settlement retry payload never changes.
            assert_eq!(&receipt.price_model_snapshot, settlement_snapshot);
        }
    }
}

#[test]
#[ignore = "upgrade drill on a private copy of a real database; set GONGBU_UPGRADE_DRILL_SOURCE"]
fn accounting_units_upgrade_drill_on_a_copy() {
    let source = PathBuf::from(
        std::env::var("GONGBU_UPGRADE_DRILL_SOURCE").expect("set GONGBU_UPGRADE_DRILL_SOURCE"),
    );
    let directory = tempfile::tempdir().unwrap();
    let path = private_copy(&source, directory.path());
    let rows = legacy_rows(&path);
    assert!(!rows.is_empty(), "the drill copy has no executions");
    for open in ["migrate", "reopen"] {
        let repository = Repository::open(&path, Redactor::default()).unwrap();
        verify(&repository, &rows);
        drop(repository);
        println!("{open}: {} executions verified", rows.len());
    }
    let receipts = rows.iter().filter(|row| row.receipt.is_some()).count();
    println!(
        "upgrade drill passed: {} executions, {receipts} receipts",
        rows.len()
    );
}
