use super::*;

fn phone_setup(name: &str) -> (PathBuf, ServerState, RegisterAgentHttpResponse, String) {
    let (path, mut state, agent, pending) =
        setup_pending_approval_with_lease_config(name, LeaseConfig::default(), "/spend/authorize");
    state.approval_push = Some(
        approval_push::ApprovalPush::new(
            "http://127.0.0.1:9/",
            "http://127.0.0.1:8787/spend/approval/phone",
            "private-phone",
        )
        .unwrap(),
    );
    let id = pending.body["decision_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let record = state.spend.lock().unwrap().decision_record(&id).unwrap();
    let token = approval_push::signed_token(
        &record.id.to_string(),
        record.created_at.timestamp() + 600,
        &state.auth.approval_token_hash,
    );
    (path, state, agent, token)
}

fn phone_action(token: &str, decision: &str) -> HttpRequest {
    let mut request = public_request("POST", approval_push::CALLBACK_PATH);
    request
        .headers
        .insert("host".into(), "127.0.0.1:8787".into());
    request
        .headers
        .insert("content-type".into(), "application/json".into());
    request.body = json!({"token":token,"decision":decision}).to_string();
    request
}

#[test]
fn phone_approval_resumes_same_authorization_without_exposing_token_and_survives_restart() {
    let (path, state, agent, token) = phone_setup("phone-resume");
    let approved = route(phone_action(&token, "approve"), &state);
    assert_eq!(approved.status, 200);
    assert_eq!(approved.body["status"], "approved");
    assert!(approved.body.get("auth_token_id").is_none());
    assert!(list_ledger(&state).unwrap().transactions.is_empty());
    let body = json!({"operation_key":"phone-resume-operation", "account_id":agent.account_id,"amount_cents":600,"reason":"requires human approval","merchant":"Acme Cafe"});
    let resumed = route(
        authenticated_json_request("/spend/authorize", body.clone()),
        &state,
    );
    assert_eq!(resumed.body["decision"], "allow");
    assert_eq!(resumed.body["idempotent_replay"], true);
    assert_eq!(route(phone_action(&token, "deny"), &state).status, 403);
    drop(state);
    let mut restarted = ServerState::new_with_db_path(&path).unwrap();
    restarted.approval_push = Some(
        approval_push::ApprovalPush::new(
            "http://127.0.0.1:9/",
            "http://127.0.0.1:8787/spend/approval/phone",
            "private-phone",
        )
        .unwrap(),
    );
    assert_eq!(
        route(phone_action(&token, "approve"), &restarted).status,
        403
    );
    let replay = route(
        authenticated_json_request("/spend/authorize", body),
        &restarted,
    );
    assert_eq!(replay.body["auth_token_id"], resumed.body["auth_token_id"]);
    drop(restarted);
    let _ = fs::remove_file(path);
}

#[test]
fn phone_denial_is_clean_and_never_spends_or_reserves() {
    let (path, state, agent, token) = phone_setup("phone-deny");
    assert_eq!(
        route(phone_action(&token, "deny"), &state).body["status"],
        "denied"
    );
    let replay = route(
        authenticated_json_request(
            "/spend",
            json!({"operation_key":"phone-deny-operation","account_id":agent.account_id,"amount_cents":600,"reason":"requires human approval","merchant":"Acme Cafe"}),
        ),
        &state,
    );
    assert_eq!(replay.body["decision"], "deny");
    assert!(replay.body["budget_hold"].is_null());
    assert!(list_ledger(&state).unwrap().transactions.is_empty());
    drop(state);
    let _ = fs::remove_file(path);
}

#[test]
fn phone_tokens_reject_tampering_expiry_browser_and_wrong_host() {
    let (path, state, _, token) = phone_setup("phone-security");
    let mut altered = token.clone();
    altered.push('0');
    assert_eq!(route(phone_action(&altered, "approve"), &state).status, 403);
    assert!(approval_push::resolve_action(
        phone_action(&token, "approve"),
        &state,
        Utc::now() + chrono::Duration::seconds(601)
    )
    .is_err());
    for (key, value) in [
        ("origin", "http://evil.test"),
        ("host", "evil.test"),
        ("x-forwarded-for", "127.0.0.1"),
        ("content-type", "text/plain"),
    ] {
        let mut request = phone_action(&token, "approve");
        request.headers.insert(key.into(), value.into());
        assert_eq!(route(request, &state).status, 403);
    }
    let mut request = phone_action(&token, "approve");
    request.method = "GET".into();
    assert_eq!(route(request, &state).status, 403);
    let id = token.split('.').next().unwrap();
    assert_eq!(
        route(
            approval_json_request(json!({"approval_request_id":id,"decision":"approve"})),
            &state
        )
        .status,
        400
    );
    assert_eq!(
        get_spend_approval(Some(id), &state).unwrap().status,
        "pending"
    );
    drop(state);
    let _ = fs::remove_file(path);
}

#[test]
fn phone_competing_actions_have_one_durable_winner() {
    let (path, state, _, token) = phone_setup("phone-race");
    let results = std::thread::scope(|scope| {
        let first = scope.spawn(|| route(phone_action(&token, "approve"), &state));
        let second = scope.spawn(|| route(phone_action(&token, "deny"), &state));
        [first.join().unwrap().status, second.join().unwrap().status]
    });
    assert_eq!(results.iter().filter(|status| **status == 200).count(), 1);
    assert_eq!(results.iter().filter(|status| **status == 403).count(), 1);
    assert!(list_ledger(&state).unwrap().transactions.is_empty());
    drop(state);
    let _ = fs::remove_file(path);
}

#[test]
fn phone_configuration_rejects_public_wildcard_dns_and_mismatched_bind() {
    for url in [
        "http://0.0.0.0/",
        "http://8.8.8.8/",
        "http://localhost/",
        "http://user@127.0.0.1/",
        "http://127.0.0.1/?secret=yes",
    ] {
        assert!(approval_push::ApprovalPush::new(
            url,
            "http://127.0.0.1:8787/spend/approval/phone",
            "phone"
        )
        .is_err());
    }
    let push = approval_push::ApprovalPush::new(
        "http://192.168.1.2/",
        "http://100.64.0.2:8787/spend/approval/phone",
        "phone",
    )
    .unwrap();
    assert!(push.validate_bind("0.0.0.0:8787").is_err());
    assert!(push.validate_bind("127.0.0.1:8787").is_err());
    assert!(push.validate_bind("100.64.0.2:8787").is_ok());
}

#[test]
fn phone_delivery_failure_keeps_pending_and_payload_omits_internal_ids_and_reason() {
    let (path, state, _, token) = phone_setup("phone-delivery");
    let id = token.split('.').next().unwrap().parse().unwrap();
    let decision = state.spend.lock().unwrap().decision_record(&id).unwrap();
    let push = state.approval_push.as_ref().unwrap();
    let payload = push.payload(&decision, &state).unwrap();
    assert!(payload["message"].as_str().unwrap().contains("$6.00"));
    assert!(!payload["message"]
        .as_str()
        .unwrap()
        .contains("requires human approval"));
    assert!(!payload["message"]
        .as_str()
        .unwrap()
        .contains(&decision.id.to_string()));
    assert!(push
        .notify(&decision.id.to_string(), &state, Utc::now())
        .is_err());
    assert_eq!(
        get_spend_approval(Some(&decision.id.to_string()), &state)
            .unwrap()
            .status,
        "pending"
    );
    assert!(list_ledger(&state).unwrap().transactions.is_empty());
    drop(state);
    let _ = fs::remove_file(path);
}
#[test]
fn phone_mock_delivery_sends_actions_once_and_private_listener_hides_agent_routes() {
    let (path, mut state, _, token) = phone_setup("phone-mock");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    state.approval_push = Some(
        approval_push::ApprovalPush::new(
            &format!("http://{address}/"),
            "http://127.0.0.1:8787/spend/approval/phone",
            "private-phone",
        )
        .unwrap(),
    );
    let id = token.split('.').next().unwrap();
    std::thread::scope(|scope| {
        let mock = scope.spawn(|| {
            let (mut stream, _) = listener.accept().unwrap();
            let raw = read_http_request(&mut stream, Instant::now() + HTTP_READ_TIMEOUT).unwrap();
            let request = parse_request(&raw).unwrap();
            let payload: Value = serde_json::from_str(&request.body).unwrap();
            assert_eq!(payload["actions"][0]["label"], "Approve");
            assert_eq!(payload["actions"][1]["label"], "Deny");
            let action: Value =
                serde_json::from_str(payload["actions"][0]["body"].as_str().unwrap()).unwrap();
            assert_eq!(action["token"], token);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .unwrap();
        });
        state
            .approval_push
            .as_ref()
            .unwrap()
            .notify(id, &state, Utc::now())
            .unwrap();
        mock.join().unwrap();
    });
    state
        .approval_push
        .as_ref()
        .unwrap()
        .notify(id, &state, Utc::now())
        .unwrap();
    listener.set_nonblocking(true).unwrap();
    assert!(listener.accept().is_err());
    let phone_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let phone_address = phone_listener.local_addr().unwrap();
    std::thread::scope(|scope| {
        let server = scope.spawn(|| {
            let (stream, _) = phone_listener.accept().unwrap();
            handle_connection_on_listener(stream, &state, true).unwrap();
        });
        let mut client = TcpStream::connect(phone_address).unwrap();
        client
            .write_all(b"GET /spend/approval HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let mut result = String::new();
        client.read_to_string(&mut result).unwrap();
        assert!(result.starts_with("HTTP/1.1 404"));
        server.join().unwrap();
    });
    drop(state);
    let _ = fs::remove_file(path);
}
