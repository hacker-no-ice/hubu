use super::*;

fn void_body(agent: &RegisterAgentHttpResponse, auth: &SpendHttpResponse) -> Value {
    json!({"spend_auth_token_id":auth.auth_token_id,"account_id":agent.account_id,"operation_key":auth.operation_key})
}

fn claim_body(agent: &RegisterAgentHttpResponse, auth: &SpendHttpResponse) -> String {
    json!({"spend_auth_token_id":auth.auth_token_id,"account_id":agent.account_id,"amount_cents":500,"merchant":"gongbu.image"}).to_string()
}

fn read(state: &ServerState, path: &str) -> Value {
    let response = route(authenticated_get_request(path), state);
    assert_eq!(response.status, 200, "{}", response.body);
    response.body
}

#[test]
fn admission_void_restores_budget_replays_and_is_visible_without_a_posting() {
    let (path, state, agent, auth) = setup_executor_authorization("admission-void");
    let body = void_body(&agent, &auth);
    let first = route(
        authenticated_json_request("/spend/void", body.clone()),
        &state,
    );
    assert_eq!(first.status, 200, "{}", first.body);
    assert_eq!(first.body["idempotent_replay"], false);
    let replay = void_unclaimed_authorization(body.to_string(), &state).unwrap();
    assert_eq!(replay["idempotent_replay"], true);
    assert!(claim_executor_spend(claim_body(&agent, &auth), &state)
        .unwrap_err()
        .to_string()
        .contains("revoked"));
    let history = read(
        &state,
        &format!("/spend/authorizations?agent_id={}", agent.agent_id),
    );
    let record = &history["authorization_records"][0];
    assert_eq!(record["status"], "released");
    assert_eq!(record["release_reason"], "executor_rejected_before_claim");
    assert_eq!(record["ledger_transaction_ids"], json!([]));
    assert!(record["claim"].is_null());
    let watch = read(&state, "/watch");
    assert_eq!(watch["rows"][0]["available_cents"], 500);
    assert_eq!(watch["rows"][0]["frozen_cents"], 0);
    assert_eq!(watch["rows"][0]["consumed_cents"], 0);
    assert_eq!(
        watch["recent_events"][0]["release_reason"],
        "executor_rejected_before_claim"
    );
    drop(state);
    let restarted = ServerState::new_with_db_path(&path).unwrap();
    // Persisted evidence is visible even after reconstructing the managers.
    let snapshot = restarted.governance.lock().unwrap().history_snapshot(
        &restarted
            .registration
            .lock()
            .unwrap()
            .account_for_pub_id(&agent.account_id)
            .unwrap()
            .unwrap()
            .owner_user_id,
    );
    assert_eq!(snapshot.unwrap().void_reasons.len(), 1);
    std::fs::remove_file(path).ok();
}

#[test]
fn claimed_authorization_cannot_be_voided_or_credited() {
    let (path, state, agent, auth) = setup_executor_authorization("claim-before-void");
    let claim = claim_executor_spend(claim_body(&agent, &auth), &state).unwrap();
    assert!(
        void_unclaimed_authorization(void_body(&agent, &auth).to_string(), &state)
            .unwrap_err()
            .to_string()
            .contains("claimed")
    );
    let snapshot = get_executor_claim(Some(&claim.claim_id), &state).unwrap();
    assert_eq!(snapshot.status, "claimed");
    assert_eq!(snapshot.spend.budget_hold.frozen_amount_cents, 500);
    assert_eq!(snapshot.spend.budget_hold.remaining_amount_cents, 0);
    std::fs::remove_file(path).ok();
}

#[test]
fn durable_void_wins_against_a_claim_using_stale_in_memory_state() {
    let (path, state, agent, auth) = setup_executor_authorization("stale-claim-after-void");
    let account = state
        .registration
        .lock()
        .unwrap()
        .account_for_pub_id(&agent.account_id)
        .unwrap()
        .unwrap();
    let mut competing = SqliteGovernanceRepository::open(&path).unwrap();
    competing
        .void_unclaimed_authorization(
            &account.owner_user_id,
            &account.id,
            &auth.auth_token_id.as_ref().unwrap().parse().unwrap(),
            &auth.operation_key,
        )
        .unwrap();
    // The API's managers still think this token and hold are available.
    assert!(claim_executor_spend(claim_body(&agent, &auth), &state)
        .unwrap_err()
        .to_string()
        .contains("revoked"));
    assert!(competing.load_executor_claims().unwrap().is_empty());
    assert_eq!(
        competing.load_budget_balances().unwrap()[0].remaining_amount,
        Amount::from_legacy_cents(500).unwrap()
    );
    assert!(state
        .spend
        .lock()
        .unwrap()
        .executor_claim_for_operation(&account.agent_id, &auth.operation_key)
        .is_none());
    std::fs::remove_file(path).ok();
}

#[test]
fn void_requires_the_bound_account_private_operation_and_authenticated_owner() {
    let (path, state, agent, auth) = setup_executor_authorization("void-scope");
    let other = register_agent(
        json!({"name":"other-void-agent","version":"v1"}).to_string(),
        &state,
    )
    .unwrap();
    for override_fields in [
        json!({"account_id":other.account_id}),
        json!({"operation_key":"different-operation"}),
        json!({"operation_key":""}),
    ] {
        let mut body = void_body(&agent, &auth);
        for (key, value) in override_fields.as_object().unwrap() {
            body[key] = value.clone();
        }
        assert!(void_unclaimed_authorization(body.to_string(), &state).is_err());
    }
    let response = route(
        HttpRequest {
            method: "POST".into(),
            path: "/spend/void".into(),
            query: Default::default(),
            headers: Default::default(),
            body: void_body(&agent, &auth).to_string(),
        },
        &state,
    );
    assert_eq!(response.status, 401);
    assert!(claim_executor_spend(claim_body(&agent, &auth), &state).is_ok());
    std::fs::remove_file(path).ok();
}
