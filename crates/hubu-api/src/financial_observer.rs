//! Independent evidence observation; it owns no spend/budget manager or provider.
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use chrono::{DateTime, Utc};
use hubu_core::{persistence::SqliteGovernanceRepository, telemetry::log_event};
use serde_json::json;

const OBSERVATION_INTERVAL: Duration = Duration::from_secs(30);

enum Signal {
    Stop,
    #[cfg(test)]
    Observe(DateTime<Utc>, mpsc::Sender<()>),
}

pub(super) struct FinancialObserver {
    signal: mpsc::Sender<Signal>,
    worker: Option<JoinHandle<()>>,
}
impl FinancialObserver {
    pub(super) fn start(
        repository: Arc<Mutex<SqliteGovernanceRepository>>,
    ) -> std::io::Result<Self> {
        Self::start_with_interval(repository, OBSERVATION_INTERVAL)
    }
    fn start_with_interval(
        repository: Arc<Mutex<SqliteGovernanceRepository>>,
        interval: Duration,
    ) -> std::io::Result<Self> {
        let (signal, receiver) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("hubu-financial-evidence".into())
            .spawn(move || {
                observe(&repository, Utc::now());
                loop {
                    match receiver.recv_timeout(interval) {
                        Ok(Signal::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => observe(&repository, Utc::now()),
                        #[cfg(test)]
                        Ok(Signal::Observe(at, done)) => {
                            observe(&repository, at);
                            let _ = done.send(());
                        }
                    }
                }
            })?;
        Ok(Self {
            signal,
            worker: Some(worker),
        })
    }
    #[cfg(test)]
    fn observe_now(&self) {
        self.observe_at(Utc::now());
    }
    #[cfg(test)]
    pub(super) fn observe_at(&self, at: DateTime<Utc>) {
        let (done, completed) = mpsc::channel();
        self.signal.send(Signal::Observe(at, done)).unwrap();
        completed.recv_timeout(Duration::from_secs(5)).unwrap();
    }
}
impl Drop for FinancialObserver {
    fn drop(&mut self) {
        let _ = self.signal.send(Signal::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn observe(repository: &Mutex<SqliteGovernanceRepository>, at: DateTime<Utc>) {
    let result = repository
        .lock()
        .map_err(|e| e.to_string())
        .and_then(|mut repo| {
            repo.observe_financial_lifecycle_at(at)
                .and_then(|_| repo.observe_all_lifecycle_coverage_inventories_at(at))
                .map_err(|e| e.to_string())
        });
    if let Err(error) = result {
        // A later tick retries. Lag is not bounded across downtime/DB errors.
        log_event(
            "error",
            "financial_lifecycle_observation_failed",
            json!({"error": error}),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hubu_core::{spend::LifecycleSource, user::UserManager};

    #[test]
    fn observer_activates_without_requests_and_stops_without_interval_wait() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("governance.sqlite");
        let owner = UserManager::open(&path)
            .unwrap()
            .ensure_default_user()
            .unwrap()
            .id;
        hubu_core::registration::RegistrationManager::open(&path).unwrap();
        let repo = Arc::new(Mutex::new(SqliteGovernanceRepository::open(&path).unwrap()));
        let observer =
            FinancialObserver::start_with_interval(Arc::clone(&repo), Duration::from_secs(3600))
                .unwrap();
        observer.observe_now();
        let snapshot = repo
            .lock()
            .unwrap()
            .lifecycle_snapshot(&owner, None)
            .unwrap();
        assert!(snapshot.events.is_empty());
        assert_eq!(snapshot.capture_markers.len(), 2);
        assert_eq!(snapshot.coverage_inventories.len(), 1);
        assert!(snapshot.coverage_inventories[0]
            .scopes
            .iter()
            .all(|scope| scope.gaps.is_empty() && scope.backlog_gaps.is_empty()));
        assert!(snapshot
            .capture_markers
            .iter()
            .any(|m| m.source == LifecycleSource::Reconciliation));
        observer.observe_now();
        assert_eq!(
            repo.lock()
                .unwrap()
                .lifecycle_snapshot(&owner, None)
                .unwrap()
                .watermark,
            snapshot.watermark
        );
        drop(observer); // explicit stop wakes the long interval and joins
        assert!(repo.lock().is_ok());
    }
    #[test]
    fn transient_observation_failure_retries_on_next_wake() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("retry.sqlite");
        let owner = UserManager::open(&path)
            .unwrap()
            .ensure_default_user()
            .unwrap()
            .id;
        hubu_core::registration::RegistrationManager::open(&path).unwrap();
        let repo = Arc::new(Mutex::new(SqliteGovernanceRepository::open(&path).unwrap()));
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TRIGGER reject_observation BEFORE INSERT ON spend_lifecycle_records BEGIN SELECT RAISE(ABORT,'transient observation failure'); END;").unwrap();
        let observer =
            FinancialObserver::start_with_interval(Arc::clone(&repo), Duration::from_secs(3600))
                .unwrap();
        observer.observe_now();
        assert!(repo
            .lock()
            .unwrap()
            .lifecycle_snapshot(&owner, None)
            .unwrap()
            .capture_markers
            .is_empty());
        connection
            .execute_batch("DROP TRIGGER reject_observation")
            .unwrap();
        observer.observe_now();
        assert_eq!(
            repo.lock()
                .unwrap()
                .lifecycle_snapshot(&owner, None)
                .unwrap()
                .capture_markers
                .len(),
            2
        );
        drop(observer);
    }
    #[test]
    fn inventory_failure_does_not_erase_financial_observation_and_retries_independently() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("inventory-retry.sqlite");
        let owner = UserManager::open(&path)
            .unwrap()
            .ensure_default_user()
            .unwrap()
            .id;
        hubu_core::registration::RegistrationManager::open(&path).unwrap();
        let repo = Arc::new(Mutex::new(SqliteGovernanceRepository::open(&path).unwrap()));
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TRIGGER reject_inventory BEFORE INSERT ON spend_lifecycle_records WHEN json_extract(NEW.payload_json,'$.record_type')='coverage_inventory' BEGIN SELECT RAISE(ABORT,'inventory observation failure'); END;").unwrap();
        let observer =
            FinancialObserver::start_with_interval(Arc::clone(&repo), Duration::from_secs(3600))
                .unwrap();
        observer.observe_now();
        let before = repo
            .lock()
            .unwrap()
            .lifecycle_snapshot(&owner, None)
            .unwrap();
        assert_eq!(before.capture_markers.len(), 2);
        assert!(before.coverage_inventories.is_empty());
        assert!(before.events.is_empty());
        connection
            .execute_batch("DROP TRIGGER reject_inventory")
            .unwrap();
        observer.observe_now();
        let after = repo
            .lock()
            .unwrap()
            .lifecycle_snapshot(&owner, None)
            .unwrap();
        assert_eq!(after.capture_markers, before.capture_markers);
        assert_eq!(after.coverage_inventories.len(), 1);
        assert!(after.events.is_empty());
        observer.observe_now();
        assert_eq!(
            repo.lock()
                .unwrap()
                .lifecycle_snapshot(&owner, None)
                .unwrap()
                .watermark,
            after.watermark
        );
        drop(observer);
    }
}
