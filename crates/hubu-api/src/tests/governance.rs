use super::*;

fn get(state: &ServerState, path: &str) -> Value {
    let response = route(authenticated_get_request(path), state);
    assert_eq!(response.status, 200, "{path}: {}", response.body);
    assert_eq!(response.body["schema_version"], "hubu-governance-v1");
    response.body
}

fn encoded(value: &str) -> String {
    value.bytes().map(|byte| format!("%{byte:02X}")).collect()
}

fn invalid(state: &ServerState, path: &str) {
    let response = route(authenticated_get_request(path), state);
    assert_eq!(response.status, 400, "{path}: {}", response.body);
    assert_eq!(response.body["error_code"], "governance_invalid_query");
}

fn absent(state: &ServerState, path: &str) -> Value {
    let response = route(authenticated_get_request(path), state);
    assert_eq!(response.status, 404, "{path}: {}", response.body);
    assert_eq!(response.body["error_code"], "governance_record_not_found");
    response.body
}

fn fixture(effect: &str) -> (tempfile::TempDir, ServerState, RegisterAgentHttpResponse) {
    let dir = tempfile::tempdir().unwrap();
    let state = ServerState::new_with_db_path(dir.path().join("governance.sqlite")).unwrap();
    init(
        json!({"display_name":"Alice","email":"alice@example.com"}).to_string(),
        &state,
    )
    .unwrap();
    let agent = register_agent(
        json!({"name":"governance-agent","version":"v1"}).to_string(),
        &state,
    )
    .unwrap();
    add_policy(
        json!({
            "agent_id":agent.agent_id,
            "policy_yaml":format!("id: governance_fixture\nversion: v1\ndefault_effect: {effect}\nrules: []\n")
        })
        .to_string(),
        &state,
    )
    .unwrap();
    (dir, state, agent)
}

fn authorize(
    state: &ServerState,
    agent: &RegisterAgentHttpResponse,
    operation: &str,
    amount: &str,
    currency: &str,
) -> Value {
    let response = route(
        authenticated_json_request(
            "/spend/authorize",
            json!({
                "operation_key":operation,"account_id":agent.account_id,
                "amount":amount,"currency":currency,"merchant":"PRIVATE_MERCHANT",
                "task_id":"PRIVATE_TASK","reason":"PRIVATE_REASON"
            }),
        ),
        state,
    );
    assert_eq!(response.status, 200, "{}", response.body);
    response.body
}

fn money(units: &str, currency: &str, display: &str) -> Value {
    json!({"units":units,"scale":6,"currency":currency,"display":display})
}

