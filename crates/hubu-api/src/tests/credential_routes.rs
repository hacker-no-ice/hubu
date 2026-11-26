//! Per-agent credentials through the route layer (HUB-357). Owner actions use
//! the test fixture provider, which confirms without a real dialog.
use super::*;
use hubu_core::agent_credential::{format_credential, parse_credential, CredentialKind};

struct Minted {
    token: String,
    credential_id: String,
    verification_hash: String,
    secret: String,
}

fn mint(kind: CredentialKind, seed: u8) -> Minted {
    let mut entropy = [0_u8; 48];
    for (index, byte) in entropy.iter_mut().enumerate() {
        *byte = seed
            .wrapping_mul(53)
            .wrapping_add((index as u8).wrapping_mul(7));
    }
    let token = format_credential(kind, &entropy);
    let parsed = parse_credential(&token).expect("minted credential parses");
    let secret = token.rsplit('_').next().unwrap().to_string();
    Minted {
        credential_id: parsed.credential_id.clone(),
        verification_hash: parsed.verification_hash(),
        token,
        secret,
    }
}

fn temp_state(label: &str) -> (std::path::PathBuf, ServerState) {
    let path = std::env::temp_dir().join(format!("hubu-api-{label}-{}.sqlite", UserId::new()));
    let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
    init(
        json!({ "display_name": "Alice Example", "email": "alice@example.com" }).to_string(),
        &state,
    )
    .expect("init should create an explicit user");
    (path, state)
}

fn post(state: &ServerState, path: &str, token: &str, body: Value) -> HttpResponse {
    route(
        credential_request_with_body("POST", path, token, body),
        state,
    )
}

fn get(state: &ServerState, path: &str, token: &str) -> HttpResponse {
    route(credential_request("GET", path, token), state)
}

fn record_registration_credential(state: &ServerState, seed: u8) -> Minted {
    let registration = mint(CredentialKind::Registration, seed);
    let response = post(
        state,
        "/agents/registration-credentials",
        TEST_AUTH_TOKEN,
        json!({
            "harness": "claude",
            "credential_id": registration.credential_id,
            "verification_hash": registration.verification_hash,
        }),
    );
    assert_eq!(response.status, 200, "{:?}", response.body);
    registration
}

fn register_with(
    state: &ServerState,
    bearer: &str,
    name: &str,
    credential: &Minted,
) -> HttpResponse {
    post(
        state,
        "/agents/register",
        bearer,
        json!({
            "name": name,
            "version": "v1",
            "agent_credential": {
                "credential_id": credential.credential_id,
                "verification_hash": credential.verification_hash,
            },
        }),
    )
}

fn confirm(state: &ServerState, agent_id: &str) -> HttpResponse {
    post(
        state,
        "/agents/confirm",
        TEST_AUTH_TOKEN,
        json!({ "agent_ids": [agent_id] }),
    )
}

fn authorize(state: &ServerState, token: &str, account_id: &str) -> HttpResponse {
    post(
        state,
        "/spend/authorize",
        token,
        json!({
            "operation_key": format!("op-{}", UserId::new()),
            "account_id": account_id,
            "amount": "0.10",
            "reason": "test spend",
            "merchant": "fixture-merchant",
        }),
    )
}

fn error_code(response: &HttpResponse) -> &str {
    response.body["error_code"].as_str().unwrap_or_default()
}

/// A pending agent: registered with a registration credential, not confirmed.
fn pending_agent(state: &ServerState, name: &str, seed: u8) -> (Minted, Value) {
    let registration = record_registration_credential(state, seed);
    let agent = mint(CredentialKind::Agent, seed.wrapping_add(1));
    let response = register_with(state, &registration.token, name, &agent);
    assert_eq!(response.status, 200, "{:?}", response.body);
    assert_eq!(response.body["credential_status"], "pending");
    (agent, response.body)
}

