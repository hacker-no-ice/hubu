//! Explicit public history projections. Never serialize persisted evidence,
//! capabilities, operation keys, or provider-controlled strings wholesale.
use super::*;
use hubu_core::{ledger::TransactionRecord, persistence::HistorySnapshot};
use sha2::{Digest, Sha256};

const VERSION: &str = "hubu-history-v1";

fn digest(value: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(value.as_bytes()))
}
fn reference(value: Option<&str>) -> Value {
    value
        .map(|v| json!({"digest": digest(v), "redacted": true}))
        .unwrap_or(Value::Null)
}
pub(super) fn decode(value: &str) -> Result<String> {
    let mut bytes = Vec::new();
    let mut input = value.bytes();
    while let Some(b) = input.next() {
        match b {
            b'+' => bytes.push(b' '),
            b'%' => {
                let a = input.next().and_then(|v| (v as char).to_digit(16));
                let b = input.next().and_then(|v| (v as char).to_digit(16));
                bytes.push(match (a, b) {
                    (Some(a), Some(b)) => (a * 16 + b) as u8,
                    _ => return Err(anyhow!("invalid query encoding")),
                });
            }
            _ => bytes.push(b),
        }
    }
    String::from_utf8(bytes).map_err(|_| anyhow!("invalid query encoding"))
}
fn query(request: &HttpRequest, allowed: &[&str]) -> Result<BTreeMap<String, String>> {
    let mut query = BTreeMap::new();
    for (key, value) in &request.query {
        let key = decode(key)?;
        if !allowed.contains(&key.as_str()) || query.contains_key(&key) {
            return Err(anyhow!("unknown or duplicate history query parameter"));
        }
        let value = decode(value)?;
        if value.is_empty() {
            return Err(anyhow!("empty history query parameter"));
        }
        query.insert(key, value);
    }
    Ok(query)
}
struct Selection {
    agent: Option<AgentId>,
    account: Option<hubu_common::ids::AgentAccountId>,
    budget: Option<BudgetId>,
}
fn selection(
    query: &BTreeMap<String, String>,
    user: &UserContext,
    state: &ServerState,
) -> Result<Selection> {
    let mut agent = query
        .get("agent_id")
        .map(|id| resolve_agent_id_for_user(id, user, state))
        .transpose()?;
    let account = if let Some(id) = query.get("account_id") {
        let registration = state
            .registration
            .lock()
            .map_err(|_| anyhow!("registration lock poisoned"))?;
        let account = registration
            .account_for_pub_id(id)?
            .filter(|a| a.owner_user_id == user.user_id)
            .ok_or_else(|| anyhow!("unknown owned account"))?;
        if agent.as_ref().is_some_and(|a| a != &account.agent_id) {
            return Err(anyhow!("account does not belong to selected agent"));
        }
        agent = Some(account.agent_id.clone());
        Some(account.id)
    } else {
        None
    };
    let budget = query
        .get("budget_id")
        .map(|id| resolve_budget_id_for_user(id, user, state))
        .transpose()?;
    if budget.is_some() && !query.contains_key("agent_id") {
        return Err(anyhow!("budget history requires agent_id"));
    }
    Ok(Selection {
        agent,
        account,
        budget,
    })
}
fn snapshot(state: &ServerState, owner: &UserId) -> Result<HistorySnapshot> {
    Ok(state
        .governance
        .lock()
        .map_err(|_| anyhow!("governance lock poisoned"))?
        .history_snapshot(owner)?)
}
fn validate_budget(selection: &Selection, data: &HistorySnapshot) -> Result<()> {
    if let Some(id) = &selection.budget {
        if !data
            .budgets
            .iter()
            .any(|b| &b.id == id && Some(&b.agent_id) == selection.agent.as_ref())
        {
            return Err(anyhow!("unknown owned agent budget"));
        }
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u32,
    binding: String,
    after: (String, String),
    ceiling: (String, String),
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn unhex(value: &str) -> Result<Vec<u8>> {
    if value.len() > 4096 || !value.len().is_multiple_of(2) || !value.is_ascii() {
        return Err(anyhow!("invalid history cursor"));
    }
    (0..value.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&value[i..i + 2], 16).map_err(|_| anyhow!("invalid history cursor"))
        })
        .collect()
}
fn page(
    mut records: Vec<Value>,
    query: &BTreeMap<String, String>,
    owner: &UserId,
    kind: &str,
) -> Result<(Vec<Value>, Option<String>)> {
    let limit = query
        .get("limit")
        .map(|v| v.parse::<usize>())
        .transpose()?
        .unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(anyhow!("history limit must be between 1 and 100"));
    }
    let filters: BTreeMap<_, _> = query
        .iter()
        .filter(|(k, _)| k.as_str() != "cursor" && k.as_str() != "limit")
        .collect();
    let binding = digest(&serde_json::to_string(&(owner, kind, filters))?);
    let key = |r: &Value| {
        (
            DateTime::parse_from_rfc3339(r["created_at"].as_str().unwrap())
                .expect("history projections contain valid timestamps")
                .with_timezone(&Utc)
                .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
            r["id"].as_str().unwrap().to_owned(),
        )
    };
    records.sort_by_key(|r| std::cmp::Reverse(key(r)));
    let cursor = query
        .get("cursor")
        .map(|v| -> Result<Cursor> {
            serde_json::from_slice(&unhex(v)?).map_err(|_| anyhow!("invalid history cursor"))
        })
        .transpose()?;
    let ceiling = if let Some(cursor) = &cursor {
        if cursor.version != 1 || cursor.binding != binding {
            return Err(anyhow!("history cursor does not match query"));
        }
        records.retain(|r| key(r) < cursor.after && key(r) <= cursor.ceiling);
        cursor.ceiling.clone()
    } else {
        records.first().map(key).unwrap_or_default()
    };
    let more = records.len() > limit;
    records.truncate(limit);
    let next = if more {
        Some(hex(&serde_json::to_vec(&Cursor {
            version: 1,
            binding,
            after: key(records.last().unwrap()),
            ceiling,
        })?))
    } else {
        None
    };
    Ok((records, next))
}