#[test]
fn governance_pages_share_summary_snapshot_and_allowlisted_records() {
    let (_dir, state, agent) = fixture("deny");
    let operation = "PRIVATE key + & % 雪";
    let denied = authorize(&state, &agent, operation, "0.005", "usd");
    assert_eq!(denied["decision"], "deny");
    let url = format!(
        "/governance/records?account_id={}&limit=1",
        agent.account_id
    );
    let first = get(&state, &url);
    let summary = &first["summary"];
    assert_eq!(summary["fact_count"], 2);
    assert_eq!(summary["distinct_attempts"], 1);
    assert_eq!(summary["distinct_operations"], 1);
    assert_eq!(summary["stage_counts_overlap"], true);
    assert_eq!(summary["requested"]["known_attempts"], 1);
    assert_eq!(summary["requested"]["unknown_attempts"], 0);
    assert_eq!(
        summary["automatic_denied_requested"]["per_currency"],
        json!([money("5000", "usd", "0.005")])
    );
    assert_eq!(
        summary["scope"],
        json!({"agent_id":agent.agent_id,"account_id":agent.account_id})
    );
    assert_eq!(first["filters"]["agent_id"], agent.agent_id);
    assert_eq!(first["filters"]["account_id"], agent.account_id);
    let token = encoded(summary["snapshot"]["token"].as_str().unwrap());
    let selected = format!("account_id={}&snapshot={token}", agent.account_id);
    let separate = get(&state, &format!("/governance/summary?{selected}"));
    assert_eq!(separate["summary"], *summary);
    assert_eq!(separate["filters"], first["filters"]);
    let record = &first["records"][0];
    assert!(record["record_id"].as_str().unwrap().starts_with("lev_"));
    assert_eq!(record["agent_id"], agent.agent_id);
    assert_eq!(record["account_id"], agent.account_id);
    assert_eq!(
        record["requested_amount"]["value"],
        money("5000", "usd", "0.005")
    );
    let detail = get(
        &state,
        &format!(
            "/governance/records/{}?{selected}",
            record["record_id"].as_str().unwrap()
        ),
    );
    assert_eq!(detail["record"], *record);
    assert_eq!(detail["summary"], *summary);
    assert!(detail.get("next_cursor").is_none());
    let second = get(
        &state,
        &format!(
            "/governance/records?account_id={}&limit=1&cursor={}",
            agent.account_id,
            encoded(first["next_cursor"].as_str().unwrap())
        ),
    );
    assert_eq!(second["summary"], *summary);
    assert_ne!(second["records"][0]["record_id"], record["record_id"]);
    assert!(second["next_cursor"].is_null());
    assert_eq!(
        authorize(&state, &agent, operation, "0.005", "usd")["decision_id"],
        denied["decision_id"]
    );
    assert_eq!(get(&state, &url), first);

    let owner = authenticated_user_context(&state).unwrap().user_id;
    let internal_agent =
        resolve_agent_id_for_user(&agent.agent_id, &UserContext::new(owner.clone()), &state)
            .unwrap();
    let internal_account = state
        .registration
        .lock()
        .unwrap()
        .account_for_agent(&internal_agent)
        .unwrap()
        .unwrap()
        .id;
    for output in [&first, &second, &separate, &detail] {
        let text = output.to_string();
        for secret in [
            operation,
            "PRIVATE_MERCHANT",
            "PRIVATE_TASK",
            "PRIVATE_REASON",
            "operation_key",
            "correlation",
            "source_evidence",
            "spend_auth_token_id",
            &owner.to_string(),
            &internal_agent.to_string(),
            &internal_account.to_string(),
        ] {
            assert!(!text.contains(secret), "leaked {secret}: {text}");
        }
    }
}

#[test]
fn governance_requires_bearer_and_validates_owned_selectors_before_empty_results() {
    let (_dir, state, alice) = fixture("deny");
    authorize(&state, &alice, "alice-private", "0.005", "usd");
    let alice_page = get(&state, "/governance/records");
    let record_id = alice_page["records"][0]["record_id"].as_str().unwrap();
    let alice_owner = authenticated_user_context(&state).unwrap().user_id;
    for path in [
        "/governance/summary".to_string(),
        "/governance/records".into(),
        format!("/governance/records/{record_id}"),
    ] {
        let mut missing = authenticated_get_request(&path);
        missing.headers.remove("authorization");
        assert_eq!(route(missing, &state).status, 401);
        let mut wrong = authenticated_get_request(&path);
        wrong
            .headers
            .insert("authorization".into(), "Bearer wrong".into());
        assert_eq!(route(wrong, &state).status, 401);
    }
    init(
        json!({"username":"bob-governance","display_name":"Bob","email":"bob@example.com"})
            .to_string(),
        &state,
    )
    .unwrap();
    let bob = register_agent(
        json!({"name":"bob-agent","version":"v1"}).to_string(),
        &state,
    )
    .unwrap();
    let other = register_agent(
        json!({"name":"bob-other-agent","version":"v1"}).to_string(),
        &state,
    )
    .unwrap();
    assert_eq!(get(&state, "/governance/records")["records"], json!([]));
    let bob_empty = get(
        &state,
        &format!("/governance/summary?account_id={}", bob.account_id),
    );
    assert_eq!(bob_empty["summary"]["scope"]["agent_id"], bob.agent_id);
    for suffix in [
        format!("agent_id={}", alice.agent_id),
        format!("account_id={}", alice.account_id),
        format!("agent_id={}&account_id={}", bob.agent_id, other.account_id),
        "agent_id=agt_unknown".into(),
        "account_id=aga_unknown".into(),
        "owner_user_id=someone".into(),
    ] {
        for base in ["/governance/summary", "/governance/records"] {
            invalid(&state, &format!("{base}?{suffix}"));
        }
        invalid(&state, &format!("/governance/records/lev_unknown?{suffix}"));
    }
    let foreign = absent(&state, &format!("/governance/records/{record_id}"));
    assert_eq!(foreign, absent(&state, "/governance/records/lev_unknown"));
    invalid(
        &state,
        &format!(
            "/governance/summary?snapshot={}",
            encoded(alice_page["summary"]["snapshot"]["token"].as_str().unwrap())
        ),
    );
    state.auth.select_owner_user(&alice_owner).unwrap();
    assert_eq!(
        get(&state, "/governance/records")["records"],
        alice_page["records"]
    );
    absent(
        &state,
        &format!("/governance/records/{record_id}?categories=authorization"),
    );
}

