//! Owner presence governance records (v0.3.0, spec section 9).
//!
//! One row per owner-presence decision: every prompt the server raised
//! (confirmed, denied, timed out, unavailable, rate limited, superseded) and
//! every tightening write that needed no prompt. The table is additive and never rewrites
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
    /// The exact reason text shown in the dialog (empty for tightening).
    pub reason: String,
    /// `confirmed`, `denied`, `timed_out`, `unavailable`, `rate_limited`,
    /// `superseded`, `not_required`, or `delegated`.
    pub outcome: String,
    /// `biometric`, `device_password`, `fixture`, or none.
    pub method: Option<String>,
    /// `portal`, `cli`, `agent`, or `unknown`.
    pub client: String,
    /// Public ids of the budget, policy, agent or authorization concerned.
    pub targets: Vec<String>,
    /// Governance override used in place of owner presence.
    pub override_action: Option<String>,
}

const OWNER_PRESENCE_SCHEMA: &str = "
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
    override_action TEXT
);
CREATE TABLE IF NOT EXISTS governance_overrides (
    action_kind TEXT PRIMARY KEY NOT NULL
);
";

impl SqliteGovernanceRepository {
    pub(super) fn init_owner_presence_schema(&mut self) -> Result<(), StorageError> {
        self.conn.execute_batch(OWNER_PRESENCE_SCHEMA)?;
        if !table_has_column(&self.conn, "owner_presence_records", "override_action")? {
            self.conn.execute(
                "ALTER TABLE owner_presence_records ADD COLUMN override_action TEXT",
                [],
            )?;
        }
        Ok(())
    }

    pub fn enabled_governance_overrides(&self) -> Result<Vec<String>, StorageError> {
        let mut stmt = self
            .conn
            .prepare("SELECT action_kind FROM governance_overrides")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        collect_rows(rows)
    }

    pub fn governance_override_enabled(&self, action: &str) -> Result<bool, StorageError> {
        self.conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM governance_overrides WHERE action_kind = ?1)",
                [action],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub fn set_governance_override(
        &mut self,
        action: &str,
        enabled: bool,
    ) -> Result<(), StorageError> {
        if enabled {
            self.conn.execute(
                "INSERT INTO governance_overrides (action_kind) VALUES (?1)
                 ON CONFLICT(action_kind) DO NOTHING",
                [action],
            )?;
        } else {
            self.conn.execute(
                "DELETE FROM governance_overrides WHERE action_kind = ?1",
                [action],
            )?;
        }
        Ok(())
    }

    pub fn record_owner_presence(
        &mut self,
        record: &OwnerPresenceRecord,
    ) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO owner_presence_records
             (record_id, recorded_at, action, reason, outcome, method, client, targets_json,
              override_action)
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
                record.override_action,
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
            "SELECT record_id, recorded_at, action, reason, outcome, method, client, targets_json,
                    override_action
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
                override_action: row.get(8)?,
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
                override_action: None,
                reason: "create a 5.00 budget for design-agent".to_string(),
                outcome: outcome.to_string(),
                method: (outcome == "confirmed").then(|| "fixture".to_string()),
                client: "cli".to_string(),
                targets: vec!["agt_1".to_string()],
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

    #[test]
    fn opening_an_existing_presence_schema_preserves_records_and_starts_strict() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hubu.sqlite3");
        let repo = SqliteGovernanceRepository::open(&path).unwrap();
        repo.conn
            .execute_batch(
                "DROP TABLE governance_overrides;
                 ALTER TABLE owner_presence_records DROP COLUMN override_action;
                 INSERT INTO owner_presence_records
                 (record_id, recorded_at, action, reason, outcome, client, targets_json)
                 VALUES ('opr_existing', '2026-11-20T00:00:00Z', 'budget.create',
                         'create a budget', 'confirmed', 'cli', '[]');",
            )
            .unwrap();
        drop(repo);

        let repo = SqliteGovernanceRepository::open(&path).unwrap();
        assert!(repo.enabled_governance_overrides().unwrap().is_empty());
        let records = repo.list_owner_presence_records(10).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record_id, "opr_existing");
        assert_eq!(records[0].override_action, None);
    }
}