fn owned_metadata<'a>(
    record: &'a TransactionRecord,
    data: &HistorySnapshot,
) -> Option<&'a hubu_core::ledger::TransactionMetadata> {
    record
        .metadata
        .as_ref()
        .filter(|m| match (&m.agent_id, &m.agent_account_id) {
            (Some(agent), Some(account)) => data.owned_agent_accounts.get(agent) == Some(account),
            (None, None) => true,
            _ => false,
        })
}
fn linked_context<'a>(
    record: &'a TransactionRecord,
    data: &HistorySnapshot,
) -> Option<&'a hubu_core::ledger::GovernedSpendContext> {
    let meta = owned_metadata(record, data)?;
    meta.context.as_ref().filter(|c| {
        meta.agent_id.as_ref() == Some(&c.agent_id)
            && meta.agent_account_id.as_ref() == Some(&c.agent_account_id)
            && data
                .budgets
                .iter()
                .any(|b| b.id == c.budget_id && b.agent_id == c.agent_id)
            && data.decisions.iter().any(|d| {
                d.id == c.spend_decision_id
                    && d.request.agent_id == c.agent_id
                    && d.request.agent_account_id == c.agent_account_id
            })
    })
}

fn transaction(
    record: &TransactionRecord,
    data: &HistorySnapshot,
    state: &ServerState,
) -> Result<Value> {
    let meta = record.metadata.as_ref();
    let context = linked_context(record, data);
    let agent = owned_metadata(record, data).and_then(|m| m.agent_id.as_ref());
    let account = owned_metadata(record, data).and_then(|m| m.agent_account_id.as_ref());
    let ids = match (agent, account) {
        (Some(a), Some(b)) => Some(public_ids_for_agent_account(a, b, state)?),
        _ => None,
    };
    let decision =
        context.and_then(|c| data.decisions.iter().find(|d| d.id == c.spend_decision_id));
    let claim = decision.and_then(|d| {
        data.claims
            .iter()
            .find(|c| c.agent_id == d.request.agent_id && c.operation_key == d.operation_key)
    });
    let provider = decision
        .and_then(|d| d.request.execution_scope.as_ref())
        .and_then(|scope| {
            trusted_execution_scope_catalog()
                .into_iter()
                .find(|s| s == scope)
        })
        .map(|s| {
            json!({"provider":s.provider,
        "billing_merchant":s.billing_merchant})
        });
    // Decimal coefficient strings are data, not arbitrary evidence text.
    for entry in &record.entries {
        entry.amount.scaled()?;
    }
    if let Some(metadata) = meta {
        metadata.effective_cost.scaled()?;
    }
    let rounding = meta
        .and_then(|m| m.rounding_delta_amount.as_deref())
        .map(|v| v.parse::<i128>())
        .transpose()?
        .map(|v| v.to_string());
    let mut missing_evidence: Vec<&str> = record
        .missing_evidence
        .iter()
        .chain(meta.into_iter().flat_map(|m| m.missing_evidence.iter()))
        .filter(|s| {
            [
                "provider",
                "billing_merchant",
                "purpose",
                "source_evidence",
                "governed_context",
            ]
            .contains(&s.as_str())
        })
        .map(String::as_str)
        .collect();
    if meta.and_then(|m| m.context.as_ref()).is_some() && context.is_none() {
        missing_evidence.push("invalid_governed_context");
    }
    let owned_transaction = |id: &&hubu_common::ids::LedgerTransactionId| {
        data.transactions.iter().any(|t| &t.id == *id)
    };
    let entries: Vec<_> = record.entries.iter().map(|e| json!({"id":e.id,
        "account_id":e.account_id,
        "account_kind":e.account_kind,
        "direction":match e.direction {hubu_wallet::LedgerDirection::Debit=>"debit",hubu_wallet::LedgerDirection::Credit=>"credit"},
        "amount":e.amount})).collect();
    Ok(json!({"id":record.id,
        "created_at":record.created_at.to_rfc3339(),
        "kind":record.kind.as_str(),
        "agent_id":ids.as_ref().map(|i|&i.1),
        "account_id":ids.as_ref().map(|i|&i.0),
        "budget_id":context.map(|c|public_budget_id(&c.budget_id)),
        "budget_version_id":context.map(|c|public_budget_version_id(&c.budget_version_id)),
        "workflow_id":context.map(|c|c.spend_decision_id.to_string()),
        "claim_id":claim.map(|c|c.id.to_string()),
        "settlement_id":claim.and_then(|c|c.settlement_id.as_ref()).map(ToString::to_string),
        "provider":provider.as_ref().map(|p|&p["provider"]),
        "billing_merchant":provider.as_ref().map(|p|&p["billing_merchant"]),
        "effective_cost":meta.map(|m|&m.effective_cost),
        "cost_semantics":meta.map(|m|if m.kind==hubu_core::ledger::TransactionKind::Adjustment {"corrected_total"} else {"original_total"}),
        "purpose_reference":reference(meta.and_then(|m|m.purpose.as_deref())),
        "budget_charge_delta_cents":meta.and_then(|m|m.budget_charge_delta_cents),
        "rounding_delta_amount":rounding,
        "original_transaction_id":meta.and_then(|m|m.original_transaction_id.as_ref()).filter(owned_transaction),
        "previous_transaction_id":meta.and_then(|m|m.previous_transaction_id.as_ref()).filter(owned_transaction),
        "source_reference":reference(meta.map(|m|m.source_key.as_str())),
        "adjustment_evidence_reference":reference(meta.and_then(|m|m.adjustment_evidence.as_deref())),
        "legacy_backfill":meta.is_some_and(|m|m.legacy_backfill),
        "coverage":{"has_budget_context":context.is_some(),
        "missing_evidence_count":record.missing_evidence.len()+meta.map(|m|m.missing_evidence.len()).unwrap_or_default()+usize::from(meta.and_then(|m|m.context.as_ref()).is_some() && context.is_none()),
        "missing_evidence":missing_evidence,
        "raw_evidence_redacted":true},
        "entries":entries}))
}

