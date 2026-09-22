use super::*;

fn read(state: &ServerState, path: &str) -> Value {
    let response = route(authenticated_get_request(path), state);
    assert_eq!(response.status, 200, "{}", response.body);
    response.body
}
fn settle(
    state: &ServerState,
    agent: &RegisterAgentHttpResponse,
    auth: &SpendHttpResponse,
    amount: i64,
) -> Value {
    claim_executor_spend(json!({"operation_key":auth.operation_key,"spend_auth_token_id":auth.auth_token_id,"account_id":agent.account_id,"amount_cents":auth.budget_hold.as_ref().unwrap().amount_cents,"merchant":"gongbu.image"}).to_string(),state).unwrap();
    let mut receipt = precise_settlement_receipt_json(amount, 3);
    receipt["artifact_reference"] =
        json!("https://vendor.example/artifact?token=VERY_PRIVATE_ARTIFACT");
    receipt["provider_request_id"] = json!("Bearer VERY_PRIVATE_PROVIDER");
    receipt["price_model_snapshot"]["credential"] = json!("VERY_PRIVATE_CREDENTIAL");
    finalize_executor_spend(
        json!({"operation_key":auth.operation_key,"agent_id":agent.agent_id,"receipt":receipt})
            .to_string(),
        state,
        true,
        None,
    )
    .unwrap()
}

#[test]
fn provider_history_is_canonical_precise_private_and_budget_scoped() {
    let (path, state, agent, auth) = setup_executor_authorization("history-precise");
    let budget = auth.budget_hold.as_ref().unwrap().budget_id.clone();
    let url = format!(
        "/ledger/transactions?agent_id={}&budget_id={budget}&limit=1",
        agent.agent_id
    );
    let empty = read(&state, &url);
    assert_eq!(empty["transactions"], json!([]));
    assert_eq!(empty["coverage"]["consumed_amount_cents"], 0);
    assert_eq!(empty["coverage"]["pending_hold_count"], 1);
    let settled = settle(&state, &agent, &auth, 1);
    let first = read(&state, &url);
    assert_eq!(first["transactions"].as_array().unwrap().len(), 1);
    let posting = &first["transactions"][0];
    assert_eq!(posting["kind"], "external_provider");
    assert_eq!(posting["effective_cost"]["amount"], "1");
    assert_eq!(posting["effective_cost"]["scale"], 3);
    assert_eq!(posting["budget_charge_delta_cents"], 1);
    assert_eq!(posting["workflow_id"], auth.decision_id);
    assert_eq!(posting["settlement_id"], settled["settlement_id"]);
    assert_eq!(first["coverage"]["recorded_budget_charges_cents"], 1);
    assert_eq!(first["coverage"]["unaccounted_consumption_cents"], 0);
    assert_eq!(read(&state, "/ledger")["transactions"], json!([]));
    let workflows = read(
        &state,
        &format!(
            "/spend/workflows?agent_id={}&status=settled",
            agent.agent_id
        ),
    );
    assert_eq!(workflows["workflows"].as_array().unwrap().len(), 1);
    let flow = &workflows["workflows"][0];
    assert_eq!(flow["ledger_transaction_ids"][0], posting["id"]);
    assert_eq!(flow["receipt"]["actual_vendor_cost"]["amount"], "1");
    assert_eq!(flow["receipt"]["artifact_reference"]["redacted"], true);
    for output in [&first, &workflows] {
        let text = output.to_string();
        for secret in [
            &auth.operation_key,
            "VERY_PRIVATE_ARTIFACT",
            "VERY_PRIVATE_PROVIDER",
            "VERY_PRIVATE_CREDENTIAL",
            "spend_auth_token_id",
            "source_evidence",
        ] {
            assert!(!text.contains(secret), "leaked {secret}");
        }
    }
    let shown = read(
        &state,
        &format!("/spend/workflows/show?workflow_id={}", auth.decision_id),
    );
    assert_eq!(shown["workflow"], *flow);
    // Replaying settlement must not create a duplicate posting or workflow.
    assert_eq!(settle(&state, &agent, &auth, 1), settled);
    assert_eq!(read(&state, &url)["transactions"], first["transactions"]);
    std::fs::remove_file(path).ok();
}