#[test]
fn governance_strict_query_and_token_errors_are_safe_and_consistent() {
    let (_dir, state, agent) = fixture("deny");
    authorize(&state, &agent, "strict-private", "0.005", "usd");
    let page = get(&state, "/governance/records?limit=1");
    let record = page["records"][0]["record_id"].as_str().unwrap();
    let detail_path = format!("/governance/records/{record}");
    for base in [
        "/governance/summary",
        "/governance/records",
        detail_path.as_str(),
    ] {
        for suffix in [
            "unknown=x",
            "agent_id=",
            "categories=no_such_category",
            "categories=initial_policy,",
            "outcomes=no_such_outcome",
            "outcomes=policy_deny,,automatic_deny",
            "occurred_from=bad",
            "occurred_before=2026-10-10",
            "occurred_from=2026-10-10T00:00:00Z&occurred_before=2026-10-10T00:00:00Z",
            "occurred_from=2026-10-11T00:00:00Z&occurred_before=2026-10-10T00:00:00Z",
            "snapshot=%",
            "snapshot=%FF",
            "snapshot=%257B",
            "snapshot=",
            "cursor=",
            "categories=%FF",
        ] {
            invalid(&state, &format!("{base}?{suffix}"));
        }
        for repeated in [
            "limit=1&limit=2",
            "limit=1&%6cimit=2",
            "agent_id=a&agent_id=b",
            "categories=initial_policy&categories=authorization",
        ] {
            assert!(
                parse_request(&format!(
                    "GET {base}?{repeated} HTTP/1.1\r\nHost: localhost\r\n\r\n"
                ))
                .is_err(),
                "{base}?{repeated}"
            );
        }
    }
    for limit in ["0", "201", "-1", "x"] {
        invalid(&state, &format!("/governance/records?limit={limit}"));
    }
    for base in ["/governance/summary", detail_path.as_str()] {
        for suffix in ["limit=1", "cursor=whatever"] {
            invalid(&state, &format!("{base}?{suffix}"));
        }
    }
    let token: Value =
        serde_json::from_str(page["summary"]["snapshot"]["token"].as_str().unwrap()).unwrap();
    for (field, value) in [
        ("version", json!(2)),
        ("binding", json!("wrong")),
        ("watermark", json!(u64::MAX)),
        ("extra", json!(true)),
    ] {
        let mut changed = token.clone();
        changed[field] = value;
        invalid(
            &state,
            &format!(
                "/governance/summary?snapshot={}",
                encoded(&changed.to_string())
            ),
        );
    }
    for field in ["event_id", "recorded_at"] {
        let mut changed = token.clone();
        changed["boundary"][field] = json!("PRIVATE_INVALID_BOUNDARY");
        let response = route(
            authenticated_get_request(&format!(
                "/governance/records?snapshot={}",
                encoded(&changed.to_string())
            )),
            &state,
        );
        assert_eq!(response.status, 400);
        assert_eq!(response.body["error_code"], "governance_invalid_query");
        assert!(!response
            .body
            .to_string()
            .contains("PRIVATE_INVALID_BOUNDARY"));
    }
    invalid(
        &state,
        &format!(
            "/governance/summary?snapshot={}",
            encoded(&"x".repeat(20_000))
        ),
    );
    let snapshot = encoded(page["summary"]["snapshot"]["token"].as_str().unwrap());
    invalid(
        &state,
        &format!("/governance/summary?categories=authorization&snapshot={snapshot}"),
    );
    let mut cursor: Value = serde_json::from_str(page["next_cursor"].as_str().unwrap()).unwrap();
    cursor["after"]["occurred_at"] = json!("2000-01-01T00:00:00Z");
    invalid(
        &state,
        &format!(
            "/governance/records?cursor={}",
            encoded(&cursor.to_string())
        ),
    );
    invalid(&state, "/governance/records/not-a-record");
    let normalized = get(&state, "/governance/records?categories=automated_admission,initial_policy,initial_policy&outcomes=automatic_deny,policy_deny,policy_deny");
    assert_eq!(
        normalized["filters"]["categories"],
        json!(["initial_policy", "automated_admission"])
    );
    assert_eq!(
        normalized["filters"]["outcomes"],
        json!(["policy_deny", "automatic_deny"])
    );
}

