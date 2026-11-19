//! Owner presence governance records (v0.3.0, spec section 9).
//!
//! One row per owner-presence decision: every prompt the server raised
//! (confirmed, denied, timed out, unavailable, rate limited, superseded), every
//! owner action performed inside an open owner window, and every tightening
//! write that needed no prompt. The table is additive and never rewrites
//! existing governance rows.
use super::*;

/// One stored owner presence record. All text fields are built by
/// `hubu-server`; none of them is caller-supplied free text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerPresenceRecord {
    pub record_id: String,
    pub recorded_at: DateTime<Utc>,
    /// Route and action kind, for example `budget.version.create`.
    pub action: String,
    /// The exact reason text shown in the dialog (or, for an action inside a
    /// window, the text of the prompt that opened the window).
    pub reason: String,
    /// `confirmed`, `denied`, `timed_out`, `unavailable`, `rate_limited`,
    /// `superseded`, `not_required`, or `window` (performed inside a window).
    pub outcome: String,
    /// `biometric`, `device_password`, `fixture`, or none.
    pub method: Option<String>,
    /// `portal`, `cli`, `agent`, or `unknown`.
    pub client: String,
    /// Public ids of the budget, policy, agent or authorization concerned.
    pub targets: Vec<String>,
    /// For an action inside a window: the record of the prompt that opened it.
    pub window_record_id: Option<String>,
}

pub(super) const OWNER_PRESENCE_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS owner_presence_records (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    record_id TEXT NOT NULL UNIQUE,
    recorded_at TEXT NOT NULL,
    action TEXT NOT NULL,
    reason TEXT NOT NULL,
    outcome TEXT NOT NULL,
    method TEXT,
    client TEXT NOT NULL,
    targets_json TEXT NOT NULL,
    window_record_id TEXT
);
";

impl SqliteGovernanceRepository {
    pub fn record_owner_presence(
        &mut self,
        record: &OwnerPresenceRecord,
    ) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO owner_presence_records
             (record_id, recorded_at, action, reason, outcome, method, client, targets_json, window_record_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                record.record_id,
                record.recorded_at.to_rfc3339(),
                record.action,
                record.reason,
                record.outcome,
                record.method,
                record.client,
                serde_json::to_string(&record.targets)?,
                record.window_record_id,
            ],
        )?;
        Ok(())
    }

    /// Newest first, at most `limit` rows.
    pub fn list_owner_presence_records(
        &self,
        limit: usize,
    ) -> Result<Vec<OwnerPresenceRecord>, StorageError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut stmt = self.conn.prepare(
            "SELECT record_id, recorded_at, action, reason, outcome, method, client, targets_json, window_record_id
             FROM owner_presence_records ORDER BY seq DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit], |row| {
            let targets: String = row.get(7)?;
            Ok(OwnerPresenceRecord {
                record_id: row.get(0)?,
                recorded_at: parse_timestamp(&row.get::<_, String>(1)?)?,
                action: row.get(2)?,
                reason: row.get(3)?,
                outcome: row.get(4)?,
                method: row.get(5)?,
                client: row.get(6)?,
                targets: parse_json(&targets)?,
                window_record_id: row.get(8)?,
            })
        })?;
        collect_rows(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_presence_records_round_trip_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let mut repo = SqliteGovernanceRepository::open(dir.path().join("hubu.sqlite3")).unwrap();
        for (index, outcome) in ["denied", "confirmed"].into_iter().enumerate() {
            repo.record_owner_presence(&OwnerPresenceRecord {
                record_id: format!("opr_{index}"),
                recorded_at: Utc::now(),
                action: "budget.create".to_string(),
                reason: "create a 5.00 budget for design-agent".to_string(),
                outcome: outcome.to_string(),
                method: (outcome == "confirmed").then(|| "fixture".to_string()),
                client: "cli".to_string(),
                targets: vec!["agt_1".to_string()],
                window_record_id: None,
            })
            .unwrap();
        }
        let records = repo.list_owner_presence_records(10).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].record_id, "opr_1");
        assert_eq!(records[0].method.as_deref(), Some("fixture"));
        assert_eq!(records[1].outcome, "denied");
        assert_eq!(records[1].targets, vec!["agt_1".to_string()]);
        assert_eq!(repo.list_owner_presence_records(1).unwrap().len(), 1);
    }
}