#[test]
fn pending_agent_is_refused_until_the_owner_confirms() {
    let (path, state) = temp_state("cred-pending");
    let (agent, registered) = pending_agent(&state, "design", 1);
    let account_id = registered["account_id"].as_str().unwrap();
    let agent_id = registered["agent_id"].as_str().unwrap();

    for response in [
        authorize(&state, &agent.token, account_id),
        get(&state, "/budgets", &agent.token),
    ] {
        assert_eq!(response.status, 403, "{:?}", response.body);
        assert_eq!(error_code(&response), "agent_pending_owner_confirmation");
        assert_eq!(response.body["retry_guidance"]["action"], "wait_for_owner");
        assert!(response.body["error"].as_str().unwrap().contains("owner"));
    }
    // The shared v0.3.0 credential cannot spend for it either.
    let shared = authorize(&state, TEST_AGENT_TOKEN, account_id);
    assert_eq!(error_code(&shared), "agent_pending_owner_confirmation");

    let agents = get(&state, "/agents", TEST_AUTH_TOKEN);
    assert_eq!(agents.body["agents"][0]["credential_status"], "pending");

    let confirmed = confirm(&state, agent_id);
    assert_eq!(confirmed.status, 200, "{:?}", confirmed.body);
    assert_eq!(confirmed.body["agents"][0]["credential_status"], "active");
    let budgets = get(&state, "/budgets", &agent.token);
    assert_eq!(budgets.status, 200, "{:?}", budgets.body);

    // Confirming again is refused: nothing is pending.
    assert_eq!(error_code(&confirm(&state, agent_id)), "agent_not_pending");
    std::fs::remove_file(path).ok();
}

#[test]
fn an_agent_spends_only_from_its_own_account() {
    let (path, state) = temp_state("cred-account");
    let (agent, registered) = pending_agent(&state, "design", 2);
    let own_account = registered["account_id"].as_str().unwrap().to_string();
    confirm(&state, registered["agent_id"].as_str().unwrap());
    let other = register_agent(
        json!({ "name": "research", "version": "v1" }).to_string(),
        &state,
    )
    .expect("legacy registration still works for the owner");

    let mismatch = authorize(&state, &agent.token, &other.account_id);
    assert_eq!(mismatch.status, 403, "{:?}", mismatch.body);
    assert_eq!(error_code(&mismatch), "agent_account_mismatch");
    let void = post(
        &state,
        "/spend/void",
        &agent.token,
        json!({
            "spend_auth_token_id": "spa_0123456789ab",
            "account_id": other.account_id,
            "operation_key": "op",
        }),
    );
    assert_eq!(error_code(&void), "agent_account_mismatch");

    // Its own account passes the binding; whatever the handler answers, it is
    // not a credential refusal.
    let own = authorize(&state, &agent.token, &own_account);
    assert!(
        !matches!(
            error_code(&own),
            "agent_account_mismatch" | "agent_pending_owner_confirmation"
        ) && own.status != 401,
        "{:?}",
        own.body
    );

    // No other credential works for its account.
    let shared = authorize(&state, TEST_AGENT_TOKEN, &own_account);
    assert_eq!(error_code(&shared), "agent_account_mismatch");
    // An agent without its own credential keeps the shared credential.
    let legacy = authorize(&state, TEST_AGENT_TOKEN, &other.account_id);
    assert_ne!(error_code(&legacy), "agent_account_mismatch");
    std::fs::remove_file(path).ok();
}

#[test]
fn a_revoked_credential_is_refused_on_the_next_request() {
    let (path, state) = temp_state("cred-revoke");
    let (agent, registered) = pending_agent(&state, "design", 3);
    let agent_id = registered["agent_id"].as_str().unwrap();
    confirm(&state, agent_id);
    assert_eq!(get(&state, "/budgets", &agent.token).status, 200);

    let revoked = post(
        &state,
        "/agents/credential/revoke",
        TEST_AUTH_TOKEN,
        json!({ "agent_id": agent_id }),
    );
    assert_eq!(revoked.status, 200, "{:?}", revoked.body);
    assert_eq!(revoked.body["credential_status"], "revoked");

    let refused = get(&state, "/budgets", &agent.token);
    assert_eq!(refused.status, 401);
    assert_eq!(error_code(&refused), "agent_credential_revoked");
    // The account still refuses the shared credential.
    let shared = authorize(
        &state,
        TEST_AGENT_TOKEN,
        registered["account_id"].as_str().unwrap(),
    );
    assert_eq!(error_code(&shared), "agent_account_mismatch");
    std::fs::remove_file(path).ok();
}