#[test]
fn budget_coverage_is_full_history_with_linked_signed_corrections() {
    let (path, state, agent, auth) = setup_executor_authorization("history-correction");
    settle(&state, &agent, &auth, 11);
    let original: hubu_common::ids::LedgerTransactionId = read(&state, "/ledger/transactions")
        ["transactions"][0]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let owner = authenticated_user_context(&state).unwrap().user_id;
    hubu_core::ledger::LedgerService::adjust(
        &mut state.budgets.lock().unwrap(),
        hubu_core::ledger::AccountingAdjustment {
            owner_user_id: owner,
            original_transaction_id: original.clone(),
            expected_previous_transaction_id: original.clone(),
            operation_key: "private-correction".into(),
            corrected_cost: hubu_core::ledger::ExactMoney {
                amount: "1".into(),
                scale: 3,
                currency: Currency::Usd,
            },
            reason: "private reason".into(),
            evidence: "private evidence".into(),
        },
    )
    .unwrap();
    let url = format!(
        "/ledger/transactions?agent_id={}&budget_id={}&limit=1",
        agent.agent_id,
        auth.budget_hold.as_ref().unwrap().budget_id
    );
    let first = read(&state, &url);
    assert_eq!(first["coverage"]["consumed_amount_cents"], 1);
    assert_eq!(first["coverage"]["recorded_budget_charges_cents"], 1);
    assert_eq!(first["transactions"][0]["kind"], "adjustment");
    assert_eq!(
        first["transactions"][0]["cost_semantics"],
        "corrected_total"
    );
    assert_eq!(first["transactions"][0]["budget_charge_delta_cents"], -1);
    assert_eq!(
        first["transactions"][0]["original_transaction_id"],
        original.to_string()
    );
    let second = read(
        &state,
        &format!("{url}&cursor={}", first["next_cursor"].as_str().unwrap()),
    );
    assert_eq!(second["coverage"], first["coverage"]);
    assert_eq!(second["transactions"][0]["kind"], "external_provider");
    assert!(second["next_cursor"].is_null());
    std::fs::remove_file(path).ok();
}

#[test]
fn owned_empty_budget_and_mismatched_filters_are_checked_before_results() {
    let (path, state, agent, auth) = setup_executor_authorization("history-ownership");
    let second = register_agent(
        json!({"name":"other-agent","version":"v1"}).to_string(),
        &state,
    )
    .unwrap();
    let empty = create_test_agent_budget(&state, &second.agent_id, 100);
    let empty = read(
        &state,
        &format!(
            "/ledger/transactions?agent_id={}&budget_id={}",
            second.agent_id, empty.budget.budget_id
        ),
    );
    assert_eq!(empty["transactions"], json!([]));
    assert_eq!(empty["coverage"]["consumed_amount_cents"], 0);
    assert_eq!(empty["coverage"]["unaccounted_consumption_cents"], 0);
    for url in [
        format!(
            "/ledger/transactions?agent_id={}&budget_id={}",
            second.agent_id,
            auth.budget_hold.as_ref().unwrap().budget_id
        ),
        format!(
            "/ledger/transactions?agent_id={}&account_id={}",
            agent.agent_id, second.account_id
        ),
        format!(
            "/ledger/transactions?budget_id={}",
            auth.budget_hold.as_ref().unwrap().budget_id
        ),
        "/ledger/transactions?owner_user_id=another".into(),
        "/spend/workflows/show?workflow_id=unknown".into(),
    ] {
        assert_ne!(
            route(authenticated_get_request(&url), &state).status,
            200,
            "{url}"
        );
    }
    assert_eq!(
        read(
            &state,
            &format!("/spend/workflows?account_id={}", second.account_id)
        )["workflows"],
        json!([])
    );
    std::fs::remove_file(path).ok();
}

