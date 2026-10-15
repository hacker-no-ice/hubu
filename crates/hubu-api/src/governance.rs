//! Transport projection only: ownership, snapshots, filtering, totals and
//! backlog are evaluated by the shared core read in one SQL transaction.
use super::*;
use hubu_core::spend::{
    GovernanceBacklog, GovernanceCategory, GovernanceDetail, GovernanceFact, GovernanceOutcome,
    GovernanceQuery, GovernanceReadError, GovernanceRequestedTotal, GovernanceScope,
    GovernanceSummary, GovernanceUnsupportedSignal, LifecycleAmount, LifecycleBudgetEvidence,
    LifecycleEvidence, LifecyclePolicyEvidence,
};
use hubu_core::storage::StorageError;

const VERSION: &str = "hubu-governance-v1";
const RECORD_PREFIX: &str = "/governance/records/";

/// Retain the parsed request so malformed governance queries can still pass
/// through bearer/origin/host authentication before returning a fixed 400.
#[derive(thiserror::Error)]
#[error("invalid governance query")]
pub(super) struct InvalidQueryRequest {
    pub(super) request: HttpRequest,
}
impl std::fmt::Debug for InvalidQueryRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InvalidQueryRequest")
    }
}

pub(super) fn reject_parsed_query(request: &HttpRequest, state: &ServerState) -> HttpResponse {
    if let Err(error) = authenticate_request(request, state) {
        return HttpResponse {
            status: 401,
            body: json!({"error":error.to_string()}),
        };
    }
    error(400, "invalid governance query", "governance_invalid_query")
}

pub(super) fn is_read_path(path: &str) -> bool {
    matches!(path, "/governance/summary" | "/governance/records") || path.starts_with(RECORD_PREFIX)
}

/// Return a response directly so the generic route error handler cannot expose
/// private storage/JSON diagnostics or caller-controlled malformed token text.
pub(super) fn read(request: &HttpRequest, state: &ServerState) -> HttpResponse {
    match response(request, state) {
        Ok(Some(body)) => HttpResponse { status: 200, body },
        Ok(None) => error(
            404,
            "governance record not found",
            "governance_record_not_found",
        ),
        Err(GovernanceReadError::InvalidQuery) => {
            error(400, "invalid governance query", "governance_invalid_query")
        }
        Err(GovernanceReadError::Unavailable(_)) => error(
            500,
            "governance evidence is unavailable",
            "governance_unavailable",
        ),
    }
}
fn error(status: u16, message: &str, code: &str) -> HttpResponse {
    HttpResponse {
        status,
        body: json!({"error":message,"error_code":code}),
    }
}
fn unavailable() -> GovernanceReadError {
    GovernanceReadError::Unavailable(StorageError::InvalidData(
        "governance read context unavailable".into(),
    ))
}
fn response(
    request: &HttpRequest,
    state: &ServerState,
) -> std::result::Result<Option<Value>, GovernanceReadError> {
    let query = query(request)?;
    let owner = authenticated_user_context(state).map_err(|_| unavailable())?;
    let repository = state.governance.lock().map_err(|_| unavailable())?;
    let body = match request.path.as_str() {
        "/governance/summary" => {
            let summary = repository.governance_summary(&owner.user_id, &query)?;
            json!({"schema_version":VERSION,"filters":filters(&query,&summary.scope),"summary":summary_value(&summary)})
        }
        "/governance/records" => {
            let page = repository.governance_list(&owner.user_id, &query)?;
            json!({"schema_version":VERSION,"filters":filters(&query,&page.summary.scope),"summary":summary_value(&page.summary),"records":page.facts.iter().map(fact).collect::<Vec<_>>(),"next_cursor":page.next_cursor})
        }
        path => {
            let id = path
                .strip_prefix(RECORD_PREFIX)
                .ok_or(GovernanceReadError::InvalidQuery)?;
            if id.is_empty() || id.contains('/') {
                return Err(GovernanceReadError::InvalidQuery);
            }
            let Some(GovernanceDetail {
                fact: record,
                summary,
            }) = repository.governance_detail(&owner.user_id, id, &query)?
            else {
                return Ok(None);
            };
            json!({"schema_version":VERSION,"filters":filters(&query,&summary.scope),"summary":summary_value(&summary),"record":fact(&record)})
        }
    };
    Ok(Some(body))
}