#[test]
fn governance_raw_query_errors_receive_http_responses_after_authentication() {
    let (_dir, state, _agent) = fixture("deny");
    let state = Arc::new(state);
    for base in [
        "/governance/summary",
        "/governance/records",
        "/governance/records/lev_unknown",
    ] {
        for query in [
            "limit=1&limit=2",
            "limit=1&%6cimit=2",
            "%ZZ=PRIVATE_BAD_KEY",
        ] {
            for authorization in [Some(TEST_AUTH_TOKEN), None, Some("wrong-bearer")] {
                let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                let address = listener.local_addr().unwrap();
                let server_state = Arc::clone(&state);
                let server = std::thread::spawn(move || {
                    let (stream, _) = listener.accept().unwrap();
                    handle_connection(stream, &server_state)
                });
                let mut client = TcpStream::connect(address).unwrap();
                client
                    .set_read_timeout(Some(StdDuration::from_secs(2)))
                    .unwrap();
                let auth_header = authorization
                    .map(|token| format!("Authorization: Bearer {token}\r\n"))
                    .unwrap_or_default();
                client
                    .write_all(
                        format!(
                            "GET {base}?{query} HTTP/1.1\r\nHost: localhost\r\n{auth_header}\r\n"
                        )
                        .as_bytes(),
                    )
                    .unwrap();
                let mut response = String::new();
                client.read_to_string(&mut response).unwrap();
                server.join().unwrap().unwrap();
                let expected_status = if authorization == Some(TEST_AUTH_TOKEN) {
                    400
                } else {
                    401
                };
                assert!(
                    response.starts_with(&format!("HTTP/1.1 {expected_status} ")),
                    "{base}?{query}: {response}"
                );
                let body: Value =
                    serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
                if expected_status == 400 {
                    assert_eq!(body["error_code"], "governance_invalid_query");
                }
                assert!(!body.to_string().contains("PRIVATE_BAD_KEY"));
            }
        }
    }
}

#[test]
fn governance_shared_money_formats_full_width_and_signed_values_exactly() {
    assert_eq!(
        to_json(MoneyHttpResponse::new_wide(i128::MIN, Currency::Usd)),
        money(
            "-170141183460469231731687303715884105728",
            "usd",
            "-170141183460469231731687303715884.105728"
        )
    );
    assert_eq!(
        to_json(MoneyHttpResponse::new_wide(i128::MAX, Currency::Usd)),
        money(
            "170141183460469231731687303715884105727",
            "usd",
            "170141183460469231731687303715884.105727"
        )
    );
    assert_eq!(
        to_json(MoneyHttpResponse::new_wide(-1, Currency::Usdc)),
        money("-1", "usdc", "-0.000001")
    );
    assert_eq!(
        to_json(MoneyHttpResponse::new_wide(0, Currency::Usd)),
        money("0", "usd", "0.00")
    );
    for units in [i64::MIN, i64::MAX, -1, 0, 1, 5_000] {
        assert_eq!(
            MoneyHttpResponse::new_wide(i128::from(units), Currency::Usd),
            MoneyHttpResponse::new(Amount::from_units(units), Currency::Usd)
        );
    }
}

