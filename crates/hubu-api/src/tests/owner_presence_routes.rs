//! Route-level owner presence tests (v0.3.0 spec, section 13). Every test uses
//! the scripted fixture provider; none depends on a real fingerprint.
use super::*;
use owner_presence::FixtureProvider;

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

const OVERRIDES: &str = "/governance/overrides";

fn override_settings(state: &ServerState) -> Value {
    let response = route(authenticated_get_request(OVERRIDES), state);
    assert_eq!(response.status, 200, "{}", response.body);
    response.body
}

fn override_body(action: &str, enabled: bool) -> Value {
    json!({ "action": action, "enabled": enabled, "acknowledge_warning": true })
}

fn set_override(state: &ServerState, action: &str, enabled: bool) {
    let response = route(
        authenticated_json_request(OVERRIDES, override_body(action, enabled)),
        state,
    );
    assert_eq!(response.status, 200, "{}", response.body);
}

fn agent_post(state: &ServerState, path: &str, body: Value) -> HttpResponse {
    route(
        credential_request_with_body("POST", path, TEST_AGENT_TOKEN, body),
        state,
    )
}

#[test]
fn override_settings_are_strict_and_can_only_change_after_warned_owner_confirmation() {
    let (path, state) = temp_state("override-settings");
    let initial = override_settings(&state);
    let settings = initial["overrides"].as_array().unwrap();
    assert_eq!(settings.len(), 2);
    assert!(settings.iter().all(|setting| setting["enabled"] == false));

    let body = override_body("lower_budget", true);
    assert!(is_owner_authority_rejection(&agent_post(
        &state,
        OVERRIDES,
        body.clone()
    )));
    let rejected = route(
        with_header(
            authenticated_json_request(OVERRIDES, body.clone()),
            owner_presence::NO_PROMPT_HEADER,
            "1",
        ),
        &state,
    );
    assert_eq!(rejected.body["error_code"], "owner_presence_required");
    let rejected = route(
        authenticated_json_request(
            OVERRIDES,
            json!({ "action": "lower_budget", "enabled": true }),
        ),
        &state,
    );
    assert_eq!(rejected.status, 400);

    for never in [
        "change_overrides",
        "raise_budget",
        "create_budget",
        "approve_spend",
        "reconcile",
        "loosen_policy",
        "register_agent",
    ] {
        let rejected = route(
            authenticated_json_request(OVERRIDES, override_body(never, true)),
            &state,
        );
        assert_eq!(rejected.status, 400, "accepted {never}: {}", rejected.body);
    }
    assert!(records(&state).is_empty());
    fixture(&state, "deny");
    let rejected = route(authenticated_json_request(OVERRIDES, body), &state);
    assert_eq!(rejected.body["error_code"], "owner_presence_denied");
    assert_eq!(initial, override_settings(&state));
    let record = &records(&state)[0];
    assert_eq!(record["target"], json!(["lower_budget"]));
    assert!(record["reason"]
        .as_str()
        .unwrap()
        .contains("A prompt-injected agent could cut spending limits"));

    fixture(&state, "confirm");
    set_override(&state, "lower_budget", true);
    drop(state);
    let state = ServerState::new_with_db_path(&path).unwrap();
    assert_eq!(override_settings(&state)["overrides"][0]["enabled"], true);
    set_override(&state, "lower_budget", false);
    assert_eq!(initial, override_settings(&state));
    std::fs::remove_file(path).ok();
}

