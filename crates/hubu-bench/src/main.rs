use std::{
    cmp,
    collections::BTreeMap,
    env, fmt,
    io::{Read, Write},
    net::{Shutdown, TcpStream},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8787";

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let config = Config::from_args(env::args().skip(1).collect())?;
    if config.help {
        print_help();
        return Ok(());
    }

    let mut client = HubuClient::new(config.base_url.clone())?;
    let scenario = setup_scenario(&mut client, &config)?;
    let report = run_benchmark(&client, &scenario, &config)?;
    print_report(&config, &scenario, &report);

    if report.error_rate() > config.max_error_rate {
        bail!(
            "error rate {:.2}% exceeded max {:.2}%",
            report.error_rate() * 100.0,
            config.max_error_rate * 100.0
        );
    }
    if let Some(max_p95_ms) = config.max_p95_ms {
        if report.latency_p95_ms() > max_p95_ms as f64 {
            bail!(
                "p95 latency {:.2} ms exceeded max {} ms",
                report.latency_p95_ms(),
                max_p95_ms
            );
        }
    }
    if !report.correct {
        bail!("correctness checks failed");
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct Config {
    base_url: String,
    agent_count: usize,
    rps: u64,
    duration: Duration,
    workers: usize,
    amount_cents: i64,
    budget_cents: i64,
    daily_limit_cents: i64,
    fail_every: Option<u64>,
    max_error_rate: f64,
    max_p95_ms: Option<u64>,
    help: bool,
}

impl Config {
    fn from_args(mut args: Vec<String>) -> Result<Self> {
        let help = take_flag(&mut args, "--help") || take_flag(&mut args, "-h");
        let agent_count = take_value(&mut args, "--agents")
            .unwrap_or_else(|| "4".to_string())
            .parse::<usize>()
            .context("--agents must be a positive integer")?;
        let rps = take_value(&mut args, "--rps")
            .unwrap_or_else(|| "8".to_string())
            .parse::<u64>()
            .context("--rps must be a positive integer")?;
        let duration_seconds = take_value(&mut args, "--duration-seconds")
            .unwrap_or_else(|| "10".to_string())
            .parse::<u64>()
            .context("--duration-seconds must be a positive integer")?;
        let workers = take_value(&mut args, "--workers")
            .map(|value| {
                value
                    .parse::<usize>()
                    .context("--workers must be a positive integer")
            })
            .transpose()?
            .unwrap_or_else(|| cmp::min(32, cmp::max(1, agent_count)));
        let amount_cents = take_value(&mut args, "--amount-cents")
            .unwrap_or_else(|| "100".to_string())
            .parse::<i64>()
            .context("--amount-cents must be a positive integer")?;
        let default_total = (rps as i64)
            .saturating_mul(duration_seconds as i64)
            .saturating_mul(amount_cents)
            .saturating_mul(2)
            .max(10_000);
        let budget_cents = take_value(&mut args, "--budget-cents")
            .map(|value| {
                value
                    .parse::<i64>()
                    .context("--budget-cents must be positive")
            })
            .transpose()?
            .unwrap_or(default_total);
        let daily_limit_cents = take_value(&mut args, "--daily-limit-cents")
            .map(|value| {
                value
                    .parse::<i64>()
                    .context("--daily-limit-cents must be positive")
            })
            .transpose()?
            .unwrap_or(amount_cents);
        let fail_every = take_value(&mut args, "--fail-every")
            .map(|value| {
                value
                    .parse::<u64>()
                    .context("--fail-every must be positive")
            })
            .transpose()?;
        let max_error_rate = take_value(&mut args, "--max-error-rate")
            .unwrap_or_else(|| "0.00".to_string())
            .parse::<f64>()
            .context("--max-error-rate must be a number from 0.0 to 1.0")?;
        let max_p95_ms = take_value(&mut args, "--max-p95-ms")
            .map(|value| {
                value
                    .parse::<u64>()
                    .context("--max-p95-ms must be positive")
            })
            .transpose()?;
        let base_url = take_value(&mut args, "--url")
            .or_else(|| env::var("HUBU_URL").ok())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());

        if !args.is_empty() {
            bail!("unexpected arguments: {}", args.join(" "));
        }
        if !help {
            if agent_count == 0 {
                bail!("--agents must be greater than zero");
            }
            if rps == 0 {
                bail!("--rps must be greater than zero");
            }
            if duration_seconds == 0 {
                bail!("--duration-seconds must be greater than zero");
            }
            if workers == 0 {
                bail!("--workers must be greater than zero");
            }
            if amount_cents <= 0 || budget_cents <= 0 || daily_limit_cents <= 0 {
                bail!("amount, budget, and daily limit values must be positive");
            }
            if !(0.0..=1.0).contains(&max_error_rate) {
                bail!("--max-error-rate must be from 0.0 to 1.0");
            }
            if matches!(fail_every, Some(0)) {
                bail!("--fail-every must be greater than zero");
            }
        }

        Ok(Self {
            base_url,
            agent_count,
            rps,
            duration: Duration::from_secs(duration_seconds),
            workers,
            amount_cents,
            budget_cents,
            daily_limit_cents,
            fail_every,
            max_error_rate,
            max_p95_ms,
            help,
        })
    }

    fn planned_requests(&self) -> u64 {
        self.rps.saturating_mul(self.duration.as_secs())
    }
}

#[derive(Debug)]
struct Scenario {
    agents: Vec<String>,
    budget_id: String,
}

fn setup_scenario(client: &mut HubuClient, config: &Config) -> Result<Scenario> {
    client.post(
        "/init",
        json!({
            "display_name": "Hubu Benchmark User",
            "email": "bench@example.com",
        }),
    )?;

    let mut agents = Vec::with_capacity(config.agent_count);
    for index in 0..config.agent_count {
        let agent = client.post(
            "/agents/register",
            json!({
                "name": format!("bench-agent-{index:04}"),
                "version": "bench-local",
            }),
        )?;
        let agent_id = string_at(&agent, "agent_id")?.to_string();
        client.post(
            "/policies",
            json!({
                "agent_id": agent_id,
                "daily_limit_cents": config.daily_limit_cents,
            }),
        )?;
        agents.push(string_at(&agent, "agent_id")?.to_string());
    }

    let budget = client.post(
        "/budgets",
        json!({
            "amount_cents": config.budget_cents,
            "starting_at": null,
            "ending_before": null,
        }),
    )?;
    let budget_id = budget
        .get("budget")
        .and_then(|budget| budget.get("budget_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("budget response missing budget.budget_id"))?
        .to_string();

    Ok(Scenario { agents, budget_id })
}

fn run_benchmark(client: &HubuClient, scenario: &Scenario, config: &Config) -> Result<BenchReport> {
    let planned = config.planned_requests();
    let next_index = Arc::new(AtomicU64::new(0));
    let results = Arc::new(Mutex::new(Vec::with_capacity(planned as usize)));
    let start = Instant::now();
    let interval_nanos = 1_000_000_000_u64 / config.rps;
    let worker_count = cmp::min(config.workers, planned.max(1) as usize);

    thread::scope(|scope| {
        for _ in 0..worker_count {
            let next_index = Arc::clone(&next_index);
            let results = Arc::clone(&results);
            let mut client = client.clone();

            scope.spawn(move || loop {
                let index = next_index.fetch_add(1, Ordering::Relaxed);
                if index >= planned {
                    break;
                }
                let due = start + Duration::from_nanos(interval_nanos.saturating_mul(index));
                let now = Instant::now();
                if due > now {
                    thread::sleep(due - now);
                }

                let result = submit_spend(&mut client, scenario, config, index);
                results
                    .lock()
                    .expect("results lock should not poison")
                    .push(result);
            });
        }
    });

    let elapsed = start.elapsed();
    let mut results = Arc::try_unwrap(results)
        .map_err(|_| anyhow!("benchmark results still shared"))?
        .into_inner()
        .map_err(|_| anyhow!("benchmark results lock poisoned"))?;
    results.sort_by_key(|result| result.index);

    let budgets = client.get("/budgets")?;
    let ledger = client.get("/ledger")?;
    Ok(BenchReport::new(results, elapsed, config, budgets, ledger))
}

fn submit_spend(
    client: &mut HubuClient,
    scenario: &Scenario,
    config: &Config,
    index: u64,
) -> RequestResult {
    let agent_index = index as usize % scenario.agents.len();
    let merchant = if config
        .fail_every
        .map(|value| value > 0 && index > 0 && index % value == 0)
        .unwrap_or(false)
    {
        "fail"
    } else {
        "bench-merchant"
    };
    let started = Instant::now();
    let response = client.post(
        "/spend",
        json!({
            "agent_id": scenario.agents[agent_index],
            "amount_cents": config.amount_cents,
            "reason": format!("bench-spend-{index}"),
            "merchant": merchant,
        }),
    );
    let latency = started.elapsed();

    match response {
        Ok(value) => RequestResult {
            index,
            agent_index,
            latency,
            outcome: SpendOutcome::from_response(&value),
            error: None,
        },
        Err(error) => RequestResult {
            index,
            agent_index,
            latency,
            outcome: SpendOutcome::default(),
            error: Some(error.to_string()),
        },
    }
}

#[derive(Debug)]
struct RequestResult {
    index: u64,
    agent_index: usize,
    latency: Duration,
    outcome: SpendOutcome,
    error: Option<String>,
}

#[derive(Debug, Default)]
struct SpendOutcome {
    decision: Option<String>,
    payment_status: Option<String>,
    hold_status: Option<String>,
    has_auth_token: bool,
}

impl SpendOutcome {
    fn from_response(value: &Value) -> Self {
        Self {
            decision: value
                .get("decision")
                .and_then(Value::as_str)
                .map(str::to_string),
            payment_status: value
                .get("payment")
                .and_then(|payment| payment.get("status"))
                .and_then(Value::as_str)
                .map(str::to_string),
            hold_status: value
                .get("budget_hold")
                .and_then(|hold| hold.get("status"))
                .and_then(Value::as_str)
                .map(str::to_string),
            has_auth_token: value.get("auth_token_id").and_then(Value::as_str).is_some(),
        }
    }
}

#[derive(Debug)]
struct BenchReport {
    elapsed: Duration,
    total: usize,
    errors: usize,
    allow_decisions: usize,
    needs_approval_decisions: usize,
    deny_decisions: usize,
    auth_tokens: usize,
    payments_succeeded: usize,
    payments_failed: usize,
    holds_settled: usize,
    holds_released: usize,
    latency_ms: Vec<f64>,
    per_agent_successes: BTreeMap<usize, usize>,
    budget_consumed_cents: i64,
    budget_frozen_cents: i64,
    budget_remaining_cents: i64,
    ledger_transactions: usize,
    correct: bool,
    correctness_notes: Vec<String>,
}

impl BenchReport {
    fn new(
        results: Vec<RequestResult>,
        elapsed: Duration,
        config: &Config,
        budgets: Value,
        ledger: Value,
    ) -> Self {
        let mut latency_ms = results
            .iter()
            .map(|result| result.latency.as_secs_f64() * 1000.0)
            .collect::<Vec<_>>();
        latency_ms.sort_by(|left, right| left.total_cmp(right));

        let mut per_agent_successes = BTreeMap::new();
        for result in &results {
            if result.error.is_none()
                && result.outcome.payment_status.as_deref() == Some("succeeded")
            {
                *per_agent_successes.entry(result.agent_index).or_insert(0) += 1;
            }
        }

        let total = results.len();
        let errors = results
            .iter()
            .filter(|result| result.error.is_some())
            .count();
        let allow_decisions = count_decisions(&results, "allow");
        let needs_approval_decisions = count_decisions(&results, "needs_approval");
        let deny_decisions = count_decisions(&results, "deny");
        let auth_tokens = results
            .iter()
            .filter(|result| result.outcome.has_auth_token)
            .count();
        let payments_succeeded = count_payment_status(&results, "succeeded");
        let payments_failed = count_payment_status(&results, "failed");
        let holds_settled = count_hold_status(&results, "settled");
        let holds_released = count_hold_status(&results, "released");
        let ledger_transactions = ledger
            .get("transactions")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default();
        let (budget_consumed_cents, budget_frozen_cents, budget_remaining_cents) =
            budget_totals(&budgets);

        let mut correctness_notes = Vec::new();
        let expected_total = config.planned_requests() as usize;
        if total != expected_total {
            correctness_notes.push(format!(
                "recorded {total} results but planned {expected_total} requests"
            ));
        }
        if errors > 0 {
            correctness_notes.push(format!(
                "{errors} requests returned transport or HTTP errors"
            ));
        }
        if needs_approval_decisions > 0 || deny_decisions > 0 {
            correctness_notes.push(format!(
                "unexpected non-allow decisions: needs_approval={needs_approval_decisions}, deny={deny_decisions}"
            ));
        }
        if auth_tokens != allow_decisions {
            correctness_notes.push(format!(
                "auth token count {auth_tokens} did not match allow decisions {allow_decisions}"
            ));
        }
        if holds_settled != payments_succeeded {
            correctness_notes.push(format!(
                "settled holds {holds_settled} did not match successful payments {payments_succeeded}"
            ));
        }
        if holds_released != payments_failed {
            correctness_notes.push(format!(
                "released holds {holds_released} did not match failed payments {payments_failed}"
            ));
        }
        if ledger_transactions != payments_succeeded {
            correctness_notes.push(format!(
                "ledger transaction count {ledger_transactions} did not match successful payments {payments_succeeded}"
            ));
        }
        let expected_consumed = payments_succeeded as i64 * config.amount_cents;
        if budget_consumed_cents != expected_consumed {
            correctness_notes.push(format!(
                "budget consumed {budget_consumed_cents} cents did not match expected {expected_consumed} cents"
            ));
        }
        if budget_frozen_cents != 0 {
            correctness_notes.push(format!(
                "budget still has {budget_frozen_cents} frozen cents after benchmark"
            ));
        }

        Self {
            elapsed,
            total,
            errors,
            allow_decisions,
            needs_approval_decisions,
            deny_decisions,
            auth_tokens,
            payments_succeeded,
            payments_failed,
            holds_settled,
            holds_released,
            latency_ms,
            per_agent_successes,
            budget_consumed_cents,
            budget_frozen_cents,
            budget_remaining_cents,
            ledger_transactions,
            correct: correctness_notes.is_empty(),
            correctness_notes,
        }
    }

    fn achieved_rps(&self) -> f64 {
        if self.elapsed.is_zero() {
            return 0.0;
        }
        self.total as f64 / self.elapsed.as_secs_f64()
    }

    fn error_rate(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.errors as f64 / self.total as f64
    }

    fn latency_p50_ms(&self) -> f64 {
        percentile(&self.latency_ms, 50.0)
    }

    fn latency_p95_ms(&self) -> f64 {
        percentile(&self.latency_ms, 95.0)
    }

    fn latency_p99_ms(&self) -> f64 {
        percentile(&self.latency_ms, 99.0)
    }

    fn latency_max_ms(&self) -> f64 {
        self.latency_ms.last().copied().unwrap_or_default()
    }
}

fn print_report(config: &Config, scenario: &Scenario, report: &BenchReport) {
    println!("Hubu local spend benchmark");
    println!("configuration");
    println!("  url: {}", config.base_url);
    println!("  agents: {}", config.agent_count);
    println!("  target_rps: {}", config.rps);
    println!("  duration_seconds: {:.2}", config.duration.as_secs_f64());
    println!("  workers: {}", config.workers);
    println!("  amount_cents: {}", config.amount_cents);
    println!("  budget_cents: {}", config.budget_cents);
    println!("  budget_id: {}", scenario.budget_id);
    println!("performance");
    println!("  requests: {}", report.total);
    println!("  elapsed_seconds: {:.3}", report.elapsed.as_secs_f64());
    println!("  achieved_rps: {:.2}", report.achieved_rps());
    println!("  latency_p50_ms: {:.2}", report.latency_p50_ms());
    println!("  latency_p95_ms: {:.2}", report.latency_p95_ms());
    println!("  latency_p99_ms: {:.2}", report.latency_p99_ms());
    println!("  latency_max_ms: {:.2}", report.latency_max_ms());
    println!("correctness");
    println!("  correct: {}", report.correct);
    println!("  errors: {}", report.errors);
    println!("  error_rate: {:.2}%", report.error_rate() * 100.0);
    println!("  allow_decisions: {}", report.allow_decisions);
    println!(
        "  needs_approval_decisions: {}",
        report.needs_approval_decisions
    );
    println!("  deny_decisions: {}", report.deny_decisions);
    println!("  auth_tokens: {}", report.auth_tokens);
    println!("  payments_succeeded: {}", report.payments_succeeded);
    println!("  payments_failed: {}", report.payments_failed);
    println!("  holds_settled: {}", report.holds_settled);
    println!("  holds_released: {}", report.holds_released);
    println!("  ledger_transactions: {}", report.ledger_transactions);
    println!("  budget_consumed_cents: {}", report.budget_consumed_cents);
    println!("  budget_frozen_cents: {}", report.budget_frozen_cents);
    println!(
        "  budget_remaining_cents: {}",
        report.budget_remaining_cents
    );
    if !report.correctness_notes.is_empty() {
        println!("correctness_notes");
        for note in &report.correctness_notes {
            println!("  - {note}");
        }
    }
    println!("per_agent_successes");
    for (agent_index, successes) in &report.per_agent_successes {
        println!("  agent_{agent_index}: {successes}");
    }
}

fn count_decisions(results: &[RequestResult], decision: &str) -> usize {
    results
        .iter()
        .filter(|result| result.outcome.decision.as_deref() == Some(decision))
        .count()
}

fn count_payment_status(results: &[RequestResult], status: &str) -> usize {
    results
        .iter()
        .filter(|result| result.outcome.payment_status.as_deref() == Some(status))
        .count()
}

fn count_hold_status(results: &[RequestResult], status: &str) -> usize {
    results
        .iter()
        .filter(|result| result.outcome.hold_status.as_deref() == Some(status))
        .count()
}

fn budget_totals(value: &Value) -> (i64, i64, i64) {
    value
        .get("budgets")
        .and_then(Value::as_array)
        .and_then(|budgets| budgets.first())
        .map(|budget| {
            (
                integer_at_or_zero(budget, "consumed_amount_cents"),
                integer_at_or_zero(budget, "frozen_amount_cents"),
                integer_at_or_zero(budget, "remaining_amount_cents"),
            )
        })
        .unwrap_or_default()
}

fn integer_at_or_zero(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or_default()
}

fn percentile(sorted_values: &[f64], percentile: f64) -> f64 {
    if sorted_values.is_empty() {
        return 0.0;
    }
    let rank = (percentile / 100.0) * (sorted_values.len().saturating_sub(1) as f64);
    sorted_values[rank.round() as usize]
}

#[derive(Clone)]
struct HubuClient {
    host: String,
    port: u16,
}

impl HubuClient {
    fn new(base_url: String) -> Result<Self> {
        let (host, port) = parse_base_url(&base_url)?;
        Ok(Self { host, port })
    }

    fn get(&self, path: &str) -> Result<Value> {
        self.request_json("GET", path, None)
    }

    fn post(&self, path: &str, body: Value) -> Result<Value> {
        self.request_json("POST", path, Some(body))
    }

    fn request_json(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        let body_text = body.map(|body| body.to_string()).unwrap_or_default();
        let mut stream = TcpStream::connect((self.host.as_str(), self.port))
            .with_context(|| format!("connect to Hubu server at {}:{}", self.host, self.port))?;

        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            self.host,
            self.port,
            body_text.len(),
            body_text
        )?;
        stream.shutdown(Shutdown::Write)?;

        let mut raw = String::new();
        stream.read_to_string(&mut raw)?;
        let (status, response_body) = parse_http_response(&raw)?;
        let json: Value = serde_json::from_str(response_body)
            .with_context(|| format!("parse server response body `{response_body}`"))?;

        if !(200..300).contains(&status) {
            let message = json
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("request failed");
            bail!("server returned HTTP {status}: {message}");
        }

        Ok(json)
    }
}

fn parse_base_url(base_url: &str) -> Result<(String, u16)> {
    let trimmed = base_url
        .strip_prefix("http://")
        .ok_or_else(|| anyhow!("only http:// Hubu URLs are supported"))?;
    let host_port = trimmed.trim_end_matches('/');
    let (host, port) = host_port
        .rsplit_once(':')
        .ok_or_else(|| anyhow!("expected URL like http://127.0.0.1:8787"))?;
    Ok((host.to_string(), port.parse()?))
}

fn parse_http_response(raw: &str) -> Result<(u16, &str)> {
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .ok_or_else(|| anyhow!("invalid HTTP response"))?;
    let status = head
        .lines()
        .next()
        .ok_or_else(|| anyhow!("missing HTTP status line"))?
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| anyhow!("missing HTTP status code"))?
        .parse::<u16>()?;
    Ok((status, body))
}

fn string_at<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("server response missing `{key}`"))
}