pub(super) fn ledger(request: &HttpRequest, state: &ServerState) -> Result<Value> {
    let query = query(
        request,
        &["agent_id", "account_id", "budget_id", "limit", "cursor"],
    )?;
    let user = authenticated_user_context(state)?;
    let selected = selection(&query, &user, state)?;
    let data = snapshot(state, &user.user_id)?;
    validate_budget(&selected, &data)?;
    let matching: Vec<_> = data
        .transactions
        .iter()
        .filter(|t| {
            let m = owned_metadata(t, &data);
            let c = linked_context(t, &data);
            selected
                .agent
                .as_ref()
                .is_none_or(|a| m.and_then(|m| m.agent_id.as_ref()) == Some(a))
                && selected
                    .account
                    .as_ref()
                    .is_none_or(|a| m.and_then(|m| m.agent_account_id.as_ref()) == Some(a))
                && selected
                    .budget
                    .as_ref()
                    .is_none_or(|b| c.is_some_and(|c| &c.budget_id == b))
        })
        .collect();
    let coverage = if let Some(id) = &selected.budget {
        let budget = data.budgets.iter().find(|b| &b.id == id).unwrap();
        let balance = data
            .balances
            .iter()
            .find(|b| &b.budget_id == id)
            .ok_or_else(|| anyhow!("missing budget balance"))?;
        // Full-budget sum, independently of page size and account filter.
        let total: i128 = data
            .transactions
            .iter()
            .filter(|t| {
                linked_context(t, &data).is_some_and(|c| {
                    &c.budget_id == id && Some(&c.agent_id) == selected.agent.as_ref()
                })
            })
            .filter_map(|t| t.metadata.as_ref())
            .filter_map(|m| m.budget_charge_delta_cents)
            .map(i128::from)
            .sum();
        let total = i64::try_from(total)?;
        json!({"budget_id":public_budget_id(id),
        "agent_id":query.get("agent_id"),
        "currency":budget.currency.to_string(),
        "consumed_amount_cents":balance.consumed_amount_cents,
        "recorded_budget_charges_cents":total,
        "unaccounted_consumption_cents":balance.consumed_amount_cents.checked_sub(total).ok_or_else(||anyhow!("coverage overflow"))?,
        "pending_hold_count":data.holds.iter().filter(|h|&h.budget_id==id && matches!(h.status,BudgetHoldStatus::Frozen|BudgetHoldStatus::Claimed)).count(),
        "owner_transactions_without_budget_context":data.transactions.iter().filter(|t|linked_context(t,&data).is_none()).count(),
        "historical_wallet_reconciliation_complete":false})
    } else {
        Value::Null
    };
    let records = matching
        .into_iter()
        .map(|t| transaction(t, &data, state))
        .collect::<Result<Vec<_>>>()?;
    let (transactions, next_cursor) = page(records, &query, &user.user_id, "ledger")?;
    Ok(json!({"schema_version":VERSION,
        "transactions":transactions,
        "coverage":coverage,
        "next_cursor":next_cursor}))
}