#[test]
fn workflow_lookup_decodes_private_key_but_returns_only_public_identity() {
    let (path, state, agent, auth) = setup_executor_authorization("history + & % 雪");
    let encoded: String = auth
        .operation_key
        .bytes()
        .map(|b| format!("%{b:02X}"))
        .collect();
    let shown = read(
        &state,
        &format!(
            "/spend/workflows/show?agent_id={}&operation_key={encoded}",
            agent.agent_id
        ),
    );
    assert_eq!(shown["workflow"]["id"], auth.decision_id);
    assert!(!shown.to_string().contains(&auth.operation_key));
    assert!(!shown.to_string().contains("operation_key"));
    assert!(parse_request(
        "GET /spend/workflows?limit=1&%6cimit=2 HTTP/1.1\r\nHost: localhost\r\n\r\n"
    )
    .is_err());
    let mixed = format!(
        "/spend/workflows/show?workflow_id={}&agent_id={}",
        auth.decision_id, agent.agent_id
    );
    assert_ne!(route(authenticated_get_request(&mixed), &state).status, 200);
    claim_executor_spend(json!({"operation_key":auth.operation_key,"spend_auth_token_id":auth.auth_token_id,"account_id":agent.account_id,"amount_cents":500,"merchant":"gongbu.image"}).to_string(),&state).unwrap();
    finalize_executor_spend(
        json!({"operation_key":auth.operation_key,"agent_id":agent.agent_id}).to_string(),
        &state,
        false,
        None,
    )
    .unwrap();
    let released = read(
        &state,
        &format!(
            "/spend/workflows?agent_id={}&status=released",
            agent.agent_id
        ),
    );
    assert_eq!(released["workflows"].as_array().unwrap().len(), 1);
    assert!(released["workflows"][0]["receipt"].is_null());
    assert!(released["workflows"][0]["claim"]["finalized_at"].is_string());
    assert_eq!(
        read(&state, "/ledger/transactions")["transactions"],
        json!([])
    );
    std::fs::remove_file(path).ok();
}

