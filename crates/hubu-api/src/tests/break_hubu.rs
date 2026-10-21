//! Episode 01 exercises concurrent authorization callers against one account.
//! Settlement receipts are synthetic test fixtures; no provider is contacted.
use super::*;
use std::{collections::BTreeSet, sync::Barrier, time::Instant};

fn read(state: &ServerState, path: &str) -> Value {
    let response = route(authenticated_get_request(path), state);
    assert_eq!(response.status, 200, "{path}: {}", response.body);
    response.body
}

fn budget_snapshot(state: &ServerState) -> Value {
    let budgets = list_budgets(state, false).unwrap();
    assert_eq!(budgets.budgets.len(), 1);
    let budget = &budgets.budgets[0];
    json!({
        "available_units": budget.remaining_amount.units.parse::<i64>().unwrap(),
        "reserved_units": budget.frozen_amount.units.parse::<i64>().unwrap(),
        "consumed_units": budget.consumed_amount.units.parse::<i64>().unwrap(),
    })
}

#[test]
fn break_hubu_episode_01_hundred_concurrent_callers_cannot_overdraw_one_dollar() {
    let started = Instant::now();
    let mut observations = Vec::new();
    let scenario: Value = serde_json::from_str(include_str!(
        "../../../../examples/break-hubu/scenario.json"
    ))
    .unwrap();
    assert_eq!(scenario["schema_version"], 1);
    assert_eq!(scenario["registered_accounts"], 1);
    assert_eq!(scenario["currency"], "USD");
    assert_eq!(scenario["scale"], ACCOUNTING_SCALE);
    assert_eq!(scenario["provider_mode"], "synthetic_executor_settlement");
    let workers_count = usize::try_from(scenario["workers"].as_u64().unwrap()).unwrap();
    let request_units = scenario["request_units"].as_i64().unwrap();
    let budget_units = scenario["budget_units"].as_i64().unwrap();
    assert_eq!(
        (workers_count, request_units, budget_units),
        (100, 30_000, 1_000_000)
    );
    let request_amount = Amount::from_units(request_units).display();
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("episode-01.sqlite");
    let state = ServerState::new_with_db_path(&database).unwrap();
    init(
        json!({"display_name":"Break Hubu demo","email":"break-hubu@example.com"}).to_string(),
        &state,
    )
    .unwrap();
    // walkthrough:setup:start
    let agent = register_agent(
        json!({"name":"break-hubu-episode-01","version":"v1"}).to_string(),
        &state,
    )
    .unwrap();
    // Policy intentionally permits all requests: the shared budget is the gate.
    let policy_yaml = "id: break_hubu_episode_01\nversion: v1\ndefault_effect: allow\nrules: []\n";
    add_policy(
        json!({
            "agent_id":agent.agent_id,
            "policy_yaml":policy_yaml
        })
        .to_string(),
        &state,
    )
    .unwrap();
    let budget = create_test_agent_budget(&state, &agent.agent_id, budget_units / 10_000);
    let initial = budget_snapshot(&state);
    assert_eq!(
        initial,
        json!({"available_units":budget_units,"reserved_units":0,"consumed_units":0})
    );

    // walkthrough:setup:end
    observations.push(json!({
        "id":"setup", "elapsed_ms":started.elapsed().as_millis(),
        "data":{"agent_id":agent.agent_id,"account_id":agent.account_id,
            "budget_id":budget.budget.budget_id,"snapshot":initial}
    }));
    println!("[setup] temporary SQLite episode-01.sqlite; one registered agent/account; budget={budget_units} USD units at scale 6; policy=allow");

    // walkthrough:burst:start
    let state = Arc::new(state);
    let barrier = Arc::new(Barrier::new(workers_count + 1));
    let workers: Vec<_> = (0..workers_count)
        .map(|worker| {
            let state = Arc::clone(&state);
            let barrier = Arc::clone(&barrier);
            let account = agent.account_id.clone();
            let amount = request_amount.clone();
            std::thread::spawn(move || {
                let operation_key = format!("break-hubu-episode-01-worker-{worker:03}");
                barrier.wait();
                let request = json!({
                    "operation_key":operation_key,"account_id":account,
                    "amount":amount,"currency":"usd",
                    "merchant":"gongbu.image",
                    "task_id":"break-hubu-episode-01",
                    "reason":"Episode 01 synthetic three-cent request"
                });
                let response = route(
                    authenticated_json_request("/spend/authorize", request.clone()),
                    &state,
                );
                (worker, operation_key, request, response)
            })
        })
        .collect();
    observations.push(json!({
        "id":"burst", "elapsed_ms":started.elapsed().as_millis(),
        "data":{"spawned_callers":workers.len(),"barrier_participants":workers_count + 1,
            "request_units":request_units}
    }));
    println!("[burst] spawned {} caller threads; /spend/authorize dispatch starts at the {}-participant barrier", workers.len(), workers_count + 1);
    barrier.wait();
    // walkthrough:burst:end
    let responses: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().expect("authorization worker should finish"))
        .collect();
    assert_eq!(responses.len(), workers_count);
    assert!(responses.iter().all(|(_, _, _, r)| r.status == 200));
    let allowed: Vec<_> = responses
        .iter()
        .filter(|(_, _, _, r)| r.body["decision"] == "allow")
        .collect();
    let denied: Vec<_> = responses
        .iter()
        .filter(|(_, _, _, r)| r.body["decision"] == "deny")
        .collect();
    assert_eq!(allowed.len(), 33);
    assert_eq!(denied.len(), 67);
    assert!(denied.iter().all(|(_, _, _, r)| {
        r.body["auth_token_id"].is_null()
            && r.body["budget_hold"].is_null()
            && r.body["reasons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|reason| reason == "budget does not have enough remaining balance")
    }));
    let allowed_decisions: BTreeSet<_> = allowed
        .iter()
        .map(|(_, _, _, r)| r.body["decision_id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(allowed_decisions.len(), 33);
    assert_eq!(
        allowed
            .iter()
            .map(|(_, _, _, r)| r.body["auth_token_id"].as_str().unwrap())
            .collect::<BTreeSet<_>>()
            .len(),
        33
    );
    // Hold every successful authorization before any settlement can free funds.
    let reserved = budget_snapshot(&state);
    assert_eq!(
        reserved,
        json!({"available_units":10_000,"reserved_units":990_000,"consumed_units":0})
    );
    {
        let repository = state.governance.lock().unwrap();
        let holds = repository.load_budget_holds().unwrap();
        let tokens = repository.load_spend_auth_tokens().unwrap();
        assert_eq!(holds.len(), 33);
        assert_eq!(tokens.len(), 33);
        assert!(repository.load_executor_claims().unwrap().is_empty());
        assert_eq!(
            holds
                .iter()
                .map(|h| h.spend_decision_id.to_string())
                .collect::<BTreeSet<_>>(),
            allowed_decisions
        );
        assert_eq!(
            tokens
                .iter()
                .map(|t| t.spend_decision_id.to_string())
                .collect::<BTreeSet<_>>(),
            allowed_decisions
        );
    }
    let ledger_path = format!(
        "/ledger/transactions?agent_id={}&budget_id={}&limit=100",
        agent.agent_id, budget.budget.budget_id
    );
    assert!(read(&state, &ledger_path)["transactions"]
        .as_array()
        .unwrap()
        .is_empty());

    observations.push(json!({
        "id":"authorized", "elapsed_ms":started.elapsed().as_millis(),
        "data":{"allowed":allowed.len(),"denied":denied.len(),"holds":33,"tokens":33,
            "claims":0,"transactions":0,"snapshot":reserved,
            "decision_ids":allowed_decisions}
    }));
    println!("[authorized] callers={} allowed={} denied={}; holds=33 tokens=33 claims=0; available=10000 reserved=990000 consumed=0", responses.len(), allowed.len(), denied.len());

    // walkthrough:settlement:start
    let mut claim_ids = BTreeSet::new();
    let mut settlement_ids = BTreeSet::new();
    for (worker, _, _, response) in &allowed {
        let claim = claim_executor_spend(
            json!({
                "contract_version":"hubu-spend-executor-v4.5",
                "spend_auth_token_id":response.body["auth_token_id"],
                "account_id":agent.account_id,
                "amount":{"units":request_units.to_string(),"scale":6,"currency":"usd"},
                "merchant":"gongbu.image"
            })
            .to_string(),
            &state,
        )
        .unwrap();
        assert!(claim_ids.insert(claim.claim_id.clone()));
        let settlement = finalize_executor_spend(
            json!({
                "claim_id":claim.claim_id,
                "receipt":{
                    "actual_vendor_cost":{"amount":3,"scale":2,"currency":"USD"},
                    "provider_request_id":format!("synthetic-episode-01-{worker:03}"),
                    "price_model_snapshot":{
                        "provider":"synthetic-episode-01","model":"fixture",
                        "quoted_amount":3,"quoted_scale":2,"pricing_unit":"request",
                        "currency":"USD","frozen_pricing_revision":"fixture-v1"
                    },
                    "artifact_reference":format!("fixture://episode-01/{worker:03}")
                }
            })
            .to_string(),
            &state,
            true,
            None,
        )
        .unwrap();
        assert!(settlement_ids.insert(settlement["settlement_id"].as_str().unwrap().to_owned()));
    }
    // walkthrough:settlement:end
    let final_snapshot = budget_snapshot(&state);
    assert_eq!(
        final_snapshot,
        json!({"available_units":10_000,"reserved_units":0,"consumed_units":990_000})
    );
    let ledger = read(&state, &ledger_path);
    let transactions = ledger["transactions"].as_array().unwrap();
    assert_eq!(transactions.len(), 33);
    assert!(ledger["next_cursor"].is_null());
    assert_eq!(ledger["coverage"]["consumed_amount"]["units"], "990000");
    assert_eq!(
        ledger["coverage"]["recorded_budget_charges"]["units"],
        "990000"
    );
    assert_eq!(ledger["coverage"]["unaccounted_consumption"]["units"], "0");
    assert_eq!(ledger["coverage"]["pending_hold_count"], 0);
    assert_eq!(
        transactions
            .iter()
            .map(|t| t["authorization_id"].as_str().unwrap().to_owned())
            .collect::<BTreeSet<_>>(),
        allowed_decisions
    );
    assert_eq!(
        transactions
            .iter()
            .map(|transaction| transaction["settlement_id"].as_str().unwrap().to_owned())
            .collect::<BTreeSet<_>>(),
        settlement_ids
    );
    for transaction in transactions {
        assert_eq!(transaction["kind"], "external_provider");
        assert_eq!(transaction["budget_charge_delta"]["units"], "30000");
        assert_eq!(transaction["budget_charge_delta"]["scale"], 6);
        assert_eq!(
            transaction["effective_cost"],
            json!({"amount":"3","scale":2,"currency":"usd"})
        );
        let entries = transaction["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        for entry in entries {
            assert_eq!(
                entry["amount"],
                json!({"amount":"30000000000000000","scale":18,"currency":"usd"})
            );
        }
        assert!(entries.iter().any(|entry| {
            entry["direction"] == "debit" && entry["account_kind"] == "AgentSpendExpense"
        }));
        assert!(entries.iter().any(|entry| {
            entry["direction"] == "credit" && entry["account_kind"] == "ExternallyBilledClearing"
        }));
    }
    observations.push(json!({
        "id":"settled", "elapsed_ms":started.elapsed().as_millis(),
        "data":{"transaction_count":transactions.len(),"claim_ids":claim_ids,
            "settlement_ids":settlement_ids,"snapshot":final_snapshot,
            "coverage":ledger["coverage"]}
    }));
    println!("[settled] 33 synthetic executor receipts; 33 external_provider transactions; available=10000 reserved=0 consumed=990000; unaccounted=0");

    // walkthrough:persistence:start
    // Reopen durable state rather than trusting only the live manager balance.
    drop(state);
    let reopened = ServerState::new_with_db_path(&database).unwrap();
    let reopened_snapshot = budget_snapshot(&reopened);
    let reopened_ledger = read(&reopened, &ledger_path);
    let transactions_match = reopened_ledger["transactions"] == ledger["transactions"];
    assert_eq!(reopened_snapshot, final_snapshot);
    assert!(transactions_match);
    // walkthrough:persistence:end
    {
        let repository = reopened.governance.lock().unwrap();
        let claims = repository.load_executor_claims().unwrap();
        assert_eq!(claims.len(), 33);
        assert_eq!(
            claims
                .iter()
                .map(|claim| claim.id.to_string())
                .collect::<BTreeSet<_>>(),
            claim_ids
        );
        assert_eq!(
            claims
                .iter()
                .map(|claim| claim.settlement_id.as_ref().unwrap().to_string())
                .collect::<BTreeSet<_>>(),
            settlement_ids
        );
        assert!(claims
            .iter()
            .all(|claim| claim.status == SpendExecutorClaimStatus::Settled));
        assert_eq!(
            claims
                .iter()
                .map(|claim| claim.operation_key.as_str())
                .collect::<BTreeSet<_>>(),
            allowed
                .iter()
                .map(|(_, key, _, _)| key.as_str())
                .collect::<BTreeSet<_>>()
        );
        assert_eq!(
            repository
                .load_budget_holds()
                .unwrap()
                .iter()
                .map(|h| h.spend_decision_id.to_string())
                .collect::<BTreeSet<_>>(),
            allowed_decisions
        );
    }
    let governance = read(
        &reopened,
        &format!(
            "/governance/summary?agent_id={}&outcomes=automatic_deny",
            agent.agent_id
        ),
    );
    assert_eq!(governance["summary"]["distinct_attempts"], 67);
    assert_eq!(governance["summary"]["distinct_operations"], 67);

    observations.push(json!({
        "id":"reopened", "elapsed_ms":started.elapsed().as_millis(),
        "data":{"snapshot":reopened_snapshot,"transactions_match":transactions_match,
            "transaction_count":reopened_ledger["transactions"].as_array().unwrap().len(),
            "settled_claims":33,"denied_attempts":governance["summary"]["distinct_attempts"]}
    }));
    println!("[reopened] fresh SQLite connection: same budget; all 33 transactions and settled claims persisted; 67 distinct denied attempts");

    if let Some(destination) = env::var_os("HUBU_BREAK_EVIDENCE_DIR") {
        let requests: Vec<_> = responses
            .iter()
            .map(|(worker, key, _, response)| {
                json!({
                    "worker":worker,"operation_key":key,"decision":response.body["decision"],
                    "decision_id":response.body["decision_id"],"reason":response.body["reasons"],
                    "amount_units":request_units
                })
            })
            .collect();
        let example = |row: &(usize, String, Value, HttpResponse)| {
            let (_, _, request, response) = row;
            json!({"request":request,"response":{
                "status":response.status,"decision":response.body["decision"],
                "decision_id":response.body["decision_id"],"reasons":response.body["reasons"],
                "has_authorization_token":!response.body["auth_token_id"].is_null(),
                "has_budget_hold":!response.body["budget_hold"].is_null()
            }})
        };
        let evidence = json!({
            "schema_version":2,
            "setup":{
                "agent":{"name":"break-hubu-episode-01","version":"v1",
                    "agent_id":agent.agent_id,"account_id":agent.account_id},
                "budget":{"budget_id":budget.budget.budget_id,"limit_units":budget_units,
                    "currency":"USD","scale":6},
                "policy":{"id":"break_hubu_episode_01","version":"v1","yaml":policy_yaml},
                "storage":{"kind":"temporary_sqlite","database_name":"episode-01.sqlite"},
                "authorization_path":"/spend/authorize","dispatch":"in_process_route",
                "authentication":"test_fixture","barrier_participants":workers_count + 1
            },
            "observations":observations,
            "persistence":{"reopened_snapshot":reopened_snapshot,"transactions_match":transactions_match,
                "transaction_count":reopened_ledger["transactions"].as_array().unwrap().len()},
            "examples":{"allowed":example(allowed[0]),"denied":example(denied[0])},
            "scenario":{
                "workers":workers_count,"registered_accounts":1,"budget_units":budget_units,
                "request_units":request_units,"currency":"USD","scale":6,
                "provider_mode":"synthetic_executor_settlement",
                "scope":"One registered agent/account issues 100 concurrent requests through API handlers; synthetic executor settlement; no external provider or real invoice",
                "agent_id":agent.agent_id,"account_id":agent.account_id,"budget_id":budget.budget.budget_id
            },
            "requests":requests,
            "summary":{
                "allowed":allowed.len(),"denied":denied.len(),"settled":transactions.len(),"errors":0,
                "authorized_units":990_000,"settled_units":990_000,"released_units":0
            },
            "snapshots":{"initial":initial,"reserved":reserved,"final":final_snapshot},
            "ledger":ledger,"governance":governance,
            "assertions":[
                {"id":"callers_completed","name":"100 concurrent callers returned decisions","passed":true},
                {"id":"budget_decisions","name":"33 requests allowed and 67 denied by remaining budget","passed":true},
                {"id":"holds_before_settlement","name":"all successful holds existed before settlement","passed":true},
                {"id":"denied_no_authority","name":"denied attempts received no token, hold or executor claim","passed":true},
                {"id":"balanced_settlements","name":"33 distinct authorizations settled with balanced ledger entries","passed":true},
                {"id":"ledger_reconciled","name":"exact ledger charges reconcile to 990000 units with no unaccounted consumption","passed":true},
                {"id":"remaining_below_request","name":"10000 units remain, insufficient for the next 30000-unit request","passed":true},
                {"id":"durable_reopen","name":"budget and ledger survive reopening SQLite","passed":true}
            ]
        });
        let serialized = serde_json::to_string_pretty(&evidence).unwrap();
        for (_, _, _, response) in &allowed {
            assert!(!serialized.contains(response.body["auth_token_id"].as_str().unwrap()));
        }
        assert!(!serialized.contains(TEST_AUTH_TOKEN));
        let destination = PathBuf::from(destination);
        fs::create_dir_all(&destination).unwrap();
        fs::write(destination.join("episode-01-evidence.json"), serialized).unwrap();
    }
}