#[test]
fn governance_maps_nested_exact_money_and_wide_per_currency_requested_totals() {
    let (_dir, state, agent) = fixture("deny");
    for key in ["wide-first", "wide-second"] {
        assert_eq!(
            authorize(&state, &agent, key, "9223372036854.775807", "usd")["decision"],
            "deny"
        );
    }
    authorize(&state, &agent, "micro-usdc", "0.000001", "usdc");
    let summary = get(&state, "/governance/summary");
    assert_eq!(summary["summary"]["distinct_attempts"], 3);
    assert_eq!(summary["summary"]["fact_count"], 6);
    let totals = json!([
        money("18446744073709551614", "usd", "18446744073709.551614"),
        money("1", "usdc", "0.000001")
    ]);
    assert_eq!(summary["summary"]["requested"]["per_currency"], totals);
    assert_eq!(
        summary["summary"]["automatic_denied_requested"]["per_currency"],
        totals
    );

    add_policy(json!({"agent_id":agent.agent_id,"policy_yaml":"id: precise_allow\nversion: v1\ndefault_effect: deny\nrules:\n  - id: allow_tiny\n    effect: allow\n    when:\n      op: lte\n      field: amount\n      value:\n        money: \"0.005\"\n    reason: PRIVATE_RULE_REASON\n"}).to_string(), &state).unwrap();
    create_budget(
        json!({"agent_id":agent.agent_id,"amount":"1.000001"}).to_string(),
        &state,
    )
    .unwrap();
    let allowed = authorize(&state, &agent, "precise-allowed", "0.005", "usd");
    assert_eq!(allowed["decision"], "allow");
    let page = get(&state, "/governance/records?categories=authorization");
    let fact = &page["records"][0];
    assert_eq!(
        fact["requested_amount"]["value"],
        money("5000", "usd", "0.005")
    );
    let constraint = &fact["policy"]["value"]["amount_constraints"][0];
    assert_eq!(constraint["configured"], money("5000", "usd", "0.005"));
    assert_eq!(
        constraint["evaluated"]["value"],
        money("5000", "usd", "0.005")
    );
    let budget = &fact["budget"]["value"];
    assert_eq!(budget["requested"], money("5000", "usd", "0.005"));
    for field in ["limit", "available"] {
        assert_eq!(
            budget[field]["value"],
            money("1000001", "usd", "1.000001"),
            "{field}"
        );
    }
    for field in ["consumed", "frozen"] {
        assert_eq!(budget[field]["value"], money("0", "usd", "0.00"), "{field}");
    }
    assert!(!page.to_string().contains("PRIVATE_RULE_REASON"));
}

