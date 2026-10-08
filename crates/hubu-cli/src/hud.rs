//! Passive demo display. All amounts and decisions come from Hubu's snapshot.
use super::*;
use std::time::{Duration, Instant};

pub(super) fn command(context: &CliContext, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        println!("Watch authoritative agent budgets and governance (read-only)\n\nUsage:\n  hubu hud [--once] [--currency USD]\n\nRefreshes about once per second; Ctrl-C exits. Changed fields have * for one refresh.\nEach row shows the operation budget (or tightest current cap), never a sum.\nALLOW describes the observed decision, not a promise that the next request is allowed.\nUse --once for scripts. Image size is unavailable in Hubu's spend contract.");
        return Ok(());
    }
    let once = take_flag(&mut args, "--once");
    let currency = take_value(&mut args, "--currency")
        .unwrap_or_else(|| "usd".into())
        .to_ascii_lowercase();
    let _: Currency = currency.parse()?;
    ensure_no_args(args)?;
    let interactive = std::io::stdout().is_terminal();
    if !once && !interactive {
        bail!("hubu hud requires a terminal; use --once for scripts");
    }
    let target = context.target()?;
    // Match existing CLI endpoint rules; never forward bearer auth on redirects.
    parse_base_url(&target.base_url)?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let token = target.auth_token()?;
    let mut url = reqwest::Url::parse(&format!("{}/hud", target.base_url.trim_end_matches('/')))?;
    url.query_pairs_mut().append_pair("currency", &currency);
    let mut previous: Option<Value> = None;
    loop {
        let started = Instant::now();
        let mut request = client.get(url.clone());
        if let Some(token) = &token {
            request = request.bearer_auth(token);
        }
        let snapshot = request
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.json::<Value>());
        let output = match snapshot {
            Ok(snapshot) => {
                if snapshot["schema_version"] != "hubu-hud-v1" || !snapshot["rows"].is_array() {
                    bail!("Hubu returned an unsupported HUD projection");
                }
                let output = render(&snapshot, previous.as_ref());
                previous = Some(snapshot);
                output
            }
            Err(_) if !once => {
                let mut output = previous
                    .as_ref()
                    .map(|s| render(s, None))
                    .unwrap_or_else(|| "HUBU HUD\n".into());
                output.push_str("STALE · refresh failed; retrying (last successful snapshot)\n");
                output
            }
            Err(error) => return Err(error.without_url().into()),
        };
        let mut stdout = std::io::stdout().lock();
        if interactive && !once {
            write!(stdout, "\x1b[H\x1b[J")?;
        }
        write!(stdout, "{output}")?;
        stdout.flush()?;
        drop(stdout);
        if once {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(1).saturating_sub(started.elapsed()));
    }
}

