//! Catalog transition tracking, probe ordering, and sanitized notifications.

use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    Mutex,
};

use chrono::{SecondsFormat, Utc};
use serde_json::{json, Value};

use crate::{
    capability::{BackendState, CapabilitySnapshot},
    diagnostics::tool_availability,
    DOMAIN_TOOLS,
};

#[derive(Debug)]
pub(super) struct CatalogTracker {
    callable_tools: Vec<&'static str>,
}

#[derive(Clone, Copy, Debug, Default)]
struct AppliedProbes {
    hubu: u64,
    gongbu: u64,
}

#[derive(Debug)]
pub(super) struct TransitionState {
    tracker: Mutex<CatalogTracker>,
    pending: AtomicUsize,
    probe_sequence: AtomicU64,
    applied_probes: Mutex<AppliedProbes>,
}

impl TransitionState {
    pub(super) fn new(snapshot: &CapabilitySnapshot) -> Self {
        Self {
            tracker: Mutex::new(CatalogTracker::new(snapshot)),
            pending: AtomicUsize::new(0),
            probe_sequence: AtomicU64::new(0),
            applied_probes: Mutex::new(AppliedProbes::default()),
        }
    }

    pub(super) fn next_probe_id(&self) -> u64 {
        self.probe_sequence.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub(super) fn apply_full(
        &self,
        target: &Mutex<CapabilitySnapshot>,
        probe_id: u64,
        refreshed: CapabilitySnapshot,
    ) {
        let mut applied = self
            .applied_probes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut snapshot = target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut changed = false;
        if probe_id >= applied.hubu {
            snapshot.hubu = refreshed.hubu;
            applied.hubu = probe_id;
            changed = true;
        }
        if probe_id >= applied.gongbu {
            snapshot.gongbu = refreshed.gongbu;
            applied.gongbu = probe_id;
            changed = true;
        }
        if changed {
            snapshot.generated_at = refreshed.generated_at;
            self.observe(&snapshot);
        }
    }

    pub(super) fn apply_hubu(
        &self,
        target: &Mutex<CapabilitySnapshot>,
        probe_id: u64,
        refreshed: crate::capability::BackendReport,
    ) {
        let mut applied = self
            .applied_probes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if probe_id < applied.hubu {
            return;
        }
        let mut snapshot = target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        snapshot.generated_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        snapshot.hubu = refreshed;
        applied.hubu = probe_id;
        self.observe(&snapshot);
    }

    pub(super) fn apply_gongbu(
        &self,
        target: &Mutex<CapabilitySnapshot>,
        probe_id: u64,
        refreshed: crate::capability::BackendReport,
    ) {
        let mut applied = self
            .applied_probes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if probe_id < applied.gongbu {
            return;
        }
        let mut snapshot = target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        snapshot.generated_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        snapshot.gongbu = refreshed;
        applied.gongbu = probe_id;
        self.observe(&snapshot);
    }

    pub(super) fn mark_hubu_unavailable(&self, target: &Mutex<CapabilitySnapshot>, probe_id: u64) {
        let mut applied = self
            .applied_probes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut snapshot = target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        snapshot.generated_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        snapshot.hubu.state = BackendState::Unavailable;
        snapshot.hubu.reason_code = Some("health_unavailable");
        applied.hubu = probe_id;
        self.observe(&snapshot);
    }

    pub(super) fn reset(&self, snapshot: &CapabilitySnapshot) {
        *self
            .tracker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = CatalogTracker::new(snapshot);
        self.pending.store(0, Ordering::Release);
    }

    pub(super) fn take_pending(&self) -> usize {
        self.pending.swap(0, Ordering::AcqRel)
    }

    fn observe(&self, snapshot: &CapabilitySnapshot) {
        let changed = self
            .tracker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .observe(snapshot);
        if changed {
            self.pending.fetch_add(1, Ordering::Release);
        }
    }
}

impl CatalogTracker {
    pub(super) fn new(snapshot: &CapabilitySnapshot) -> Self {
        Self {
            callable_tools: callable_tools(snapshot),
        }
    }

    pub(super) fn observe(&mut self, snapshot: &CapabilitySnapshot) -> bool {
        let callable_tools = callable_tools(snapshot);
        if callable_tools == self.callable_tools {
            return false;
        }
        self.callable_tools = callable_tools;
        true
    }
}

fn callable_tools(snapshot: &CapabilitySnapshot) -> Vec<&'static str> {
    let mut tools = vec![
        "hubu_unified_capabilities",
        hubu_feedback::GUIDANCE_TOOL,
        hubu_feedback::PREPARE_TOOL,
    ];
    tools.extend(DOMAIN_TOOLS.iter().filter_map(|(name, owner)| {
        tool_availability(name, *owner, snapshot)
            .is_ok()
            .then_some(*name)
    }));
    tools.sort_unstable();
    tools
}

pub(super) fn tools_list_changed_notification() -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/tools/list_changed"
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{BackendReport, BackendState, CapabilitySnapshot, ContractVersions};

    fn report(state: BackendState, gongbu: bool) -> BackendReport {
        BackendReport {
            state,
            product_version: None,
            source_commit: None,
            api_schema_version: gongbu.then_some(2),
            mcp_schema_version: gongbu.then_some(2),
            contract_versions: ContractVersions { executor: None },
            reason_code: None,
        }
    }

    fn snapshot(hubu: BackendState, gongbu: BackendState) -> CapabilitySnapshot {
        CapabilitySnapshot {
            generated_at: String::new(),
            hubu: report(hubu, false),
            gongbu: report(gongbu, true),
        }
    }

    #[test]
    fn observes_only_catalog_affecting_transitions() {
        let mut tracker =
            CatalogTracker::new(&snapshot(BackendState::Available, BackendState::Available));
        assert!(!tracker.observe(&snapshot(BackendState::Available, BackendState::Available)));
        assert!(tracker.observe(&snapshot(BackendState::Available, BackendState::Degraded)));
        assert!(!tracker.observe(&snapshot(BackendState::Available, BackendState::Degraded)));
        assert!(tracker.observe(&snapshot(
            BackendState::Available,
            BackendState::Incompatible
        )));
        assert!(!tracker.observe(&snapshot(
            BackendState::Available,
            BackendState::Unavailable
        )));
    }

    #[test]
    fn notification_has_no_backend_payload() {
        assert_eq!(
            tools_list_changed_notification(),
            json!({
                "jsonrpc": "2.0",
                "method": "notifications/tools/list_changed"
            })
        );
    }
}