fn workflow(
    decision: &SpendDecisionRecord,
    data: &HistorySnapshot,
    state: &ServerState,
    now: DateTime<Utc>,
) -> Result<Value> {
    let (account, agent) = public_ids_for_agent_account(
        &decision.request.agent_id,
        &decision.request.agent_account_id,
        state,
    )?;
    let token = data
        .tokens
        .iter()
        .find(|t| t.spend_decision_id == decision.id);
    let hold = data
        .holds
        .iter()
        .find(|h| h.spend_decision_id == decision.id);
    let claim = token.and_then(|t| data.claims.iter().find(|c| c.spend_auth_token_id == t.id));
    let receipt = claim.and_then(|c| data.receipts.iter().find(|r| r.claim_id == c.id));
    let status = if let Some(c) = claim {
        match c.status {
            SpendExecutorClaimStatus::Settled => "settled",
            SpendExecutorClaimStatus::Released => "released",
            SpendExecutorClaimStatus::Claimed if c.expires_at <= now => "reconciliation_required",
            _ => "claimed",
        }
    } else if let Some(h) = hold {
        match h.status {
            BudgetHoldStatus::Settled => "settled",
            BudgetHoldStatus::Released => "released",
            BudgetHoldStatus::Expired => "expired",
            _ if h.expires_at <= now => "expired",
            _ => "authorized",
        }
    } else {
        match data.authorization_outcomes.get(&decision.id) {
            Some(SpendAuthorizationDecision::PendingApproval) => "needs_approval",
            Some(SpendAuthorizationDecision::Denied) => "denied",
            Some(SpendAuthorizationDecision::Allowed) => "authorized",
            None => "unknown",
        }
    };
    let provider = decision
        .request
        .execution_scope
        .as_ref()
        .and_then(|scope| {
            trusted_execution_scope_catalog()
                .into_iter()
                .find(|s| s == scope)
        })
        .map(|s| {
            json!({"provider":s.provider,
        "billing_merchant":s.billing_merchant})
        });
    let transactions: Vec<_> = data
        .transactions
        .iter()
        .filter(|t| linked_context(t, data).is_some_and(|c| c.spend_decision_id == decision.id))
        .map(|t| t.id.to_string())
        .collect();
    let receipt = receipt.map(|r| {
        json!({"settlement_id":r.settlement_id,
        "created_at":r.created_at.to_rfc3339(),
        "authorized_max_cents":r.authorized_max_cents,
        "actual_vendor_cost":{"amount":r.receipt.actual_vendor_cost.amount.to_string(),
        "scale":r.receipt.actual_vendor_cost.scale,
        "currency":r.receipt.actual_vendor_cost.currency.to_string()},
        "budget_charge_cents":r.budget_charge_cents,
        "released_amount_cents":r.released_amount_cents,
        "overrun_amount_cents":r.overrun_amount_cents,
        "currency":r.currency.to_string(),
        "provider_request_reference":reference(Some(&r.receipt.provider_request_id)),
        "artifact_reference":reference(Some(&r.receipt.artifact_reference)),
        "price_model_reference":reference(Some(&r.receipt.price_model_snapshot.to_string()))})
    });
    Ok(json!({"id":decision.id,
        "workflow_id":decision.id,
        "decision_id":decision.id,
        "created_at":decision.created_at.to_rfc3339(),
        "revision":decision.revision,
        "agent_id":agent,
        "account_id":account,
        "decision":data.authorization_outcomes.get(&decision.id).map(|outcome|match outcome {SpendAuthorizationDecision::Allowed=>"allow",SpendAuthorizationDecision::PendingApproval=>"needs_approval",SpendAuthorizationDecision::Denied=>"deny"}),
        "policy_decision":effect_name(decision.evaluation.decision),
        "status":status,
        "authorized_max_cents":decision.request.amount_cents,
        "currency":decision.request.currency.to_string(),
        "provider":provider.as_ref().map(|p|&p["provider"]),
        "billing_merchant":provider.as_ref().map(|p|&p["billing_merchant"]),
        "budget_hold":hold.map(|h|json!({"id":h.id,
        "budget_id":public_budget_id(&h.budget_id),
        "budget_version_id":public_budget_version_id(&h.budget_version_id),
        "amount_cents":h.amount_cents,
        "currency":h.currency.to_string(),
        "status":budget_hold_status_name(&h.status),
        "expires_at":h.expires_at.to_rfc3339()})),
        "claim":claim.map(|c|json!({"id":c.id,
        "status":match c.status {SpendExecutorClaimStatus::Claimed=>"claimed",SpendExecutorClaimStatus::Settled=>"settled",SpendExecutorClaimStatus::Released=>"released"},
        "claimed_at":c.claimed_at.to_rfc3339(),
        "expires_at":c.expires_at.to_rfc3339(),
        "finalized_at":c.finalized_at.map(|t|t.to_rfc3339()),
        "reconciliation_required":c.status==SpendExecutorClaimStatus::Claimed && c.expires_at<=now,
        "reconciled_at":c.reconciled_at.map(|t|t.to_rfc3339()),
        "reconciliation_outcome":c.reconciled_at.map(|_|match c.status {SpendExecutorClaimStatus::Settled=>"vendor_billed",SpendExecutorClaimStatus::Released=>"vendor_did_not_bill",SpendExecutorClaimStatus::Claimed=>"pending"}),
        "lease_profile_reference":reference(Some(&c.lease_profile)),
        "reconciliation_evidence_reference":reference(c.reconciliation_evidence.as_deref()),
        "provider_reference":reference(c.provider_reference.as_deref())})),
        "receipt":receipt,
        "ledger_transaction_ids":transactions,
        "purpose_reference":reference(Some(&decision.request.reason)),
        "task_reference":reference(decision.request.task_id.as_deref()),
        "raw_evidence_redacted":true}))
}