fn query(request: &HttpRequest) -> std::result::Result<GovernanceQuery, GovernanceReadError> {
    const ALLOWED: &[&str] = &[
        "agent_id",
        "account_id",
        "categories",
        "outcomes",
        "occurred_from",
        "occurred_before",
        "snapshot",
        "cursor",
        "limit",
    ];
    let mut fields = BTreeMap::new();
    for (key, value) in &request.query {
        // The raw parser also rejects exact repeated keys before its HashMap
        // can erase them; this catches differently encoded aliases as well.
        let key = history::decode(key).map_err(|_| GovernanceReadError::InvalidQuery)?;
        if !ALLOWED.contains(&key.as_str()) || fields.contains_key(&key) {
            return Err(GovernanceReadError::InvalidQuery);
        }
        let value = history::decode(value).map_err(|_| GovernanceReadError::InvalidQuery)?;
        if value.is_empty() {
            return Err(GovernanceReadError::InvalidQuery);
        }
        fields.insert(key, value);
    }
    let parse_time =
        |key: &str| -> std::result::Result<Option<DateTime<Utc>>, GovernanceReadError> {
            fields
                .get(key)
                .map(|value| {
                    DateTime::parse_from_rfc3339(value)
                        .map(|t| t.with_timezone(&Utc))
                        .map_err(|_| GovernanceReadError::InvalidQuery)
                })
                .transpose()
        };
    Ok(GovernanceQuery {
        agent_public_id: fields.get("agent_id").cloned(),
        account_public_id: fields.get("account_id").cloned(),
        categories: csv::<GovernanceCategory>(fields.get("categories"))?,
        outcomes: csv::<GovernanceOutcome>(fields.get("outcomes"))?,
        occurred_from: parse_time("occurred_from")?,
        occurred_before: parse_time("occurred_before")?,
        snapshot: fields.get("snapshot").cloned(),
        cursor: fields.get("cursor").cloned(),
        limit: fields
            .get("limit")
            .map(|value| {
                if !value.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(GovernanceReadError::InvalidQuery);
                }
                value.parse().map_err(|_| GovernanceReadError::InvalidQuery)
            })
            .transpose()?,
    })
}
fn csv<T: serde::de::DeserializeOwned + Ord>(
    value: Option<&String>,
) -> std::result::Result<Vec<T>, GovernanceReadError> {
    let Some(value) = value else {
        return Ok(vec![]);
    };
    let mut values = value
        .split(',')
        .map(|part| {
            serde_json::from_value::<T>(json!(part)).map_err(|_| GovernanceReadError::InvalidQuery)
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    values.sort();
    values.dedup();
    Ok(values)
}
fn filters(query: &GovernanceQuery, selected: &GovernanceScope) -> Value {
    json!({"agent_id":selected.agent_public_id,"account_id":selected.account_public_id,"categories":query.categories,"outcomes":query.outcomes,"occurred_from":query.occurred_from,"occurred_before":query.occurred_before})
}
fn scope(selected: &GovernanceScope) -> Value {
    json!({"agent_id":selected.agent_public_id,"account_id":selected.account_public_id})
}
fn money(value: &LifecycleAmount) -> Value {
    to_json(MoneyHttpResponse::new(value.amount, value.currency))
}
fn evidence<T>(value: &LifecycleEvidence<T>, map: impl FnOnce(&T) -> Value) -> Value {
    match value {
        LifecycleEvidence::Known { value } => json!({"coverage":"known","value":map(value)}),
        LifecycleEvidence::Unavailable { reason } => {
            json!({"coverage":"unavailable","reason":reason})
        }
    }
}
fn policy(value: &LifecyclePolicyEvidence) -> Value {
    json!({"policy_id":value.policy_id,"revision":value.revision,"payload_hash":value.payload_hash,"decisive_rule_ids":value.decisive_rule_ids,
        "amount_constraints":value.amount_constraints.iter().map(|c|json!({"rule_id":c.rule_id,"comparison":c.comparison,"configured":money(&c.configured),"evaluated":evidence(&c.evaluated,money)})).collect::<Vec<_>>()})
}
fn budget(value: &LifecycleBudgetEvidence) -> Value {
    json!({"budget_id":value.budget_id,"budget_version_id":value.budget_version_id,"requested":money(&value.requested),"limit":evidence(&value.limit,money),"available":evidence(&value.available,money),"consumed":evidence(&value.consumed,money),"frozen":evidence(&value.frozen,money)})
}
fn fact(value: &GovernanceFact) -> Value {
    json!({"record_id":value.event_public_id,"agent_id":value.agent_public_id,"account_id":value.account_public_id,"occurred_at":value.occurred_at,"recorded_at":value.recorded_at,
        "attempt_revision":value.attempt_revision,"category":value.category,"outcome":value.outcome,"source":value.source,"authorization_release_reason":value.authorization_release_reason,"reconciliation_resolution":value.reconciliation_resolution,"automatic_denial_reason":value.automatic_denial_reason,
        "requested_amount":evidence(&value.requested_amount,money),"execution_scope":evidence(&value.execution_scope,|s|json!({"provider":s.provider,"executor":s.executor,"capability":s.capability,"billing_merchant":s.billing_merchant})),"policy":evidence(&value.policy,policy),"budget":evidence(&value.budget,budget)})
}
fn requested(value: &GovernanceRequestedTotal) -> Value {
    json!({"known_attempts":value.known_attempts,"unknown_attempts":value.unknown_attempts,"per_currency":value.per_currency.iter().map(|m|MoneyHttpResponse::new_wide(m.units,m.currency)).collect::<Vec<_>>()})
}
fn backlog(value: &GovernanceBacklog) -> Value {
    json!({"scope":scope(&value.scope),"captured_evidence_as_of":value.captured_evidence_as_of,"pending_human_approvals":value.pending_human_approvals,"unresolved_reconciliations":value.unresolved_reconciliations})
}
fn summary_value(value: &GovernanceSummary) -> Value {
    json!({"scope":scope(&value.scope),"snapshot":{"token":value.snapshot.token,"watermark":value.snapshot.watermark,"captured_evidence_as_of":value.snapshot.captured_evidence_as_of},
        "fact_count":value.fact_count,"distinct_attempts":value.distinct_attempts,"distinct_operations":value.distinct_operations,"stage_counts_overlap":value.stage_counts_overlap,
        "stages":value.stages.iter().map(|s|json!({"outcome":s.outcome,"fact_count":s.fact_count})).collect::<Vec<_>>(),"automatic_denials":value.automatic_denials.iter().map(|s|json!({"reason":s.reason,"distinct_attempts":s.distinct_attempts})).collect::<Vec<_>>(),
        "requested":requested(&value.requested),"automatic_denied_requested":requested(&value.automatic_denied_requested),"backlog":backlog(&value.backlog),
        "record_coverage":value.record_coverage.iter().map(|c|json!({"category":c.category,"supported":c.supported,"complete_history":c.complete_history,"capture_declarations":c.capture_declarations.iter().map(|m|json!({"source":m.source,"state":m.state,"effective_at":m.effective_at,"recorded_at":m.recorded_at})).collect::<Vec<_>>()})).collect::<Vec<_>>(),
        "unsupported_signals":value.unsupported_signals.iter().map(|s|match s {GovernanceUnsupportedSignal::ProviderExecutionFailure=>"provider_execution_failure",GovernanceUnsupportedSignal::PreExpiryBillingUncertainty=>"pre_expiry_billing_uncertainty"}).collect::<Vec<_>>(),
        "explanation_coverage":value.explanation_coverage.iter().map(|c|json!({"category":c.category,"facts":c.facts,"known_requested_amount":c.known_requested_amount,"known_policy":c.known_policy,"known_budget":c.known_budget,"known_execution_scope":c.known_execution_scope})).collect::<Vec<_>>(),
        "legacy_coverage":{"observed_at":value.legacy_coverage.observed_at,"assessed":value.legacy_coverage.assessed,"unlinked_metrics":value.legacy_coverage.unlinked_metrics,"unlinked_human_approval_backlog":value.legacy_coverage.unlinked_human_approval_backlog,"unlinked_reconciliation_backlog":value.legacy_coverage.unlinked_reconciliation_backlog,"broader_attribution_unknown":value.legacy_coverage.broader_attribution_unknown}})
}