#[test]
fn rotate_replaces_the_credential() {
    let (path, state) = temp_state("cred-rotate");
    let (old, registered) = pending_agent(&state, "design", 4);
    let agent_id = registered["agent_id"].as_str().unwrap();
    let new = mint(CredentialKind::Agent, 40);
    let early = post(
        &state,
        "/agents/credential/rotate",
        TEST_AUTH_TOKEN,
        json!({
            "agent_id": agent_id,
            "credential_id": new.credential_id,
            "verification_hash": new.verification_hash,
        }),
    );
    assert_eq!(error_code(&early), "agent_credential_not_active");
    confirm(&state, agent_id);
    let rotated = post(
        &state,
        "/agents/credential/rotate",
        TEST_AUTH_TOKEN,
        json!({
            "agent_id": agent_id,
            "credential_id": new.credential_id,
            "verification_hash": new.verification_hash,
        }),
    );
    assert_eq!(rotated.status, 200, "{:?}", rotated.body);
    assert_eq!(
        error_code(&get(&state, "/budgets", &old.token)),
        "agent_credential_revoked"
    );
    assert_eq!(get(&state, "/budgets", &new.token).status, 200);
    std::fs::remove_file(path).ok();
}

#[test]
fn re_registration_keeps_the_account_and_refuses_a_second_credential() {
    let (path, state) = temp_state("cred-rereg");
    let registration = record_registration_credential(&state, 5);
    let agent = mint(CredentialKind::Agent, 6);
    let first = register_with(&state, &registration.token, "design", &agent);
    assert_eq!(first.status, 200, "{:?}", first.body);

    // The same credential again creates nothing new.
    let again = register_with(&state, &registration.token, "design", &agent);
    assert_eq!(again.status, 200, "{:?}", again.body);
    assert_eq!(again.body["identity_resolution"], "reused");
    assert_eq!(again.body["account_id"], first.body["account_id"]);

    let intruder = mint(CredentialKind::Agent, 7);
    let refused = register_with(&state, &registration.token, "design", &intruder);
    assert_eq!(refused.status, 409, "{:?}", refused.body);
    assert_eq!(error_code(&refused), "agent_already_registered");

    // After confirmation the agent may re-register itself with its own
    // credential, and only itself.
    confirm(&state, first.body["agent_id"].as_str().unwrap());
    let own = register_with(&state, &agent.token, "design", &agent);
    assert_eq!(own.status, 200, "{:?}", own.body);
    assert_eq!(own.body["credential_status"], "active");
    let other = register_with(&state, &agent.token, "someone-else", &agent);
    assert_eq!(error_code(&other), "agent_account_mismatch");
    let agents = get(&state, "/agents", TEST_AUTH_TOKEN);
    assert_eq!(
        agents.body["agents"].as_array().unwrap().len(),
        1,
        "a refused registration creates no agent: {:?}",
        agents.body
    );
    std::fs::remove_file(path).ok();
}

