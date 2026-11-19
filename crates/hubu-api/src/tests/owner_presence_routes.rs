//! Route-level owner presence tests (v0.3.0 spec, section 13). Every test uses
//! the scripted fixture provider; none depends on a real fingerprint.
use super::*;
use owner_presence::{FixtureProvider, OwnerClient, PresenceMethod};

fn fixture(state: &ServerState, script: &str) {
    state
        .presence
        .set_provider(Arc::new(FixtureProvider::parse(script).unwrap()));
}

fn temp_state(label: &str) -> (std::path::PathBuf, ServerState) {
    let path = std::env::temp_dir().join(format!("hubu-api-{label}-{}.sqlite", UserId::new()));
    let state = ServerState::new_with_db_path(&path).expect("server state should initialize");
    (path, state)
}

fn state_with_agent(label: &str) -> (std::path::PathBuf, ServerState, RegisterAgentHttpResponse) {
    let (path, state) = temp_state(label);
    init(
        json!({ "display_name": "Alice Example", "email": "alice@example.com" }).to_string(),
        &state,
    )
    .expect("init should create an explicit user");
    let agent = register_agent(
        json!({ "name": "design-agent", "version": "v1" }).to_string(),
        &state,
    )
    .expect("agent should register");
    (path, state, agent)
}

fn with_header(mut request: HttpRequest, name: &str, value: &str) -> HttpRequest {
    request.headers.insert(name.to_string(), value.to_string());
    request
}

fn records(state: &ServerState) -> Vec<Value> {
    let response = route(
        authenticated_get_request("/governance/owner-presence?limit=200"),
        state,
    );
    assert_eq!(response.status, 200, "{}", response.body);
    response.body["records"].as_array().unwrap().clone()
}

fn create_budget_request(agent_id: &str, amount: &str) -> HttpRequest {
    authenticated_json_request(
        "/budgets",
        json!({ "agent_id": agent_id, "amount": amount }),
    )
}

/// Every owner action is refused without an owner window, whatever else the
/// caller holds: no-prompt gets `owner_presence_required`, a refused dialog
/// gets the refusal, and malformed requests are rejected before any prompt.
#[test]
fn owner_actions_are_refused_without_a_window_or_confirmation() {
    let (path, state, agent) = state_with_agent("presence-sweep");
    for (method, sample, pattern) in ROUTE_SAMPLES {
        let declared = ROUTE_ACCESS
            .iter()
            .find(|(m, p, _)| m == method && p == pattern)
            .map(|(_, _, access)| *access)
            .unwrap();
        if declared != RouteAccess::OwnerAction {
            continue;
        }
        for script in ["deny", "timeout", "unavailable"] {
            fixture(&state, script);
            let mut request = credential_request(method, sample, TEST_AUTH_TOKEN);
            request.headers.insert(
                APPROVAL_CAPABILITY_HEADER.to_string(),
                TEST_APPROVAL_TOKEN.to_string(),
            );
            request.headers.insert(
                RECONCILIATION_CAPABILITY_HEADER.to_string(),
                TEST_RECONCILIATION_TOKEN.to_string(),
            );
            let response = route(request, &state);
            assert!(
                response.status >= 400,
                "{method} {sample} must be refused with {script}: {}",
                response.body
            );
            let no_prompt = route(
                with_header(
                    credential_request(method, sample, TEST_AUTH_TOKEN),
                    owner_presence::NO_PROMPT_HEADER,
                    "1",
                ),
                &state,
            );
            assert!(no_prompt.status >= 400, "{method} {sample}");
        }
    }
    // A well-formed budget creation shows the refusal codes exactly.
    for (script, code, status) in [
        ("deny", "owner_presence_denied", 403),
        ("timeout", "owner_presence_timeout", 403),
        ("unavailable", "owner_presence_unavailable", 403),
    ] {
        fixture(&state, script);
        let response = route(create_budget_request(&agent.agent_id, "10.00"), &state);
        assert_eq!(response.status, status);
        assert_eq!(response.body["error_code"], code);
        assert!(response.body["retry_guidance"]["action"].is_string());
    }
    let no_prompt = route(
        with_header(
            create_budget_request(&agent.agent_id, "10.00"),
            owner_presence::NO_PROMPT_HEADER,
            "1",
        ),
        &state,
    );
    assert_eq!(no_prompt.status, 403);
    assert_eq!(
        no_prompt.body["error_code"],
        OWNER_PRESENCE_REQUIRED_ERROR_CODE
    );
    let budgets = route(authenticated_get_request("/budgets?all=true"), &state);
    assert_eq!(budgets.body["budgets"].as_array().unwrap().len(), 0);
    std::fs::remove_file(path).ok();
}