#[test]
fn history_never_crosses_owner_boundaries_even_with_known_public_ids() {
    let (path, state, alice, auth) = setup_executor_authorization("history-two-owners");
    settle(&state, &alice, &auth, 1);
    let alice_owner = authenticated_user_context(&state).unwrap().user_id;
    init(
        json!({"username":"bob-history","display_name":"Bob","email":"bob-history@example.com"})
            .to_string(),
        &state,
    )
    .unwrap();
    let bob = register_agent(
        json!({"name":"bob-history-agent","version":"v1"}).to_string(),
        &state,
    )
    .unwrap();
    let bob_budget = create_test_agent_budget(&state, &bob.agent_id, 100);
    assert_eq!(
        read(&state, "/ledger/transactions")["transactions"],
        json!([])
    );
    assert_eq!(read(&state, "/spend/workflows")["workflows"], json!([]));
    for query in [
        format!("/ledger/transactions?agent_id={}", alice.agent_id),
        format!("/ledger/transactions?account_id={}", alice.account_id),
        format!(
            "/ledger/transactions?agent_id={}&budget_id={}",
            bob.agent_id,
            auth.budget_hold.as_ref().unwrap().budget_id
        ),
        format!("/spend/workflows/show?workflow_id={}", auth.decision_id),
        format!(
            "/spend/workflows/show?agent_id={}&operation_key={}",
            alice.agent_id, auth.operation_key
        ),
    ] {
        assert_ne!(
            route(authenticated_get_request(&query), &state).status,
            200,
            "{query}"
        );
    }
    assert_eq!(
        read(
            &state,
            &format!(
                "/ledger/transactions?agent_id={}&budget_id={}",
                bob.agent_id, bob_budget.budget.budget_id
            )
        )["coverage"]["consumed_amount_cents"],
        0
    );
    state.auth.select_owner_user(&alice_owner).unwrap();
    assert_eq!(
        read(&state, "/ledger/transactions")["transactions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    std::fs::remove_file(path).ok();
}

#[test]
fn workflow_uses_final_authorization_outcome_after_human_approval_or_denial() {
    let (path, state, agent, pending) = setup_pending_approval_with_lease_config_and_merchant(
        "history-approval",
        LeaseConfig::default(),
        "/spend/authorize",
        "gongbu.image",
    );
    let id = pending.body["decision_id"].as_str().unwrap();
    assert_eq!(
        read(&state, "/spend/workflows?status=needs_approval")["workflows"][0]["id"],
        id
    );
    let approved = route(
        approval_json_request(json!({"approval_request_id":id,"decision":"approve"})),
        &state,
    );
    assert_eq!(approved.status, 200, "{}", approved.body);
    let shown = read(&state, &format!("/spend/workflows/show?workflow_id={id}"));
    assert_eq!(shown["workflow"]["decision"], "allow");
    assert_eq!(shown["workflow"]["policy_decision"], "needs_approval");
    assert_eq!(shown["workflow"]["status"], "authorized");
    claim_executor_spend(json!({"operation_key":"history-approval-operation","spend_auth_token_id":approved.body["auth_token_id"],"account_id":agent.account_id,"amount_cents":600,"merchant":"gongbu.image"}).to_string(),&state).unwrap();
    finalize_executor_spend(json!({"operation_key":"history-approval-operation","agent_id":agent.agent_id,"receipt":precise_settlement_receipt_json(1,3)}).to_string(),&state,true,None).unwrap();
    assert_eq!(
        read(
            &state,
            &format!(
                "/spend/workflows/show?agent_id={}&operation_key=history-approval-operation",
                agent.agent_id
            )
        )["workflow"]["status"],
        "settled"
    );
    let denied = route(
        authenticated_json_request(
            "/spend/authorize",
            json!({"operation_key":"denied-human","account_id":agent.account_id,"amount_cents":600,"merchant":"gongbu.image","reason":"needs approval"}),
        ),
        &state,
    );
    assert_eq!(denied.body["decision"], "needs_approval");
    let rejected = route(
        approval_json_request(
            json!({"approval_request_id":denied.body["decision_id"],"decision":"deny"}),
        ),
        &state,
    );
    assert_eq!(rejected.status, 200);
    assert_eq!(
        read(&state, "/spend/workflows?status=needs_approval")["workflows"],
        json!([])
    );
    assert_ne!(
        route(
            authenticated_get_request(&format!(
                "/spend/workflows/show?workflow_id={}",
                denied.body["decision_id"].as_str().unwrap()
            )),
            &state
        )
        .status,
        200
    );
    std::fs::remove_file(path).ok();
}

#[test]
fn budget_denial_is_not_reported_as_authorized_and_expired_claim_requires_reconciliation() {
    let mut config = LeaseConfig::default();
    config
        .lease_profiles
        .get_mut("default")
        .unwrap()
        .claim_ttl_seconds = 1;
    let (path, state, agent, auth) =
        setup_executor_authorization_with_lease_config("history-expiry", config);
    let denied = route(
        authenticated_json_request(
            "/spend/authorize",
            json!({"operation_key":"budget-denied","account_id":agent.account_id,"amount_cents":1,"merchant":"gongbu.image","reason":"no remaining budget"}),
        ),
        &state,
    );
    assert_eq!(denied.body["decision"], "deny");
    assert_eq!(
        read(&state, "/spend/workflows")["workflows"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    claim_executor_spend(json!({"operation_key":auth.operation_key,"spend_auth_token_id":auth.auth_token_id,"account_id":agent.account_id,"amount_cents":500,"merchant":"gongbu.image"}).to_string(),&state).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let reconciliation = read(&state, "/spend/workflows?status=reconciliation_required");
    assert_eq!(reconciliation["workflows"].as_array().unwrap().len(), 1);
    assert!(reconciliation["workflows"][0]["receipt"].is_null());
    assert_eq!(
        read(&state, "/ledger/transactions")["transactions"],
        json!([])
    );
    std::fs::remove_file(path).ok();
}

#[test]
fn inconsistent_historical_linkage_is_unassigned_and_cannot_reveal_foreign_ids() {
    let (path, state, alice, auth) = setup_executor_authorization("history-bad-linkage");
    settle(&state, &alice, &auth, 1);
    let alice_owner = authenticated_user_context(&state).unwrap().user_id;
    init(json!({"username":"other-linkage-owner","display_name":"Other owner","email":"other-linkage@example.com"}).to_string(),&state).unwrap();
    let bob = register_agent(
        json!({"name":"other-linkage-agent","version":"v1"}).to_string(),
        &state,
    )
    .unwrap();
    let bob_budget = create_test_agent_budget(&state, &bob.agent_id, 100);
    let bob_context = authenticated_user_context(&state).unwrap();
    let bob_agent = resolve_agent_id_for_user(&bob.agent_id, &bob_context, &state).unwrap();
    let bob_account = state
        .registration
        .lock()
        .unwrap()
        .account_for_agent(&bob_agent)
        .unwrap()
        .unwrap()
        .id;
    let bob_budget_id =
        resolve_budget_id_for_user(&bob_budget.budget.budget_id, &bob_context, &state).unwrap();
    state.auth.select_owner_user(&alice_owner).unwrap();
    // Fault injection only: simulate an inconsistent pre-existing evidence row.
    let conn = rusqlite::Connection::open(&path).unwrap();
    let raw: String = conn
        .query_row(
            "SELECT record_json FROM ledger_transaction_metadata",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let mut metadata: Value = serde_json::from_str(&raw).unwrap();
    metadata["agent_id"] = json!(bob_agent);
    metadata["agent_account_id"] = json!(bob_account);
    metadata["context"]["agent_id"] = json!(bob_agent);
    metadata["context"]["agent_account_id"] = json!(bob_account);
    metadata["context"]["budget_id"] = json!(bob_budget_id);
    conn.execute_batch("DROP TRIGGER ledger_metadata_no_update")
        .unwrap();
    conn.execute(
        "UPDATE ledger_transaction_metadata SET record_json=?1",
        [metadata.to_string()],
    )
    .unwrap();
    let all = read(&state, "/ledger/transactions");
    assert!(all["transactions"][0]["agent_id"].is_null());
    assert!(all["transactions"][0]["budget_id"].is_null());
    assert_eq!(
        all["transactions"][0]["coverage"]["missing_evidence"],
        json!(["invalid_governed_context"])
    );
    assert!(!all.to_string().contains(&bob.agent_id));
    assert!(!all.to_string().contains(&bob.account_id));
    assert!(!all.to_string().contains(&bob_budget.budget.budget_id));
    let scoped = read(
        &state,
        &format!(
            "/ledger/transactions?agent_id={}&budget_id={}",
            alice.agent_id,
            auth.budget_hold.as_ref().unwrap().budget_id
        ),
    );
    assert_eq!(scoped["transactions"], json!([]));
    assert_eq!(scoped["coverage"]["recorded_budget_charges_cents"], 0);
    assert_eq!(scoped["coverage"]["unaccounted_consumption_cents"], 1);
    std::fs::remove_file(path).ok();
}

#[test]
fn two_providers_for_one_task_have_independent_receipts_and_postings() {
    let path = std::env::temp_dir().join(format!(
        "hubu-history-two-providers-{}.sqlite",
        UserId::new()
    ));
    let state = ServerState::new_with_db_path(&path).unwrap();
    init(
        json!({"display_name":"Two provider owner","email":"providers@example.com"}).to_string(),
        &state,
    )
    .unwrap();
    let agent = register_agent(
        json!({"name":"two-provider-agent","version":"v1"}).to_string(),
        &state,
    )
    .unwrap();
    add_policy(
        json!({"agent_id":agent.agent_id,"daily_limit_cents":500}).to_string(),
        &state,
    )
    .unwrap();
    let budget = create_test_agent_budget(&state, &agent.agent_id, 1000);
    for (index, scope) in trusted_execution_scope_catalog().iter().take(2).enumerate() {
        let key = format!("independent-private-operation-{index}");
        let selector = scope_as_selector(scope);
        let auth=authorize_spend(json!({"operation_key":key,"account_id":agent.account_id,"amount_cents":100,"execution_scope":selector,"reason":"compare image providers","task_id":"private-shared-task"}).to_string(),&state).unwrap();
        claim_executor_spend(json!({"operation_key":key,"spend_auth_token_id":auth.auth_token_id,"account_id":agent.account_id,"amount_cents":100,"execution_scope":scope}).to_string(),&state).unwrap();
        finalize_executor_spend(json!({"operation_key":key,"agent_id":agent.agent_id,"receipt":precise_settlement_receipt_json((index+1) as i64,3)}).to_string(),&state,true,None).unwrap();
    }
    let output = read(
        &state,
        &format!(
            "/spend/workflows?agent_id={}&status=settled",
            agent.agent_id
        ),
    );
    let rows = output["workflows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_ne!(rows[0]["id"], rows[1]["id"]);
    assert_ne!(rows[0]["provider"], rows[1]["provider"]);
    assert_ne!(
        rows[0]["receipt"]["settlement_id"],
        rows[1]["receipt"]["settlement_id"]
    );
    assert_ne!(
        rows[0]["ledger_transaction_ids"],
        rows[1]["ledger_transaction_ids"]
    );
    assert_eq!(rows[0]["task_reference"], rows[1]["task_reference"]);
    assert_eq!(rows[0]["task_reference"]["redacted"], true);
    assert!(!output.to_string().contains("private-shared-task"));
    let history = read(
        &state,
        &format!(
            "/ledger/transactions?agent_id={}&budget_id={}&limit=1",
            agent.agent_id, budget.budget.budget_id
        ),
    );
    assert_eq!(history["coverage"]["consumed_amount_cents"], 2);
    assert_eq!(history["coverage"]["recorded_budget_charges_cents"], 2);
    assert!(history["next_cursor"].is_string());
    std::fs::remove_file(path).ok();
}