#[test]
fn registration_credential_reaches_only_registration() {
    let (path, state) = temp_state("cred-scope");
    let registration = record_registration_credential(&state, 8);
    for (method, sample, _) in ROUTE_SAMPLES {
        if *method == "POST" && *sample == "/agents/register" {
            continue;
        }
        let response = route(
            credential_request(method, sample, &registration.token),
            &state,
        );
        assert_eq!(
            (response.status, error_code(&response)),
            (403, "registration_credential_scope"),
            "{method} {sample}: {:?}",
            response.body
        );
    }
    let missing = post(
        &state,
        "/agents/register",
        &registration.token,
        json!({ "name": "design", "version": "v1" }),
    );
    assert_eq!(error_code(&missing), "agent_credential_required");
    assert_eq!(
        get(&state, "/agents", TEST_AUTH_TOKEN).body["agents"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    // Running `hubu init` again replaces the registration credential.
    let replacement = record_registration_credential(&state, 9);
    let agent = mint(CredentialKind::Agent, 10);
    let stale = register_with(&state, &registration.token, "design", &agent);
    assert_eq!(stale.status, 401);
    assert_eq!(error_code(&stale), "agent_credential_invalid");
    let fresh = register_with(&state, &replacement.token, "design", &agent);
    assert_eq!(fresh.status, 200, "{:?}", fresh.body);
    std::fs::remove_file(path).ok();
}

#[test]
fn unknown_and_tampered_credentials_are_invalid() {
    let (path, state) = temp_state("cred-invalid");
    let (agent, _) = pending_agent(&state, "design", 11);
    let unknown = mint(CredentialKind::Agent, 99);
    let mut tampered = agent.token.clone();
    let last = tampered.pop().unwrap();
    tampered.push(if last == 'a' { 'b' } else { 'a' });
    for token in [unknown.token.as_str(), tampered.as_str()] {
        let response = get(&state, "/budgets", token);
        assert_eq!(response.status, 401);
        assert_eq!(error_code(&response), "agent_credential_invalid");
    }
    std::fs::remove_file(path).ok();
}

#[test]
fn confirm_names_the_agent_and_needs_the_owner() {
    let (path, state) = temp_state("cred-confirm");
    let (agent, registered) = pending_agent(&state, "design", 12);
    let agent_id = registered["agent_id"].as_str().unwrap();
    // An agent credential cannot confirm anything.
    let by_shared = post(
        &state,
        "/agents/confirm",
        TEST_AGENT_TOKEN,
        json!({ "agent_ids": [agent_id] }),
    );
    assert_eq!(error_code(&by_shared), OWNER_AUTHORITY_REQUIRED_ERROR_CODE);
    let by_pending = post(
        &state,
        "/agents/confirm",
        &agent.token,
        json!({ "agent_ids": [agent_id] }),
    );
    assert_eq!(error_code(&by_pending), "agent_pending_owner_confirmation");

    let mut request = credential_request_with_body(
        "POST",
        "/agents/confirm",
        TEST_AUTH_TOKEN,
        json!({ "agent_ids": [agent_id] }),
    );
    request.headers.insert(
        owner_presence::NO_PROMPT_HEADER.to_string(),
        "1".to_string(),
    );
    let response = route(request, &state);
    assert_eq!(error_code(&response), OWNER_PRESENCE_REQUIRED_ERROR_CODE);
    assert_eq!(response.body["owner_action"], "agent.confirm");

    assert_eq!(confirm(&state, agent_id).status, 200);
    let records = get(
        &state,
        "/governance/owner-presence?limit=200",
        TEST_AUTH_TOKEN,
    );
    let record = records.body["records"]
        .as_array()
        .expect("presence records")
        .iter()
        .find(|record| record["action"] == "agent.confirm")
        .unwrap_or_else(|| panic!("no agent.confirm record: {:?}", records.body))
        .clone();
    assert_eq!(
        record["reason"],
        "confirm new agent \"design\" on claude (reported by the agent)"
    );

    let unknown = confirm(&state, "agt_0000000000000000");
    assert_eq!(unknown.status, 404, "{:?}", unknown.body);
    assert_eq!(error_code(&unknown), "agent_not_found");
    std::fs::remove_file(path).ok();
}

#[test]
fn no_credential_value_reaches_storage_or_responses() {
    let (path, state) = temp_state("cred-hygiene");
    let registration = record_registration_credential(&state, 13);
    let agent = mint(CredentialKind::Agent, 14);
    let mut responses = vec![register_with(&state, &registration.token, "design", &agent)];
    let agent_id = responses[0].body["agent_id"].as_str().unwrap().to_string();
    responses.push(confirm(&state, &agent_id));
    responses.push(get(&state, "/agents", TEST_AUTH_TOKEN));
    responses.push(get(&state, "/budgets", &agent.token));
    for response in &responses {
        let body = response.body.to_string();
        for secret in [&agent.secret, &registration.secret] {
            assert!(!body.contains(secret.as_str()), "{body}");
        }
    }
    drop(state);
    let database = std::fs::read(&path).expect("database file");
    let database = String::from_utf8_lossy(&database);
    for secret in [&agent.secret, &registration.secret] {
        assert!(!database.contains(secret.as_str()));
    }
    for token in [&agent.token, &registration.token] {
        assert!(!database.contains(token.as_str()));
    }
    std::fs::remove_file(path).ok();
}