#[test]
fn one_confirmation_opens_a_five_minute_window_for_that_client() {
    let (path, state, agent) = state_with_agent("presence-window");
    // The second answer would deny, so a second prompt would fail.
    fixture(&state, "confirm,deny");
    let first = route(
        with_header(
            create_budget_request(&agent.agent_id, "10.00"),
            owner_presence::CLIENT_HEADER,
            "portal",
        ),
        &state,
    );
    assert_eq!(first.status, 200, "{}", first.body);
    let token = first.body["owner_window"]["token"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(token.starts_with("hubu_window_"));
    let budget_id = first.body["budget"]["budget_id"]
        .as_str()
        .unwrap()
        .to_string();

    let raise = |token: &str| {
        with_header(
            authenticated_json_request(
                &format!("/budgets/{budget_id}/versions"),
                json!({ "amount_limit": "20.00", "expected_revision": 1, "reason": "more" }),
            ),
            owner_presence::OWNER_WINDOW_HEADER,
            token,
        )
    };
    let second = route(raise(&token), &state);
    assert_eq!(second.status, 200, "{}", second.body);
    assert!(second.body.get("owner_window").is_none());

    // A forged token is not a window: the request prompts, and is denied.
    let forged = route(raise("hubu_window_forged"), &state);
    assert_eq!(forged.body["error_code"], "owner_presence_denied");

    // The window does not slide and closes after five minutes.
    state
        .presence
        .advance_clock(owner_presence::OWNER_WINDOW_DURATION);
    let expired = route(
        with_header(
            authenticated_json_request(
                &format!("/budgets/{budget_id}/versions"),
                json!({ "amount_limit": "30.00", "expected_revision": 2, "reason": "more" }),
            ),
            owner_presence::OWNER_WINDOW_HEADER,
            &token,
        ),
        &state,
    );
    assert_eq!(expired.body["error_code"], "owner_presence_denied");

    let records = records(&state);
    let outcomes = records
        .iter()
        .map(|record| record["outcome"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(outcomes, vec!["denied", "denied", "window", "confirmed"]);
    assert_eq!(records[3]["client"], "portal");
    assert_eq!(records[3]["method"], "fixture");
    assert_eq!(
        records[3]["reason"],
        "create a 10.00 USD budget for design-agent"
    );
    assert_eq!(records[2]["action"], "budget.version.create");
    assert_eq!(records[2]["window_record_id"], records[3]["record_id"]);
    assert_eq!(
        records[1]["reason"],
        "set design-agent's budget to 20.00 USD (now 20.00 USD)"
    );
    std::fs::remove_file(path).ok();
}

#[test]
fn lock_closes_windows_and_the_agent_credential_may_lock() {
    let (path, state, agent) = state_with_agent("presence-lock");
    fixture(&state, "confirm,deny");
    let unlocked = route(
        authenticated_json_request("/owner/unlock", json!({})),
        &state,
    );
    assert_eq!(unlocked.status, 200, "{}", unlocked.body);
    let token = unlocked.body["owner_window"]["token"]
        .as_str()
        .unwrap()
        .to_string();
    let status = route(
        with_header(
            credential_request("GET", "/owner/presence", TEST_AGENT_TOKEN),
            owner_presence::OWNER_WINDOW_HEADER,
            &token,
        ),
        &state,
    );
    assert_eq!(status.body["window"]["open"], true);

    let locked = route(
        credential_request("POST", "/owner/lock", TEST_AGENT_TOKEN),
        &state,
    );
    assert_eq!(locked.status, 200);
    assert_eq!(locked.body["closed_windows"], 1);
    let after = route(
        with_header(
            create_budget_request(&agent.agent_id, "10.00"),
            owner_presence::OWNER_WINDOW_HEADER,
            &token,
        ),
        &state,
    );
    assert_eq!(after.body["error_code"], "owner_presence_denied");
    assert_eq!(
        records(&state)[1]["reason"],
        "unlock the Hubu admin portal for 5 minutes"
    );
    std::fs::remove_file(path).ok();
}

#[test]
fn tightening_needs_no_prompt_and_is_recorded() {
    let (path, state, agent) = state_with_agent("presence-tighten");
    let budget = create_test_agent_budget(&state, &agent.agent_id, 1_000);
    fixture(&state, "deny");
    let revoked = route(
        with_header(
            authenticated_json_request(
                "/budgets/revoke",
                json!({ "budget_id": budget.budget.budget_id }),
            ),
            owner_presence::CLIENT_HEADER,
            "cli",
        ),
        &state,
    );
    assert_eq!(revoked.status, 200, "{}", revoked.body);
    let records = records(&state);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["outcome"], "not_required");
    assert_eq!(records[0]["action"], "budget.revoke");
    assert_eq!(records[0]["client"], "cli");
    assert!(records[0]["method"].is_null());
    std::fs::remove_file(path).ok();
}

#[test]
fn owner_deny_of_a_spend_is_tightening() {
    let (path, state, _agent, pending) = setup_pending_approval("presence-deny-spend");
    let id = pending.body["approval"]["approval_request_id"]
        .as_str()
        .unwrap();
    fixture(&state, "deny");
    let denied = route(
        authenticated_json_request(
            "/spend/approval/resolve",
            json!({ "approval_request_id": id, "decision": "deny" }),
        ),
        &state,
    );
    assert_eq!(denied.status, 200, "{}", denied.body);
    assert_eq!(records(&state)[0]["outcome"], "not_required");
    std::fs::remove_file(path).ok();
}

fn prompt_request(id: &str) -> HttpRequest {
    credential_request_with_body(
        "POST",
        "/spend/approval/prompt",
        TEST_AGENT_TOKEN,
        json!({ "approval_request_id": id }),
    )
}

fn approval_status(state: &ServerState, id: &str) -> String {
    let response = route(
        credential_request(
            "GET",
            &format!("/spend/approval?approval_request_id={id}"),
            TEST_AGENT_TOKEN,
        ),
        state,
    );
    response.body["status"].as_str().unwrap().to_string()
}

#[test]
fn agent_raised_approval_confirm_approves() {
    let (path, state, _agent, pending) = setup_pending_approval("presence-agent-confirm");
    let id = pending.body["approval"]["approval_request_id"]
        .as_str()
        .unwrap();
    fixture(&state, "confirm");
    let approved = route(prompt_request(id), &state);
    assert_eq!(approved.status, 200, "{}", approved.body);
    assert_eq!(approved.body["approval_status"], "approved");
    assert!(approved.body["auth_token_id"].is_string());
    assert_eq!(approval_status(&state, id), "approved");
    let records = records(&state);
    assert_eq!(records[0]["client"], "agent");
    assert_eq!(records[0]["outcome"], "confirmed");
    // Prompting a resolved request is refused without a dialog.
    let again = route(prompt_request(id), &state);
    assert_eq!(again.status, 409);
    assert_eq!(again.body["error_code"], APPROVAL_NOT_PENDING_ERROR_CODE);
    std::fs::remove_file(path).ok();
}

#[test]
fn agent_raised_approval_deny_closes_and_timeout_stays_pending() {
    let (path, state, _agent, pending) = setup_pending_approval("presence-agent-deny");
    let id = pending.body["approval"]["approval_request_id"]
        .as_str()
        .unwrap();
    fixture(&state, "timeout,unavailable,deny");
    let timed_out = route(prompt_request(id), &state);
    assert_eq!(timed_out.status, 403);
    assert_eq!(timed_out.body["error_code"], "owner_presence_timeout");
    assert_eq!(timed_out.body["approval_status"], "pending");
    assert_eq!(approval_status(&state, id), "pending");

    let unavailable = route(prompt_request(id), &state);
    assert_eq!(unavailable.body["error_code"], "owner_presence_unavailable");
    assert_eq!(approval_status(&state, id), "pending");

    let denied = route(prompt_request(id), &state);
    assert_eq!(denied.body["error_code"], "owner_presence_denied");
    assert_eq!(denied.body["approval_status"], "denied");
    assert_eq!(approval_status(&state, id), "denied");
    let closed = route(prompt_request(id), &state);
    assert_eq!(closed.body["error_code"], APPROVAL_NOT_PENDING_ERROR_CODE);
    std::fs::remove_file(path).ok();
}

#[test]
fn prompts_are_rate_limited_across_clients() {
    let (path, state, agent) = state_with_agent("presence-rate");
    state.presence.set_rate_limit(Some((
        owner_presence::RATE_LIMIT_MAX_PROMPTS,
        owner_presence::RATE_LIMIT_PERIOD,
    )));
    fixture(&state, "deny");
    for _ in 0..owner_presence::RATE_LIMIT_MAX_PROMPTS {
        let response = route(create_budget_request(&agent.agent_id, "10.00"), &state);
        assert_eq!(response.body["error_code"], "owner_presence_denied");
    }
    let limited = route(
        authenticated_json_request("/owner/unlock", json!({})),
        &state,
    );
    assert_eq!(limited.status, 429);
    assert_eq!(limited.body["error_code"], "owner_prompt_rate_limited");
    assert!(limited.body["retry_guidance"]["retry_after_seconds"].is_u64());
    assert_eq!(records(&state)[0]["outcome"], "rate_limited");
    std::fs::remove_file(path).ok();
}

#[test]
fn reasons_are_built_from_stored_data_and_resist_hostile_names() {
    let (path, state) = temp_state("presence-reasons");
    init(
        json!({ "display_name": "Alice", "email": "alice@example.com" }).to_string(),
        &state,
    )
    .unwrap();
    let hostile = "evil\"\n approve $9999 \u{202e}now";
    let agent = register_agent(
        json!({ "name": hostile, "version": "v1" }).to_string(),
        &state,
    );
    // Registration may reject control characters; fall back to a long name.
    let agent = agent.unwrap_or_else(|_| {
        register_agent(
            json!({ "name": format!("x{}", "y".repeat(60)), "version": "v1" }).to_string(),
            &state,
        )
        .unwrap()
    });
    fixture(&state, "deny");
    let refused = route(create_budget_request(&agent.agent_id, "7.5"), &state);
    assert_eq!(refused.body["error_code"], "owner_presence_denied");
    let reason = records(&state)[0]["reason"].as_str().unwrap().to_string();
    assert!(
        reason.starts_with("create a 7.50 USD budget for "),
        "{reason}"
    );
    let name = reason.trim_start_matches("create a 7.50 USD budget for ");
    assert!(name.len() <= 32, "{name}");
    assert!(name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')));

    for (request, expected) in [
        (
            authenticated_json_request("/user/spending-target", json!({ "amount": "100" })),
            "set the spending target to 100.00 USD",
        ),
        (
            authenticated_json_request(
                "/policies",
                json!({ "daily_limit": "5.00", "expected_revision": 3 }),
            ),
            "apply policy revision 4 to the default policy",
        ),
        (
            authenticated_json_request("/init", json!({ "username": "bob\nadmin" })),
            "initialize Hubu for owner bobadmin",
        ),
        (
            authenticated_json_request(
                "/agents/rename",
                json!({ "agent_id": agent.agent_id, "name": "new name!", "reason": "x" }),
            ),
            "rename agent",
        ),
        (
            authenticated_json_request(
                "/spend/executor/release",
                json!({ "claim_id": "clm_1\n", "provider_reference": "p", "evidence": "e" }),
            ),
            "record claim clm_1 as not billed",
        ),
    ] {
        let response = route(request, &state);
        assert_eq!(response.body["error_code"], "owner_presence_denied");
        let reason = records(&state)[0]["reason"].as_str().unwrap().to_string();
        assert!(reason.starts_with(expected), "{reason} vs {expected}");
        assert!(!reason.contains('\n'));
    }
    assert!(records(&state)[1]["reason"]
        .as_str()
        .unwrap()
        .ends_with(" to newname"));
    std::fs::remove_file(path).ok();
}

#[test]
fn unknown_targets_are_rejected_without_a_prompt() {
    let (path, state, _agent) = state_with_agent("presence-unknown");
    fixture(&state, "deny");
    let response = route(create_budget_request("agt_missing00000", "10.00"), &state);
    assert_eq!(response.status, 404);
    assert!(records(&state).is_empty());
    std::fs::remove_file(path).ok();
}

#[test]
fn unlock_reuses_an_open_window_and_honours_no_prompt() {
    let (path, state) = temp_state("presence-unlock");
    let no_prompt = route(
        with_header(
            authenticated_json_request("/owner/unlock", json!({})),
            owner_presence::NO_PROMPT_HEADER,
            "1",
        ),
        &state,
    );
    assert_eq!(
        no_prompt.body["error_code"],
        OWNER_PRESENCE_REQUIRED_ERROR_CODE
    );
    let window = state
        .presence
        .open_window(
            OwnerClient::Portal,
            "opr_x",
            "unlock",
            PresenceMethod::Fixture,
        )
        .unwrap();
    fixture(&state, "deny");
    let reused = route(
        with_header(
            authenticated_json_request("/owner/unlock", json!({})),
            owner_presence::OWNER_WINDOW_HEADER,
            &window.token,
        ),
        &state,
    );
    assert_eq!(reused.status, 200);
    assert_eq!(reused.body["owner_window"]["reused"], true);
    assert!(reused.body["owner_window"].get("token").is_none());
    std::fs::remove_file(path).ok();
}