#[test]
fn lower_budget_override_only_allows_a_decrease_from_the_current_revision() {
    let (path, state, agent) = state_with_agent("override-budget");
    let budget_id = create_test_agent_budget(&state, &agent.agent_id, 1_000)
        .budget
        .budget_id;
    let version_route = format!("/budgets/{budget_id}/versions");
    let update =
        |amount, revision| json!({ "amount_limit": amount, "expected_revision": revision });
    assert!(is_owner_authority_rejection(&agent_post(
        &state,
        &version_route,
        update("5.00", 1)
    )));
    set_override(&state, "lower_budget", true);
    set_override(&state, "add_deny_rule", true);
    fixture(&state, "deny");

    for (route, body) in [
        (version_route.as_str(), update("20.00", 1)),
        (version_route.as_str(), update("10.00", 1)),
        (
            "/budgets",
            json!({ "agent_id": agent.agent_id, "amount": "20.00" }),
        ),
        (
            "/spend/approval/resolve",
            json!({ "approval_request_id": "fixture", "decision": "approve" }),
        ),
        (
            "/spend/executor/settle",
            json!({ "claim_id": "fixture", "evidence": "fixture" }),
        ),
        (OVERRIDES, override_body("add_deny_rule", false)),
    ] {
        let rejected = agent_post(&state, route, body);
        assert!(
            is_owner_authority_rejection(&rejected),
            "{route}: {}",
            rejected.body
        );
    }
    let lowered = agent_post(&state, &version_route, update("5.00", 1));
    assert_eq!(lowered.status, 200, "{}", lowered.body);
    assert_eq!(
        lowered.body["current_budget"]["amount_limit"]["display"],
        "5.00"
    );
    for (amount, revision) in [("2.00", 1), ("7.00", 2)] {
        assert!(is_owner_authority_rejection(&agent_post(
            &state,
            &version_route,
            update(amount, revision)
        )));
    }
    let audit = records(&state);
    assert_eq!(audit.len(), 3);
    assert_eq!(audit[0]["outcome"], "delegated");
    assert_eq!(audit[0]["client"], "agent");
    assert_eq!(audit[0]["override_action"], "lower_budget");
    assert_eq!(audit[0]["target"], json!([budget_id]));

    fixture(&state, "confirm");
    set_override(&state, "lower_budget", false);
    assert!(is_owner_authority_rejection(&agent_post(
        &state,
        &version_route,
        update("4.00", 2)
    )));
    let listed = route(authenticated_get_request("/budgets"), &state);
    assert_eq!(listed.body["budgets"][0]["amount_limit"]["display"], "5.00");
    std::fs::remove_file(path).ok();
}

#[test]
fn deny_rule_override_preserves_the_current_policy_and_assignment() {
    let (path, state, agent) = state_with_agent("override-policy");
    let original = "id: demo\nversion: v1\ndefault_effect: needs_approval\nrules: []";
    let with_deny = "id: demo\nversion: v1\ndefault_effect: needs_approval\nrules:
  - id: block_fixture
    effect: deny
    reason: fixture rule
    when: { op: eq, field: merchant, value: { string: fixture } }";
    let applied = route(
        authenticated_json_request("/policies", json!({ "policy_yaml": original })),
        &state,
    );
    assert_eq!(applied.status, 200, "{}", applied.body);
    let revision = |yaml: &str| {
        json!({
            "policy_yaml": yaml,
            "expected_revision": applied.body["revision"],
            "expected_hash": applied.body["payload_hash"],
        })
    };
    assert!(is_owner_authority_rejection(&agent_post(
        &state,
        "/policies",
        revision(with_deny)
    )));
    set_override(&state, "add_deny_rule", true);
    fixture(&state, "deny");

    let mut reassign = revision(with_deny);
    reassign["agent_id"] = json!(agent.agent_id);
    let mut stale = revision(with_deny);
    stale["expected_revision"] = json!(2);
    for rejected in [
        reassign,
        stale,
        revision(&with_deny.replace("needs_approval", "allow")),
        revision(&with_deny.replace("effect: deny", "effect: allow")),
    ] {
        assert!(is_owner_authority_rejection(&agent_post(
            &state,
            "/policies",
            rejected
        )));
    }
    let tightened = agent_post(&state, "/policies", revision(with_deny));
    assert_eq!(tightened.status, 200, "{}", tightened.body);
    assert_eq!(tightened.body["revision"], 2);
    assert_eq!(tightened.body["policy_id"], applied.body["policy_id"]);
    assert_eq!(tightened.body["assignment_changed"], false);
    let rejected = agent_post(
        &state,
        "/policies",
        json!({
            "policy_yaml": original,
            "expected_revision": tightened.body["revision"],
            "expected_hash": tightened.body["payload_hash"],
        }),
    );
    assert!(is_owner_authority_rejection(&rejected));
    let current = route(authenticated_get_request("/policies/show"), &state);
    assert_eq!(current.body["revision"], 2);
    assert_eq!(current.body["policy"]["rules"][0]["effect"], "deny");
    let audit = records(&state);
    assert_eq!(audit.len(), 3);
    assert_eq!(audit[0]["outcome"], "delegated");
    assert_eq!(audit[0]["override_action"], "add_deny_rule");
    assert_eq!(audit[0]["target"], json!([applied.body["policy_id"]]));
    std::fs::remove_file(path).ok();
}