fn text(value: &Value, width: usize) -> String {
    value
        .as_str()
        .unwrap_or("—")
        .chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .take(width)
        .collect()
}
fn cents(value: &Value) -> String {
    value
        .as_i64()
        .map(|v| format!("{v}¢"))
        .unwrap_or_else(|| "—".into())
}
fn field(row: &Value, old: Option<&Value>, key: &str, width: usize) -> String {
    let value = if key == "state" {
        text(&row[key], width - 1)
    } else {
        cents(&row[key])
    };
    let changed = old.is_some_and(|r| r[key] != row[key]);
    let value = format!("{value}{}", if changed { "*" } else { " " });
    let padded = format!("{value:>width$}");
    if changed {
        terminal::stdout().accent(padded)
    } else {
        padded
    }
}
fn exact_cents(cost: &Value) -> String {
    let Some(amount) = cost["amount"]
        .as_str()
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
    else {
        return "—".into();
    };
    let Some(scale) = cost["scale"].as_u64().filter(|s| *s <= 18) else {
        return "—".into();
    };
    if scale <= 2 {
        return format!("{}{}¢", amount, "0".repeat((2 - scale) as usize));
    }
    let places = (scale - 2) as usize;
    let padded = format!("{:0>width$}", amount, width = places + 1);
    let split = padded.len() - places;
    let fraction = padded[split..].trim_end_matches('0');
    if fraction.is_empty() {
        format!("{}¢", &padded[..split])
    } else {
        format!("{}.{}¢", &padded[..split], fraction)
    }
}
fn event_line(event: &Value) -> String {
    let provider = match event["provider"]["id"].as_str() {
        Some("provider:black-forest-labs:flux") => "FLUX".into(),
        Some("provider:google:gemini-developer") => "Gemini".into(),
        _ => text(&event["provider"]["display_name"], 16),
    };
    let rules = event["rule_ids"].as_array().map(|rules| {
        rules
            .iter()
            .map(|r| text(r, 22))
            .collect::<Vec<_>>()
            .join(",")
    });
    let rule = rules
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "default / budget".into());
    let rule: String = rule.chars().take(32).collect();
    let status = text(&event["status"], 24);
    let amount = if !event["reserved_cents"].is_null() {
        format!("reserved {}", cents(&event["reserved_cents"]))
    } else {
        format!("requested {}", cents(&event["requested_cents"]))
    };
    let finish = if !event["settled_cost"].is_null() {
        format!(
            " → settled {} (charge {})",
            exact_cents(&event["settled_cost"]),
            cents(&event["budget_charge_cents"])
        )
    } else {
        format!(" · {status}")
    };
    format!(
        "  {provider} · size — · rule {rule}\n  {amount}{finish} {}\n",
        text(&event["currency"], 3).to_ascii_uppercase()
    )
}
fn render(snapshot: &Value, previous: Option<&Value>) -> String {
    let rows = snapshot["rows"].as_array().expect("validated rows");
    let currency = rows
        .first()
        .map(|r| text(&r["currency"], 3).to_ascii_uppercase())
        .unwrap_or_else(|| "USD".into());
    let mut output = format!(
        "HUBU · {currency} · live budgets · Ctrl-C exits\n\n{:<18} {:>10} {:>9} {:>10} {:>9}\n",
        "AGENT", "AVAILABLE", "FROZEN", "CONSUMED", "STATE"
    );
    for row in rows {
        let old = previous
            .and_then(|s| s["rows"].as_array())
            .and_then(|rows| rows.iter().find(|r| r["agent_id"] == row["agent_id"]));
        output.push_str(&format!(
            "{:<18} {} {} {} {}\n",
            text(&row["name"], 18),
            field(row, old, "available_cents", 10),
            field(row, old, "frozen_cents", 9),
            field(row, old, "consumed_cents", 10),
            field(row, old, "state", 9)
        ));
        if row["budget_count"].as_u64().unwrap_or(0) > 1 {
            output.push_str(&format!(
                "  {} of {} budgets: {}\n",
                text(&row["budget_selection"], 10),
                row["budget_count"],
                text(&row["budget_id"], 24)
            ));
        } else if row["budget_id"].is_null() {
            output.push_str("  no current budget in selected currency\n");
        }
        if !row["event"].is_null() {
            output.push_str(&event_line(&row["event"]));
        }
    }
    if rows.is_empty() {
        output.push_str("No registered agents for the active user.\n");
    }
    output.push_str("\n* changed · amounts are budget cents · size unavailable\n");
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(frozen: i64, status: &str, settled: Value) -> Value {
        json!({"rows":[{"agent_id":"agt_1","name":"image-agent","currency":"USD","budget_id":"bgt_1","budget_count":1,"available_cents":200-frozen,"frozen_cents":frozen,"consumed_cents":0,"state":"ALLOW","event":{"provider":{"display_name":"FLUX"},"rule_ids":["draft_images"],"status":status,"requested_cents":8,"reserved_cents":8,"settled_cost":if settled.is_null() {Value::Null} else {json!({"amount":"6","scale":2,"currency":"USD"})},"budget_charge_cents":settled,"currency":"USD"}}]})
    }
    #[test]
    fn reservation_to_settlement_highlights_changed_fields() {
        let pending = snapshot(8, "claimed", Value::Null);
        assert!(render(&pending, None).contains("reserved 8¢ · claimed"));
        let settled = snapshot(0, "settled", json!(6));
        let output = render(&settled, Some(&pending));
        assert!(output.contains("0¢*"));
        assert!(output.contains("reserved 8¢ → settled 6¢"));
        assert!(!render(&settled, Some(&settled)).contains("0¢*"));
    }
    #[test]
    fn exact_settlement_preserves_subcent_cost() {
        assert_eq!(exact_cents(&json!({"amount":"1","scale":3})), "0.1¢");
        assert_eq!(exact_cents(&json!({"amount":"1234","scale":4})), "12.34¢");
        assert_eq!(exact_cents(&json!({"amount":"2","scale":0})), "200¢");
        assert_eq!(exact_cents(&json!({"amount":"600","scale":4})), "6¢");
    }
    #[test]
    fn untrusted_strings_cannot_inject_terminal_controls() {
        let mut s = snapshot(0, "denied", Value::Null);
        s["rows"][0]["name"] = json!("bad\u{001b}[2J\nname");
        s["rows"][0]["event"]["rule_ids"] = json!(["deny\u{001b}[2J"]);
        let output = render(&s, None);
        assert!(!output.contains('\u{001b}'));
        assert!(output.contains("rule deny?[2J"));
    }
}