pub(super) fn workflows(request: &HttpRequest, state: &ServerState, show: bool) -> Result<Value> {
    let allowed = if show {
        vec!["workflow_id", "agent_id", "operation_key"]
    } else {
        vec!["agent_id", "account_id", "status", "limit", "cursor"]
    };
    let query = query(request, &allowed)?;
    if show
        && !((query.len() == 1 && query.contains_key("workflow_id"))
            || (query.len() == 2
                && query.contains_key("agent_id")
                && query.contains_key("operation_key")))
    {
        return Err(anyhow!(
            "workflow lookup requires workflow_id or agent_id plus operation_key"
        ));
    }
    if let Some(status) = query.get("status") {
        if ![
            "authorized",
            "needs_approval",
            "unknown",
            "claimed",
            "settled",
            "released",
            "expired",
            "reconciliation_required",
        ]
        .contains(&status.as_str())
        {
            return Err(anyhow!("unknown workflow status"));
        }
    }
    let user = authenticated_user_context(state)?;
    let selected = selection(&query, &user, state)?;
    let data = snapshot(state, &user.user_id)?;
    let now = Utc::now();
    // An operation's latest authorization is its discovery row. Explicit public
    // decision IDs can still inspect an earlier non-denied revision.
    let mut latest: HashMap<(AgentId, String), &SpendDecisionRecord> = HashMap::new();
    for decision in &data.decisions {
        let key = (
            decision.request.agent_id.clone(),
            decision.operation_key.clone(),
        );
        if latest
            .get(&key)
            .is_none_or(|previous| previous.revision < decision.revision)
        {
            latest.insert(key, decision);
        }
    }
    let candidates: Vec<_> = if query.contains_key("workflow_id") {
        data.decisions.iter().collect()
    } else {
        latest.into_values().collect()
    };
    let records = candidates
        .into_iter()
        .filter(|d| {
            data.authorization_outcomes.get(&d.id) != Some(&SpendAuthorizationDecision::Denied)
        })
        .filter(|d| {
            selected
                .agent
                .as_ref()
                .is_none_or(|a| &d.request.agent_id == a)
                && selected
                    .account
                    .as_ref()
                    .is_none_or(|a| &d.request.agent_account_id == a)
        })
        .filter(|d| {
            query
                .get("workflow_id")
                .is_none_or(|id| d.id.to_string() == *id)
                && query
                    .get("operation_key")
                    .is_none_or(|key| &d.operation_key == key)
        })
        .map(|d| workflow(d, &data, state, now))
        .collect::<Result<Vec<_>>>()?;
    if show {
        return match records.as_slice() {
            [record] => Ok(json!({"schema_version":VERSION,
        "workflow":record})),
            _ => Err(anyhow!("unknown owned workflow")),
        };
    }
    let records = records
        .into_iter()
        .filter(|r| {
            query
                .get("status")
                .is_none_or(|s| r["status"].as_str() == Some(s))
        })
        .collect();
    let (workflows, next_cursor) = page(records, &query, &user.user_id, "workflows")?;
    Ok(json!({"schema_version":VERSION,
        "workflows":workflows,
        "next_cursor":next_cursor}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keyset_pages_bind_owner_filters_and_handle_ties_and_newer_rows() {
        let owner = UserId::new();
        let rows = vec![
            json!({"id":"a",
        "created_at":"2026-09-22T00:00:00Z"}),
            json!({"id":"b",
        "created_at":"2026-09-22T00:00:00Z"}),
            json!({"id":"c",
        "created_at":"2026-09-21T00:00:00Z"}),
        ];
        let mut query = BTreeMap::from([
            ("limit".into(), "1".into()),
            ("agent_id".into(), "agent".into()),
        ]);
        let (first, cursor) = page(rows.clone(), &query, &owner, "ledger").unwrap();
        assert_eq!(first[0]["id"], "b");
        query.insert("cursor".into(), cursor.unwrap());
        let mut newer = rows.clone();
        newer.push(json!({"id":"new",
        "created_at":"2026-09-23T00:00:00Z"}));
        let (second, cursor) = page(newer, &query, &owner, "ledger").unwrap();
        assert_eq!(second[0]["id"], "a");
        assert!(page(rows.clone(), &query, &UserId::new(), "ledger").is_err());
        assert!(page(rows.clone(), &query, &owner, "workflows").is_err());
        let mut changed = query.clone();
        changed.insert("agent_id".into(), "other".into());
        assert!(page(rows.clone(), &changed, &owner, "ledger").is_err());
        query.insert("cursor".into(), cursor.unwrap());
        let (last, cursor) = page(rows, &query, &owner, "ledger").unwrap();
        assert_eq!(last[0]["id"], "c");
        assert!(cursor.is_none());
        assert!(unhex("☃").is_err());
        assert!(unhex("zz").is_err());
    }
    #[test]
    fn fractional_second_and_timezone_spelling_do_not_change_page_order() {
        let rows = vec![
            json!({"id":"zero","created_at":"2026-09-22T00:00:00Z"}),
            json!({"id":"half","created_at":"2026-09-22T00:00:00.5+00:00"}),
            json!({"id":"quarter","created_at":"2026-09-21T17:00:00.250-07:00"}),
        ];
        let owner = UserId::new();
        let mut query = BTreeMap::from([("limit".into(), "1".into())]);
        let (first, cursor) = page(rows.clone(), &query, &owner, "ledger").unwrap();
        assert_eq!(first[0]["id"], "half");
        query.insert("cursor".into(), cursor.unwrap());
        let (second, cursor) = page(rows.clone(), &query, &owner, "ledger").unwrap();
        assert_eq!(second[0]["id"], "quarter");
        query.insert("cursor".into(), cursor.unwrap());
        let (last, cursor) = page(rows, &query, &owner, "ledger").unwrap();
        assert_eq!(last[0]["id"], "zero");
        assert!(cursor.is_none());
    }
}