/// Every owner action is refused without the owner's confirmation, whatever
/// else the caller holds: no-prompt gets `owner_presence_required`, a refused dialog
/// gets the refusal, and nothing is ever performed.
#[test]
fn owner_actions_are_refused_without_confirmation() {
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

/// No owner window (owner decision, 2026-11-19): a confirmation authorizes
/// only the action its dialog named, so the next action prompts again.
#[test]
fn each_owner_action_prompts_once_and_confirmation_does_not_carry_over() {
    let (path, state, agent) = state_with_agent("presence-per-action");
    fixture(&state, "confirm,deny,confirm");
    let first = route(
        with_header(
            create_budget_request(&agent.agent_id, "10.00"),
            owner_presence::CLIENT_HEADER,
            "portal",
        ),
        &state,
    );
    assert_eq!(first.status, 200, "{}", first.body);
    assert!(first.body.get("owner_window").is_none());
    let budget_id = first.body["budget"]["budget_id"]
        .as_str()
        .unwrap()
        .to_string();
    let raise = |amount: &str| {
        authenticated_json_request(
            &format!("/budgets/{budget_id}/versions"),
            json!({ "amount_limit": amount, "expected_revision": 1, "reason": "more" }),
        )
    };
    // Right after a confirmation, the next action still prompts: denied.
    let denied = route(raise("20.00"), &state);
    assert_eq!(denied.body["error_code"], "owner_presence_denied");
    // And the one after prompts again: confirmed.
    let raised = route(raise("20.00"), &state);
    assert_eq!(raised.status, 200, "{}", raised.body);

    let records = records(&state);
    let outcomes = records
        .iter()
        .map(|record| record["outcome"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(outcomes, vec!["confirmed", "denied", "confirmed"]);
    assert_eq!(records[2]["client"], "portal");
    assert_eq!(records[2]["method"], "fixture");
    assert_eq!(
        records[2]["reason"],
        "create a 10.00 USD budget for design-agent"
    );
    assert_eq!(records[0]["action"], "budget.version.create");
    assert_eq!(
        records[0]["reason"],
        "set design-agent's budget to 20.00 USD (now 10.00 USD)"
    );
    assert!(records
        .iter()
        .all(|record| record.get("window_record_id").is_none()));
    std::fs::remove_file(path).ok();
}

/// The window routes are gone, and undeclared owner routes are refused
/// without prompting the owner.
#[test]
fn window_routes_are_gone_and_unknown_routes_never_prompt() {
    let (path, state) = temp_state("presence-no-window-routes");
    fixture(&state, "confirm");
    for path_ in ["/owner/unlock", "/owner/lock", "/budgets/raise"] {
        let response = route(authenticated_json_request(path_, json!({})), &state);
        assert_eq!(response.status, 404, "{path_}: {}", response.body);
    }
    assert!(records(&state).is_empty());
    let status = route(
        credential_request("GET", "/owner/presence", TEST_AGENT_TOKEN),
        &state,
    );
    assert_eq!(status.status, 200);
    assert!(status.body.get("window").is_none());
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
    let limited = route(create_budget_request(&agent.agent_id, "10.00"), &state);
    assert_eq!(limited.status, 429);
    assert_eq!(limited.body["error_code"], "owner_prompt_rate_limited");
    assert!(limited.body["retry_guidance"]["retry_after_seconds"].is_u64());
    assert_eq!(records(&state)[0]["outcome"], "rate_limited");
    let status = route(authenticated_get_request("/owner/presence"), &state);
    assert_eq!(status.body["refusals_remaining"], 0);
    std::fs::remove_file(path).ok();
}

/// Confirmed prompts never count toward the limit, so an owner working
/// through many changes is not slowed down.
#[test]
fn confirmed_prompts_are_not_rate_limited() {
    let (path, state, agent) = state_with_agent("presence-rate-confirm");
    state.presence.set_rate_limit(Some((
        owner_presence::RATE_LIMIT_MAX_PROMPTS,
        owner_presence::RATE_LIMIT_PERIOD,
    )));
    fixture(&state, "confirm");
    for _ in 0..(owner_presence::RATE_LIMIT_MAX_PROMPTS * 2) {
        // Repeated creates fail as overlapping budgets after the owner
        // confirms; what matters is that the prompt is never rate limited.
        let response = route(create_budget_request(&agent.agent_id, "1.00"), &state);
        assert_ne!(response.status, 429, "{}", response.body);
    }
    assert!(records(&state)
        .iter()
        .all(|record| record["outcome"] == "confirmed"));
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
                json!({ "claim_id": "00000000-0000-4000-8000-000000000456", "provider_reference": "p", "evidence": "e" }),
            ),
            "record claim 00000000-0000-4000-8000-000000000456 as not billed",
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

/// Codex P2: a confirmed agent-raised approval whose resolution fails must not
/// report `approved`.
#[test]
fn agent_raised_approval_reports_the_stored_status_when_resolution_fails() {
    let (path, state, agent, pending) = setup_pending_approval("presence-agent-fail");
    let id = pending.body["approval"]["approval_request_id"]
        .as_str()
        .unwrap();
    let budgets = route(authenticated_get_request("/budgets"), &state);
    for budget in budgets.body["budgets"].as_array().unwrap() {
        if budget["agent_id"] == json!(agent.agent_id) {
            revoke_budget(
                json!({ "budget_id": budget["budget_id"] }).to_string(),
                &state,
            )
            .unwrap();
        }
    }
    fixture(&state, "confirm");
    let response = route(prompt_request(id), &state);
    assert_eq!(response.status, 400, "{}", response.body);
    assert_eq!(response.body["approval_status"], "pending");
    assert_eq!(approval_status(&state, id), "pending");
    std::fs::remove_file(path).ok();
}

/// Codex P2: presence checks accept the same currency spellings as the
/// handlers, so `"USD"` still creates a budget.
#[test]
fn budget_reasons_accept_any_currency_case() {
    let (path, state, agent) = state_with_agent("presence-currency");
    for currency in ["USD", " usdc "] {
        let response = route(
            authenticated_json_request(
                "/budgets",
                json!({ "agent_id": agent.agent_id, "amount": "3.00", "currency": currency }),
            ),
            &state,
        );
        assert_eq!(response.status, 200, "{currency}: {}", response.body);
    }
    let reasons = records(&state)
        .iter()
        .map(|record| record["reason"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        reasons,
        vec![
            "create a 3.00 USDC budget for design-agent",
            "create a 3.00 USD budget for design-agent"
        ]
    );
    std::fs::remove_file(path).ok();
}

/// Dialog first (spec 5.4, owner decision 2026-11-19): the owner confirms,
/// then the handler runs all its checks. An invalid request fails after
/// confirmation with the handler's error and writes nothing.
#[test]
fn invalid_owner_requests_fail_after_confirmation_without_writing() {
    let (path, state, agent) = state_with_agent("presence-dialog-first");
    fixture(&state, "confirm");
    let policies_before = route(authenticated_get_request("/policies"), &state).body;
    for body in [
        json!({ "policy_yaml": "not: [valid" }),
        json!({ "daily_limit": "5.00", "unexpected": true }),
    ] {
        let response = route(
            authenticated_json_request("/policies", body.clone()),
            &state,
        );
        assert_eq!(response.status, 400, "{body}: {}", response.body);
    }
    assert_eq!(
        route(authenticated_get_request("/policies"), &state).body,
        policies_before
    );
    for body in [
        json!({ "agent_id": agent.agent_id, "amount": "5.00", "starting_at": "yesterday" }),
        json!({ "agent_id": agent.agent_id, "amount": "5.00", "extra": 1 }),
    ] {
        let response = route(authenticated_json_request("/budgets", body.clone()), &state);
        assert_eq!(response.status, 400, "{body}: {}", response.body);
    }
    let budgets = route(authenticated_get_request("/budgets?all=true"), &state);
    assert_eq!(budgets.body["budgets"].as_array().unwrap().len(), 0);
    let response = route(
        authenticated_json_request(
            "/spend/executor/settle",
            json!({ "claim_id": "00000000-0000-4000-8000-000000000456",
                    "provider_reference": "p", "evidence": "e" }),
        ),
        &state,
    );
    assert!(response.status >= 400, "{}", response.body);
    // Each one was confirmed first: the dialog came before the checks.
    let records = records(&state);
    assert_eq!(records.len(), 5);
    assert!(records
        .iter()
        .all(|record| record["outcome"] == "confirmed"));
    std::fs::remove_file(path).ok();
}

/// The dialog text still needs the amount and the named agent or budget, so a
/// request without them is refused with no dialog.
#[test]
fn requests_without_dialog_inputs_get_no_dialog() {
    let (path, state, agent) = state_with_agent("presence-dialog-inputs");
    fixture(&state, "deny");
    for (route_path, body, status) in [
        (
            "/budgets",
            json!({ "agent_id": agent.agent_id, "amount": "ten" }),
            400,
        ),
        (
            "/budgets",
            json!({ "agent_id": "agt_missing00000", "amount": "1.00" }),
            404,
        ),
        ("/user/spending-target", json!({}), 400),
    ] {
        let response = route(authenticated_json_request(route_path, body.clone()), &state);
        assert_eq!(
            response.status, status,
            "{route_path} {body}: {}",
            response.body
        );
    }
    assert!(records(&state).is_empty());
    std::fs::remove_file(path).ok();
}

/// Codex P1 (spec 9): a tightening write whose record cannot be stored does
/// not run.
#[test]
fn tightening_does_not_run_when_its_record_cannot_be_written() {
    let (path, state, agent) = state_with_agent("presence-tighten-record");
    let budget = create_test_agent_budget(&state, &agent.agent_id, 1_000);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("DROP TABLE owner_presence_records;")
        .unwrap();
    let response = route(
        authenticated_json_request(
            "/budgets/revoke",
            json!({ "budget_id": budget.budget.budget_id }),
        ),
        &state,
    );
    assert_eq!(response.status, 500, "{}", response.body);
    assert_eq!(response.body["error_code"], "owner_presence_record_failed");
    let budgets = route(authenticated_get_request("/budgets"), &state);
    let status = budgets.body["budgets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["budget_id"] == json!(budget.budget.budget_id))
        .map(|b| b["status"].clone());
    assert_eq!(status, Some(json!("active")), "{}", budgets.body);
    std::fs::remove_file(path).ok();
}

/// Codex P1 (spec 5.4): `X-Hubu-No-Prompt` is honoured on agent-raised
/// approval prompts too.
#[test]
fn agent_raised_approval_honours_no_prompt() {
    let (path, state, _agent, pending) = setup_pending_approval("presence-agent-no-prompt");
    let id = pending.body["approval"]["approval_request_id"]
        .as_str()
        .unwrap();
    fixture(&state, "confirm");
    let response = route(
        with_header(prompt_request(id), owner_presence::NO_PROMPT_HEADER, "1"),
        &state,
    );
    assert_eq!(response.status, 403, "{}", response.body);
    assert_eq!(
        response.body["error_code"],
        OWNER_PRESENCE_REQUIRED_ERROR_CODE
    );
    assert_eq!(response.body["approval_status"], "pending");
    assert_eq!(approval_status(&state, id), "pending");
    assert!(records(&state).is_empty(), "no dialog may be raised");
    std::fs::remove_file(path).ok();
}