fn take_value(args: &mut Vec<String>, key: &str) -> Option<String> {
    let position = args.iter().position(|arg| arg == key)?;
    args.remove(position);
    if position >= args.len() {
        return Some(String::new());
    }
    Some(args.remove(position))
}

fn take_flag(args: &mut Vec<String>, key: &str) -> bool {
    if let Some(position) = args.iter().position(|arg| arg == key) {
        args.remove(position);
        true
    } else {
        false
    }
}

fn print_help() {
    println!(
        "{}",
        HelpText {
            binary: "hubu-bench"
        }
    );
}

struct HelpText {
    binary: &'static str,
}

impl fmt::Display for HelpText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Usage: {} [OPTIONS]", self.binary)?;
        writeln!(f)?;
        writeln!(
            f,
            "Simulate N agents owned by one user making Y spend approval requests/sec."
        )?;
        writeln!(f)?;
        writeln!(f, "Options:")?;
        writeln!(
            f,
            "  --url URL                    Hubu server URL [default: {DEFAULT_BASE_URL}]"
        )?;
        writeln!(
            f,
            "  --agents N                   registered agents to simulate [default: 4]"
        )?;
        writeln!(
            f,
            "  --rps Y                      target spend requests per second [default: 8]"
        )?;
        writeln!(
            f,
            "  --duration-seconds N         benchmark duration [default: 10]"
        )?;
        writeln!(
            f,
            "  --workers N                  client worker threads [default: min(agents, 32)]"
        )?;
        writeln!(
            f,
            "  --amount-cents N             spend amount per request [default: 100]"
        )?;
        writeln!(
            f,
            "  --budget-cents N             user budget [default: 2x planned spend]"
        )?;
        writeln!(
            f,
            "  --daily-limit-cents N        per-agent policy allow limit [default: amount]"
        )?;
        writeln!(
            f,
            "  --fail-every N               use fail merchant every Nth request"
        )?;
        writeln!(
            f,
            "  --max-error-rate N           fail if error rate exceeds N [default: 0.00]"
        )?;
        writeln!(
            f,
            "  --max-p95-ms N               fail if p95 latency exceeds N ms"
        )?;
        writeln!(f, "  -h, --help                   print help")?;
        Ok(())
    }
}