#[test]
fn governance_snapshot_keeps_human_backlog_after_later_resolution() {
    let (path, state, agent, pending) = setup_pending_approval_with_lease_config_and_merchant(
        "governance-human",
        LeaseConfig::default(),
        "/spend/authorize",
        "gongbu.image",
    );
    let first = get(&state, "/governance/records?limit=1");
    assert_eq!(first["summary"]["backlog"]["pending_human_approvals"], 1);
    let recent_filter = format!(
        "agent_id={}&categories=authorization&occurred_from={}",
        agent.agent_id,
        encoded(&Utc::now().to_rfc3339())
    );
    let recent_pending = get(&state, &format!("/governance/summary?{recent_filter}"));
    assert_eq!(recent_pending["summary"]["fact_count"], 0);
    assert_eq!(
        recent_pending["summary"]["backlog"]["pending_human_approvals"],
        1
    );
    let pending_token = encoded(
        recent_pending["summary"]["snapshot"]["token"]
            .as_str()
            .unwrap(),
    );
    let token = encoded(first["summary"]["snapshot"]["token"].as_str().unwrap());
    let resolved = route(
        approval_json_request(
            json!({"approval_request_id":pending.body["decision_id"],"decision":"deny"}),
        ),
        &state,
    );
    assert_eq!(resolved.status, 200, "{}", resolved.body);
    let frozen = get(&state, &format!("/governance/summary?snapshot={token}"));
    assert_eq!(frozen["summary"], first["summary"]);
    assert_eq!(
        get(
            &state,
            &format!("/governance/summary?{recent_filter}&snapshot={pending_token}")
        ),
        recent_pending
    );
    let detail = get(
        &state,
        &format!(
            "/governance/records/{}?snapshot={token}",
            first["records"][0]["record_id"].as_str().unwrap()
        ),
    );
    assert_eq!(detail["summary"], first["summary"]);
    assert_eq!(detail["record"], first["records"][0]);
    let next = get(
        &state,
        &format!(
            "/governance/records?cursor={}",
            encoded(first["next_cursor"].as_str().unwrap())
        ),
    );
    assert_eq!(next["summary"], first["summary"]);
    let latest = get(&state, "/governance/summary");
    assert_eq!(latest["summary"]["backlog"]["pending_human_approvals"], 0);
    assert!(latest["summary"]["automatic_denials"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(latest["summary"]["stages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|stage| stage["outcome"] == "human_declined" && stage["fact_count"] == 1));
    let filtered = get(
        &state,
        &format!(
            "/governance/summary?agent_id={}&categories=authorization&occurred_from={}",
            agent.agent_id,
            encoded(&Utc::now().to_rfc3339())
        ),
    );
    assert_eq!(filtered["summary"]["fact_count"], 0);
    assert_eq!(filtered["summary"]["backlog"]["pending_human_approvals"], 0);
    std::fs::remove_file(path).ok();
}

#[test]
fn governance_snapshot_keeps_financial_backlog_after_confirmed_reconciliation() {
    let mut config = LeaseConfig::default();
    config
        .lease_profiles
        .get_mut("default")
        .unwrap()
        .claim_ttl_seconds = 1;
    let (path, state, agent, auth) =
        setup_executor_authorization_with_lease_config("governance-financial", config);
    let claim = claim_executor_spend(json!({"spend_auth_token_id":auth.auth_token_id,"account_id":agent.account_id,"amount_cents":500,"merchant":"gongbu.image"}).to_string(), &state).unwrap();
    let owner = authenticated_user_context(&state).unwrap().user_id;
    let persisted = state
        .spend
        .lock()
        .unwrap()
        .executor_claim_records_for_owner(&owner)
        .remove(0);
    std::thread::sleep(std::time::Duration::from_millis(1_100));
    let observer =
        financial_observer::FinancialObserver::start(Arc::clone(&state.governance)).unwrap();
    observer.observe_at(persisted.expires_at);
    drop(observer);
    let first = get(
        &state,
        "/governance/records?categories=reconciliation_entry",
    );
    assert_eq!(first["summary"]["backlog"]["unresolved_reconciliations"], 1);
    assert_eq!(first["records"].as_array().unwrap().len(), 1);
    let token = encoded(first["summary"]["snapshot"]["token"].as_str().unwrap());
    let resolved = finalize_executor_spend(json!({"claim_id":claim.claim_id,"provider_reference":"PRIVATE_PROVIDER_REFERENCE","evidence":"PRIVATE_RECONCILIATION_EVIDENCE"}).to_string(), &state, false, Some(TEST_RECONCILIATION_TOKEN)).unwrap();
    assert_eq!(resolved["status"], "released");
    let frozen = get(
        &state,
        &format!("/governance/records?categories=reconciliation_entry&snapshot={token}"),
    );
    assert_eq!(frozen, first);
    let detail = get(
        &state,
        &format!(
            "/governance/records/{}?categories=reconciliation_entry&snapshot={token}",
            first["records"][0]["record_id"].as_str().unwrap()
        ),
    );
    assert_eq!(detail["summary"], first["summary"]);
    let latest = get(
        &state,
        "/governance/records?categories=reconciliation_resolution",
    );
    assert_eq!(
        latest["summary"]["backlog"]["unresolved_reconciliations"],
        0
    );
    assert_eq!(
        latest["records"][0]["reconciliation_resolution"],
        "vendor_did_not_bill"
    );
    for secret in [
        "PRIVATE_PROVIDER_REFERENCE",
        "PRIVATE_RECONCILIATION_EVIDENCE",
        "reconciled_by_user_id",
        "claim_id",
        "operation_key",
    ] {
        assert!(!latest.to_string().contains(secret), "leaked {secret}");
    }
    std::fs::remove_file(path).ok();
}

#[test]
fn governance_coverage_observation_is_explicit_and_gets_never_write_or_touch_managers() {
    let (dir, state, agent) = fixture("deny");
    let state = Arc::new(state);
    let first = get(&state, "/governance/summary");
    assert_eq!(first["summary"]["snapshot"]["watermark"], 0);
    assert!(first["summary"]["snapshot"]["captured_evidence_as_of"].is_null());
    assert_eq!(first["summary"]["legacy_coverage"]["assessed"], false);
    assert!(first["summary"]["record_coverage"]
        .as_array()
        .unwrap()
        .iter()
        .all(|coverage| coverage["complete_history"] == false));
    assert!(first["summary"]["unsupported_signals"]
        .as_array()
        .unwrap()
        .contains(&json!("provider_execution_failure")));
    let observer =
        financial_observer::FinancialObserver::start(Arc::clone(&state.governance)).unwrap();
    observer.observe_at(Utc::now());
    drop(observer);
    let observed = get(&state, "/governance/summary");
    assert_eq!(observed["summary"]["fact_count"], 0);
    assert_eq!(observed["summary"]["legacy_coverage"]["assessed"], true);
    assert!(
        observed["summary"]["snapshot"]["watermark"]
            .as_u64()
            .unwrap()
            > 0
    );
    let frozen = get(
        &state,
        &format!(
            "/governance/summary?snapshot={}",
            encoded(first["summary"]["snapshot"]["token"].as_str().unwrap())
        ),
    );
    assert_eq!(frozen, first);
    authorize(&state, &agent, "purity-private", "0.005", "usd");
    let selectors = format!(
        "agent_id={}&account_id={}",
        agent.agent_id, agent.account_id
    );
    let page_url = format!("/governance/records?{selectors}");
    let page = get(&state, &page_url);
    let conn = rusqlite::Connection::open(dir.path().join("governance.sqlite")).unwrap();
    let tables = conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").unwrap().query_map([], |row| row.get::<_, String>(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
    for (index, table) in tables.iter().enumerate() {
        for verb in ["INSERT", "UPDATE", "DELETE"] {
            conn.execute_batch(&format!("CREATE TRIGGER governance_read_only_{index}_{verb} BEFORE {verb} ON \"{}\" BEGIN SELECT RAISE(ABORT,'governance GET attempted a write'); END;", table.replace('"', "\"\""))).unwrap();
        }
    }
    let spend = state.spend.lock().unwrap();
    let budgets = state.budgets.lock().unwrap();
    let payments = state.payments.lock().unwrap();
    let registration = state.registration.lock().unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let worker_state = Arc::clone(&state);
    let record = page["records"][0]["record_id"]
        .as_str()
        .unwrap()
        .to_string();
    let worker = std::thread::spawn(move || {
        let replies = [
            "/governance/summary".to_string(),
            "/governance/records".into(),
            format!("/governance/records/{record}"),
        ]
        .map(|path| {
            route(
                authenticated_get_request(&format!("{path}?{selectors}")),
                &worker_state,
            )
        });
        send.send(replies).unwrap();
    });
    let result = receive.recv_timeout(std::time::Duration::from_secs(5));
    drop(registration);
    drop(payments);
    drop(budgets);
    drop(spend);
    worker.join().unwrap();
    for response in
        result.expect("governance reads must not acquire financial/provider manager locks")
    {
        assert_eq!(response.status, 200, "{}", response.body);
        assert_eq!(response.body["summary"], page["summary"]);
    }
    assert_eq!(get(&state, &page_url), page);
    invalid(&state, "/governance/summary?owner_user_id=PRIVATE_OWNER");
    conn.execute_batch("DROP TABLE spend_lifecycle_records")
        .unwrap();
    let unavailable = route(authenticated_get_request("/governance/summary"), &state);
    assert_eq!(unavailable.status, 500);
    assert_eq!(unavailable.body["error_code"], "governance_unavailable");
    assert!(!unavailable
        .body
        .to_string()
        .contains("spend_lifecycle_records"));
    assert!(!unavailable.body.to_string().contains("governance.sqlite"));
}
