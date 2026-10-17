//! Passive budget display: all balances and decisions come from Hubu.
use super::*;
use std::time::{Duration, Instant};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Debug)]
struct Options {
    once: bool,
    currency: String,
    compact: bool,
    events: usize,
    agent: Option<String>,
}
fn options(mut args: Vec<String>) -> Result<Options> {
    let once = take_flag(&mut args, "--once");
    let compact = take_flag(&mut args, "--compact");
    let events = take_value(&mut args, "--events")
        .map(|v| v.parse::<usize>())
        .transpose()?
        .unwrap_or(5);
    if !(1..=10).contains(&events) {
        bail!("--events must be from 1 to 10");
    }
    let agent = take_value(&mut args, "--agent");
    let currency = take_value(&mut args, "--currency")
        .unwrap_or_else(|| "usd".into())
        .to_ascii_lowercase();
    let _: Currency = currency.parse()?;
    ensure_no_args(args)?;
    Ok(Options {
        once,
        currency,
        compact,
        events,
        agent,
    })
}
pub(super) fn command(context: &CliContext, mut args: Vec<String>) -> Result<()> {
    if take_help(&mut args) {
        println!("Watch authoritative agent budgets and recent decisions (read-only)\n\nUsage:\n  hubu watch [--compact] [--events 1..10] [--agent NAME|ID] [--once] [--currency USD]\n\nRefreshes about once per second; Ctrl-C exits. Default: 5 recent decisions.\nEach bar shows the operation budget (or tightest current cap), never a sum.\n--compact shows bars only; --agent filters the snapshot locally.\nUse --once for scripts.");
        return Ok(());
    }
    let opts = options(args)?;
    let interactive = std::io::stdout().is_terminal();
    if !opts.once && !interactive {
        bail!("hubu watch requires a terminal; use --once for scripts");
    }
    let target = context.target()?;
    parse_base_url(&target.base_url)?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let token = target.auth_token()?;
    let mut url = reqwest::Url::parse(&format!("{}/watch", target.base_url.trim_end_matches('/')))?;
    url.query_pairs_mut()
        .append_pair("currency", &opts.currency);
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
                if snapshot["schema_version"] != "hubu-watch-v1"
                    || !snapshot["rows"].is_array()
                    || !snapshot["recent_events"].is_array()
                {
                    bail!("Hubu returned an unsupported watch projection");
                }
                let (height, columns) = terminal_size();
                let output = render(
                    &snapshot,
                    previous.as_ref(),
                    &opts,
                    terminal::stdout(),
                    false,
                    height,
                    layout_width(columns),
                )?;
                previous = Some(snapshot);
                output
            }
            Err(_) if !opts.once => match &previous {
                Some(s) => {
                    let (height, columns) = terminal_size();
                    render(
                        s,
                        None,
                        &opts,
                        terminal::stdout(),
                        true,
                        height,
                        layout_width(columns),
                    )?
                }
                None => terminal::stdout().heading(" HUBU WATCH · STALE · retrying\n"),
            },
            Err(error) => return Err(error.without_url().into()),
        };
        let mut stdout = std::io::stdout().lock();
        if interactive && !opts.once {
            write!(stdout, "\x1b[H\x1b[J")?;
        }
        write!(stdout, "{output}")?;
        stdout.flush()?;
        drop(stdout);
        if opts.once {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(1).saturating_sub(started.elapsed()));
    }
}
/// Terminal rows and columns, defaulting to 24x70 when stdout is not a terminal.
fn terminal_size() -> (usize, usize) {
    #[cfg(unix)]
    {
        let mut size: libc::winsize = unsafe { std::mem::zeroed() };
        // Read terminal dimensions only; ioctl neither mutates Hubu nor the terminal.
        if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) } == 0
            && size.ws_row > 0
        {
            let columns = if size.ws_col > 0 {
                size.ws_col as usize
            } else {
                MIN_WIDTH
            };
            return (size.ws_row as usize, columns);
        }
    }
    (24, MIN_WIDTH)
}
// The layout never shrinks below its 70-column design or stretches past
// where bars stop reading well; agent names get the space in between.
const MIN_WIDTH: usize = 70;
const MAX_WIDTH: usize = 100;
const MIN_NAME: usize = 12;
const MAX_NAME: usize = 24;
fn layout_width(columns: usize) -> usize {
    columns.clamp(MIN_WIDTH, MAX_WIDTH)
}
fn clean(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}
// Terminal cells, not chars: CJK and emoji occupy two columns. Text is split
// into grapheme clusters, the units a terminal draws (flags, ZWJ sequences,
// keycaps, variation selectors and combining marks stay whole), and each is
// measured as a string.
fn units(s: &str) -> Vec<(&str, usize)> {
    s.graphemes(true)
        .map(|g| {
            // A regional-indicator pair is one two-cell flag.
            let flag = g.chars().count() == 2
                && g.chars().all(|c| ('\u{1F1E6}'..='\u{1F1FF}').contains(&c));
            (g, if flag { 2 } else { g.width() })
        })
        .collect()
}
fn display_width(s: &str) -> usize {
    units(s).iter().map(|(_, w)| w).sum()
}
fn text(value: &Value, width: usize) -> String {
    let s = clean(value.as_str().unwrap_or("—"));
    if display_width(&s) <= width {
        return s;
    }
    let mut out = String::new();
    let mut used = 0;
    for (unit, cells) in units(&s) {
        if used + cells > width.saturating_sub(1) {
            break;
        }
        used += cells;
        out.push_str(unit);
    }
    out.push('…');
    out
}
fn pad(value: &str, width: usize) -> String {
    format!(
        "{value}{}",
        " ".repeat(width.saturating_sub(display_width(value)))
    )
}
/// One amount from a row or event: its exact money object, or the legacy
/// `*_cents` field from an older server.
fn money_of(value: &Value, key: &str) -> Option<money::Money> {
    money::money_field(value, key).ok()
}
fn shown(value: &Value, key: &str) -> String {
    money_of(value, key)
        .map(|amount| money::format_money(amount, money::Direction::for_field(key)))
        .unwrap_or_else(|| "—".into())
}
/// Exact micro-units, for bar proportions and percentages.
fn micro_units(row: &Value, key: &str) -> i128 {
    money_of(row, key).map_or(0, |amount| i128::from(amount.amount.units()))
}
fn clock(value: &Value) -> String {
    value
        .as_str()
        .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
        .map(|v| v.with_timezone(&Local).format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "—".into())
}
// Allocate a fixed bar using actual proportions, while retaining visibility of
// every positive segment. Overruns cap only the picture, never the amounts.
fn bar_cells(
    consumed: i128,
    frozen: i128,
    available: i128,
    limit: i128,
    width: usize,
) -> [usize; 3] {
    let used = consumed.max(0) + frozen.max(0);
    let weights = [
        consumed.max(0),
        frozen.max(0),
        if used >= limit { 0 } else { available.max(0) },
    ];
    let total = weights.iter().sum::<i128>().max(1);
    let mut cells = weights.map(|v| {
        if v > 0 {
            ((v * width as i128) / total).max(1) as usize
        } else {
            0
        }
    });
    while cells.iter().sum::<usize>() > width {
        let i = (0..3)
            .filter(|i| cells[*i] > 1)
            .max_by_key(|i| cells[*i])
            .unwrap();
        cells[i] -= 1;
    }
    while cells.iter().sum::<usize>() < width {
        let i = (0..3)
            .max_by_key(|i| weights[*i] * width as i128 - cells[*i] as i128 * total)
            .unwrap();
        cells[i] += 1;
    }
    cells
}
// Wrap before painting. Unicode glyphs and terminal escape bytes never affect
// padding, and extreme monetary values remain intact across continuation lines.
fn line(
    out: &mut String,
    segments: &[(String, Option<terminal::Role>)],
    style: terminal::TerminalStyle,
    width: usize,
) {
    let mut column = 0;
    for (s, role) in segments {
        let mut chunk = String::new();
        for (unit, cells) in units(s) {
            if column + cells > width {
                out.push_str(
                    &role
                        .map(|r| style.paint(r, &chunk))
                        .unwrap_or_else(|| chunk.clone()),
                );
                chunk.clear();
                out.push('\n');
                column = 0;
            }
            chunk.push_str(unit);
            column += cells;
        }
        out.push_str(&role.map(|r| style.paint(r, &chunk)).unwrap_or(chunk));
    }
    out.push('\n');
}
/// The exact provider cost `{amount, scale}` of any scale, never rounded.
fn exact_cost(cost: &Value, currency: &Value) -> String {
    let decimal = cost["scale"]
        .as_u64()
        .filter(|scale| *scale <= 18)
        .and_then(|scale| {
            cost["amount"]
                .as_str()
                .and_then(|amount| money::format_exact_decimal(amount, scale as u32))
        });
    match decimal {
        None => "—".into(),
        Some(decimal) if currency.as_str() == Some("usdc") => format!("{decimal} USDC"),
        Some(decimal) => format!("${decimal}"),
    }
}

fn event_status(event: &Value, single: bool) -> (&'static str, String) {
    let reserved = if event["reserved_cents"].is_null() && event["reserved"].is_null() {
        shown(event, "requested_cents")
    } else {
        shown(event, "reserved_cents")
    };
    match event["status"].as_str() {
        Some("authorized" | "claimed") => ("◐", format!("reserved {reserved}")),
        Some("settled") => (
            "✓",
            format!(
                "{reserved} → {}{}",
                if single { "settled " } else { "" },
                exact_cost(&event["settled_cost"], &event["currency"])
            ),
        ),
        Some("needs_approval") => (
            "‖",
            format!(
                "{} {}",
                shown(event, "requested_cents"),
                if single {
                    "awaiting approval"
                } else {
                    "approval"
                }
            ),
        ),
        Some("denied") => ("✗", format!("{} blocked", shown(event, "requested_cents"))),
        Some("released" | "expired") => ("○", format!("{reserved} released")),
        Some("reconciliation_required") => (
            "!",
            format!(
                "{reserved} {}",
                if single {
                    "needs reconciliation"
                } else {
                    "reconcile"
                }
            ),
        ),
        _ => ("?", text(&event["status"], 24)),
    }
}
fn event_line(event: &Value, single: bool, name_width: usize, width: usize) -> String {
    let provider = match event["provider"]["id"].as_str() {
        Some("provider:black-forest-labs:flux") => "FLUX".into(),
        Some("provider:google:gemini-developer") => "Gemini".into(),
        _ => text(&event["provider"]["display_name"], 6),
    };
    // Fixed columns: time, glyph, agent, provider and status; the rule takes the rest.
    let fixed = if single { 43 } else { 35 + name_width };
    let rule_width = (width - 9).saturating_sub(fixed).max(14);
    let rule = if event["release_reason"].as_str() == Some("executor_rejected_before_claim") {
        "executor_rejected_before_claim".into()
    } else if event["denial_kind"] == "budget" {
        "budget".into()
    } else {
        event["rule_ids"]
            .as_array()
            .and_then(|r| r.first())
            .map(|r| text(r, rule_width))
            .unwrap_or_else(|| "default".into())
    };
    let (glyph, status) = event_status(event, single);
    let agent = if single {
        String::new()
    } else {
        format!(
            "{} ",
            pad(&text(&event["agent_name"], name_width), name_width)
        )
    };
    format!(
        " {}  {glyph} {agent}{} {} {rule}",
        clock(&event["created_at"]),
        pad(&provider, 6),
        pad(&status, if single { 22 } else { 13 })
    )
}
fn render(
    snapshot: &Value,
    previous: Option<&Value>,
    opts: &Options,
    style: terminal::TerminalStyle,
    stale: bool,
    height: usize,
    width: usize,
) -> Result<String> {
    let all = snapshot["rows"].as_array().expect("validated rows");
    let rows: Vec<_> = all
        .iter()
        .filter(|r| {
            opts.agent
                .as_ref()
                .is_none_or(|a| r["agent_id"].as_str() == Some(a) || r["name"].as_str() == Some(a))
        })
        .collect();
    if opts.agent.is_some() && rows.len() != 1 {
        bail!("--agent must identify exactly one agent by name or ID");
    }
    let single = rows.len() == 1;
    // Content spans the width minus a margin (60 columns at the 70-column minimum).
    let content = width - 10;
    let longest = rows
        .iter()
        .map(|r| &r["name"])
        .chain(
            snapshot["recent_events"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|e| &e["agent_name"]),
        )
        .map(|n| display_width(&clean(n.as_str().unwrap_or("—"))))
        .max()
        .unwrap_or(0);
    // Feed lines keep at least 14 columns for the rule after the name, which
    // also leaves bars at least 10 cells; 70 columns therefore keep 12.
    let name_width = longest
        .clamp(MIN_NAME, MAX_NAME)
        .min(content.saturating_sub(48).max(MIN_NAME));
    // Amounts read as columns: decimal points line up across agent rows.
    let has_budget =
        |row: &Value| !row["budget_id"].is_null() && money_of(row, "limit_cents").is_some();
    let used_of = |row: &Value| {
        let used = money_of(row, "consumed_cents")
            .zip(money_of(row, "frozen_cents"))
            .and_then(|(consumed, frozen)| {
                consumed
                    .amount
                    .checked_add(frozen.amount)
                    .map(|amount| money::Money::new(amount, consumed.currency))
            });
        used.map(|used| money::format_money(used, money::Direction::Up))
            .unwrap_or_else(|| "—".into())
    };
    let budgeted: Vec<&Value> = rows.iter().copied().filter(|row| has_budget(row)).collect();
    let used_column =
        money::align_decimals(&budgeted.iter().map(|row| used_of(row)).collect::<Vec<_>>());
    let limit_column = money::align_decimals(
        &budgeted
            .iter()
            .map(|row| shown(row, "limit_cents"))
            .collect::<Vec<_>>(),
    );
    let mut budgeted_index = 0;
    let mut out = String::new();
    let title = if rows.len() > 1 {
        format!(" HUBU WATCH · {} agents", rows.len())
    } else {
        " HUBU WATCH".into()
    };
    let right = if stale {
        "STALE · retrying".into()
    } else {
        format!(
            "{} · {}",
            text(&snapshot["currency"], 3).to_ascii_uppercase(),
            clock(&snapshot["observed_at"])
        )
    };
    let header = format!(
        "{}{}",
        pad(&title, content.saturating_sub(display_width(&right))),
        right
    );
    line(
        &mut out,
        &[(
            header,
            if stale {
                Some(terminal::Role::Heading)
            } else {
                None
            },
        )],
        style,
        width,
    );
    line(
        &mut out,
        &[(format!(" {}", "─".repeat(content)), None)],
        style,
        width,
    );
    for row in &rows {
        let name = text(&row["name"], if single { content } else { name_width });
        let changed = previous
            .and_then(|s| s["rows"].as_array())
            .and_then(|rs| rs.iter().find(|r| r["agent_id"] == row["agent_id"]))
            .is_some_and(|old| {
                [
                    "consumed",
                    "frozen",
                    "available",
                    "limit",
                    "consumed_cents",
                    "frozen_cents",
                    "available_cents",
                    "limit_cents",
                ]
                .iter()
                .any(|k| old[*k] != row[*k])
            });
        let highlight = if changed {
            Some(terminal::Role::Heading)
        } else {
            None
        };
        if single {
            line(&mut out, &[(format!(" {name}"), highlight)], style, width);
        }
        if !has_budget(row) {
            line(
                &mut out,
                &[(
                    format!(
                        " {}no current budget",
                        if single {
                            String::new()
                        } else {
                            format!("{} ", pad(&name, name_width))
                        }
                    ),
                    highlight,
                )],
                style,
                width,
            );
            continue;
        }
        let consumed = micro_units(row, "consumed_cents");
        let frozen = micro_units(row, "frozen_cents");
        let available = micro_units(row, "available_cents");
        let limit = micro_units(row, "limit_cents");
        let used = consumed + frozen;
        let pct = if limit > 0 {
            (used.max(0) * 100) / limit
        } else {
            0
        };
        let label = if single {
            " ".into()
        } else {
            format!(" {} ", pad(&name, name_width))
        };
        let (used_text, limit_text) = (&used_column[budgeted_index], &limit_column[budgeted_index]);
        budgeted_index += 1;
        // Multi-agent amounts and percentages align so rows read as columns.
        let detail = if single {
            format!(
                "  {} of {} used · {}%",
                used_text.trim(),
                limit_text.trim(),
                pct
            )
        } else {
            format!("  {used_text} of {limit_text}{:>6}", format!("{pct}%"))
        };
        let cells = bar_cells(
            consumed,
            frozen,
            available,
            limit,
            content
                .saturating_sub(1 + display_width(&label) + display_width(&detail))
                .max(10),
        );
        line(
            &mut out,
            &[
                (label, highlight),
                // Monochrome by design: segments differ by glyph texture only.
                ("█".repeat(cells[0]), None),
                ("▓".repeat(cells[1]), None),
                ("░".repeat(cells[2]), None),
                (detail, highlight),
            ],
            style,
            width,
        );
        if single {
            line(
                &mut out,
                &[(
                    format!(
                        " consumed {}   frozen {}   free {}",
                        shown(row, "consumed_cents"),
                        shown(row, "frozen_cents"),
                        shown(row, "available_cents")
                    ),
                    highlight,
                )],
                style,
                width,
            );
        }
    }
    if rows.is_empty() {
        line(
            &mut out,
            &[(" No registered agents for the active user.".into(), None)],
            style,
            width,
        );
    }
    if !opts.compact {
        if !single && !rows.is_empty() {
            line(
                &mut out,
                &[(
                    format!("{}█ consumed  ▓ frozen  ░ free", " ".repeat(name_width + 3)),
                    None,
                )],
                style,
                width,
            );
        }
        let events: Vec<_> = snapshot["recent_events"]
            .as_array()
            .expect("validated events")
            .iter()
            .filter(|e| !single || rows.first().is_some_and(|r| e["agent_id"] == r["agent_id"]))
            .take(opts.events)
            .collect();
        if !events.is_empty() && out.lines().count() + 3 < height {
            out.push('\n');
            line(
                &mut out,
                &[(" RECENT DECISIONS".into(), Some(terminal::Role::Heading))],
                style,
                width,
            );
            for event in events {
                let changed = previous.is_some_and(|s| {
                    s["recent_events"]
                        .as_array()
                        .and_then(|es| es.iter().find(|e| e["id"] == event["id"]))
                        .is_none_or(|old| old["status"] != event["status"])
                });
                let mut rendered = String::new();
                line(
                    &mut rendered,
                    &[(
                        event_line(event, single, name_width, width),
                        if changed {
                            Some(terminal::Role::Heading)
                        } else {
                            None
                        },
                    )],
                    style,
                    width,
                );
                if out.lines().count() + rendered.lines().count() >= height {
                    break;
                }
                out.push_str(&rendered);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn admission_void_reason_is_visible_in_the_watch_event() {
        let event = serde_json::json!({"created_at":"2026-10-13T12:00:00Z", "status":"released", "reserved_cents":500,
            "release_reason":"executor_rejected_before_claim", "rule_ids":["allow"]});
        let line = super::event_line(&event, true, 12, 100);
        assert!(line.contains("released"));
        assert!(line.contains("executor_rejected_before_claim"));
    }
    use super::*;
    fn snapshot() -> Value {
        json!({"schema_version":"hubu-watch-v1","currency":"usd","observed_at":"2026-10-08T03:12:07Z","rows":[{"agent_id":"a","name":"image-agent","budget_id":"b","limit_cents":200,"consumed_cents":6,"frozen_cents":8,"available_cents":186}],"recent_events":[{"id":"d","agent_id":"a","agent_name":"image-agent","created_at":"2026-10-08T03:12:05Z","provider":{"id":"provider:black-forest-labs:flux","display_name":"untrusted"},"status":"settled","requested_cents":8,"reserved_cents":8,"settled_cost":{"amount":"58","scale":3},"budget_charge_cents":6,"rule_ids":["draft_images"]}]})
    }
    fn output(s: &Value, o: &Options) -> String {
        render(s, None, o, terminal::TerminalStyle::plain(), false, 100, 70).unwrap()
    }
    #[test]
    fn single_multi_and_compact_layouts() {
        let mut s = snapshot();
        let o = options(vec![]).unwrap();
        let one = output(&s, &o);
        assert!(one.contains("$0.14 of $2.00 used · 7%"));
        assert!(one.contains("consumed $0.06   frozen $0.08   free $1.86"));
        assert!(one.contains("$0.08 → settled $0.058"));
        assert!(!one.contains("charge"));
        assert!(!one.contains("█ consumed"));
        let mut second = s["rows"][0].clone();
        second["agent_id"] = json!("b");
        second["name"] = json!("research-bot");
        s["rows"].as_array_mut().unwrap().push(second);
        let multi = output(&s, &o);
        assert!(multi.contains("HUBU WATCH · 2 agents"));
        assert!(multi.contains("█ consumed  ▓ frozen  ░ free"));
        assert!(multi.contains("$0.08 → $0.058"));
        assert!(!multi.contains("settled $0.058"));
        let compact = output(&s, &options(vec!["--compact".into()]).unwrap());
        assert!(!compact.contains("RECENT"));
        assert!(!compact.contains("█ consumed"));
        let selected = output(
            &s,
            &options(vec!["--agent".into(), "image-agent".into()]).unwrap(),
        );
        assert!(!selected.contains("research-bot"));
        assert!(!selected.contains("2 agents"));
        s["rows"][1]["name"] = json!("image-agent");
        assert!(render(
            &s,
            None,
            &options(vec!["--agent".into(), "image-agent".into()]).unwrap(),
            terminal::TerminalStyle::plain(),
            false,
            24,
            70
        )
        .is_err());
    }
    #[test]
    fn flags_validate_range_and_display_only_filter() {
        assert_eq!(options(vec![]).unwrap().events, 5);
        for n in ["0", "11", "bad"] {
            assert!(options(vec!["--events".into(), n.into()]).is_err());
        }
        for n in ["1", "10"] {
            assert!(
                options(vec![
                    "--events".into(),
                    n.into(),
                    "--once".into(),
                    "--currency".into(),
                    "USD".into()
                ])
                .unwrap()
                .once
            );
        }
    }
    #[test]
    fn all_status_glyphs_provider_trust_and_rule_fallbacks() {
        let mut e = snapshot()["recent_events"][0].clone();
        for (status, glyph, word) in [
            ("authorized", "◐", "reserved"),
            ("claimed", "◐", "reserved"),
            ("settled", "✓", "settled"),
            ("needs_approval", "‖", "awaiting approval"),
            ("denied", "✗", "blocked"),
            ("released", "○", "released"),
            ("expired", "○", "released"),
            ("reconciliation_required", "!", "needs reconciliation"),
        ] {
            e["status"] = json!(status);
            let line = event_line(&e, true, 12, 70);
            assert!(line.contains(glyph) && line.contains(word), "{line}");
        }
        e["denial_kind"] = json!("budget");
        assert!(event_line(&e, true, 12, 70).ends_with("budget"));
        e["denial_kind"] = Value::Null;
        e["rule_ids"] = json!([]);
        assert!(event_line(&e, true, 12, 70).ends_with("default"));
        e["provider"]["id"] = json!("unknown");
        e["provider"]["display_name"] = json!("longprovider");
        assert!(event_line(&e, true, 12, 70).contains("longp…"));
    }
    #[test]
    fn bar_visibility_width_overrun_and_no_budget() {
        for width in [23, 31] {
            for (c, f, a, l) in [
                (6, 8, 186, 200),
                (0, 8, 192, 200),
                (1, 1, 99998, 100000),
                (240, 0, 0, 200),
                (200, 2, 0, 200),
            ] {
                let cells = bar_cells(c, f, a, l, width);
                assert_eq!(cells.iter().sum::<usize>(), width);
                assert_eq!(cells[0] > 0, c > 0);
                assert_eq!(cells[1] > 0, f > 0);
                if c + f < l {
                    assert_eq!(cells[2] > 0, a > 0);
                } else {
                    assert_eq!(cells[2], 0);
                }
            }
        }
        let mut s = snapshot();
        s["rows"][0]["consumed_cents"] = json!(240);
        s["rows"][0]["frozen_cents"] = json!(0);
        s["rows"][0]["available_cents"] = json!(0);
        assert!(output(&s, &options(vec![]).unwrap()).contains("$2.40 of $2.00 used · 120%"));
        s["rows"][0]["budget_id"] = Value::Null;
        assert!(output(&s, &options(vec![]).unwrap()).contains("no current budget"));
    }
    #[test]
    fn exact_sub_cent_rows_align_on_the_decimal_point() {
        let money = |units: &str, display: &str| json!({"units": units, "scale": 6, "currency": "usd", "display": display});
        let row = |id: &str, consumed: (&str, &str), limit: (&str, &str)| {
            json!({"agent_id": id, "name": id, "budget_id": format!("bgt-{id}"),
                "consumed": money(consumed.0, consumed.1), "frozen": money("0", "0.00"),
                "available": money("1", "0.000001"), "limit": money(limit.0, limit.1),
                "consumed_cents": 1, "frozen_cents": 0, "available_cents": 0, "limit_cents": 1})
        };
        let mut s = snapshot();
        s["rows"] = json!([
            row("x402-agent", ("3100", "0.0031"), ("10000", "0.01")),
            row(
                "image-agent",
                ("12500000", "12.50"),
                ("100000000", "100.00")
            ),
        ]);
        let out = output(&s, &options(vec!["--compact".into()]).unwrap());
        let details: Vec<&str> = out
            .lines()
            .filter_map(|line| line.split_once("  ").map(|(_, rest)| rest))
            .filter(|rest| rest.contains(" of "))
            .collect();
        assert!(out.contains(" $0.0031 of   $0.01 "), "{out}");
        assert!(out.contains("$12.50   of $100.00 "), "{out}");
        assert_eq!(details.len(), 2, "{out}");
        assert!(out.lines().all(|l| display_width(l) <= 70));
    }
    #[test]
    fn width_sanitization_exact_cost_and_stale_header() {
        let mut s = snapshot();
        s["rows"][0]["name"] = json!("evil\u{1b}[2J\nvery-long-name");
        s["rows"][0]["consumed_cents"] = json!(i64::MAX);
        s["rows"][0]["frozen_cents"] = json!(i64::MAX);
        s["rows"][0]["limit_cents"] = json!(1);
        s["recent_events"][0]["settled_cost"] = json!({"amount":"9223372036854775807","scale":18});
        s["recent_events"][0]["rule_ids"] = json!(["evil\u{1b}[2J\nlongrule"]);
        let o = options(vec![]).unwrap();
        let result = output(&s, &o);
        assert!(!result.contains('\u{1b}'));
        assert!(result.lines().all(|l| display_width(l) <= 70));
        assert!(result.contains("$9.223372036854775807"));
        assert!(result.contains("evil?[2J?"));
        assert_eq!(
            exact_cost(&json!({"amount":"1","scale":3}), &json!("usd")),
            "$0.001"
        );
        assert_eq!(
            exact_cost(&json!({"amount":"1234","scale":4}), &json!("usd")),
            "$0.1234"
        );
        assert_eq!(
            exact_cost(&json!({"amount":"5000","scale":6}), &json!("usdc")),
            "0.005 USDC"
        );
        let stale = render(
            &s,
            None,
            &o,
            terminal::TerminalStyle::plain(),
            true,
            100,
            70,
        )
        .unwrap();
        assert!(stale.lines().next().unwrap().contains("STALE · retrying"));
        assert!(!stale.lines().next().unwrap().contains("USD"));
    }
    #[test]
    fn monochrome_bold_highlights_only_amount_or_status_changes() {
        let s = snapshot();
        let o = options(vec![]).unwrap();
        let colored = terminal::TerminalStyle::colored();
        let same = render(&s, Some(&s), &o, colored, false, 100, 70).unwrap();
        let mut changed = s.clone();
        changed["rows"][0]["frozen_cents"] = json!(9);
        changed["recent_events"][0]["status"] = json!("claimed");
        let highlighted = render(&changed, Some(&s), &o, colored, false, 100, 70).unwrap();
        let stale = render(&s, None, &o, colored, true, 100, 70).unwrap();
        for output in [&same, &highlighted, &stale] {
            // No foreground colors (30-37, 90-97) anywhere; emphasis is bold only.
            assert!(
                !output.contains("\u{1b}[3") && !output.contains("\u{1b}[9"),
                "{output:?}"
            );
        }
        assert!(highlighted.matches("\u{1b}[1m").count() > same.matches("\u{1b}[1m").count());
        assert!(!highlighted.contains('*'));
        assert!(
            render(&s, None, &o, terminal::TerminalStyle::plain(), false, 5, 70)
                .unwrap()
                .lines()
                .count()
                <= 5
        );
    }

    #[test]
    fn wide_terminals_show_full_names_and_rules_within_width() {
        let mut s = snapshot();
        let long = "research-agent-campaigns";
        let mut second = s["rows"][0].clone();
        second["agent_id"] = json!("b");
        second["name"] = json!(long);
        s["rows"].as_array_mut().unwrap().push(second);
        s["recent_events"][0]["agent_id"] = json!("b");
        s["recent_events"][0]["agent_name"] = json!(long);
        s["recent_events"][0]["rule_ids"] = json!(["allow_under_40_cents_drafts"]);
        let o = options(vec![]).unwrap();
        let narrow = render(
            &s,
            None,
            &o,
            terminal::TerminalStyle::plain(),
            false,
            100,
            70,
        )
        .unwrap();
        assert!(narrow.contains("research-ag…"));
        assert!(narrow.lines().all(|l| display_width(l) <= 70), "{narrow}");
        let wide = render(
            &s,
            None,
            &o,
            terminal::TerminalStyle::plain(),
            false,
            100,
            layout_width(120),
        )
        .unwrap();
        assert!(
            wide.contains(long) && !wide.contains("research-ag…"),
            "{wide}"
        );
        assert!(wide.contains("allow_under_40_cents_drafts"), "{wide}");
        assert!(wide.lines().all(|l| display_width(l) <= 100), "{wide}");
        // Bars widen with the terminal: the wide row is longer than the narrow one.
        let bar = |out: &str| {
            out.lines()
                .find(|l| l.contains(long) || l.contains("research-ag…"))
                .unwrap()
                .chars()
                .filter(|c| "█▓░".contains(*c))
                .count()
        };
        assert!(bar(&wide) > bar(&narrow));
        assert_eq!(layout_width(40), 70);
        assert_eq!(layout_width(500), 100);
    }

    #[test]
    fn wide_glyphs_and_single_view_stay_within_width() {
        let wide_name = "設計エージェント研究用設計エージェント研究用"; // 22 wide glyphs = 44 cells
        let mut s = snapshot();
        let mut second = s["rows"][0].clone();
        second["agent_id"] = json!("b");
        second["name"] = json!(wide_name);
        s["rows"].as_array_mut().unwrap().push(second);
        s["recent_events"][0]["agent_id"] = json!("b");
        s["recent_events"][0]["agent_name"] = json!(wide_name);
        s["recent_events"][0]["rule_ids"] = json!(["上限四十セント以下の下書き画像"]);
        let o = options(vec![]).unwrap();
        for width in [70, 85, 100] {
            let out = render(
                &s,
                None,
                &o,
                terminal::TerminalStyle::plain(),
                false,
                100,
                width,
            )
            .unwrap();
            assert!(
                out.lines().all(|l| display_width(l) <= width),
                "{width}: {out}"
            );
            assert!(out.contains('…'), "{out}");
        }
        // The single-agent view puts the name on its own line, so even at 70
        // columns a long name is shown in full while staying within the width.
        let long = "research-agent-for-quarterly-campaigns";
        let mut one = snapshot();
        one["rows"][0]["name"] = json!(long);
        one["recent_events"][0]["rule_ids"] = json!(["allow_under_40c_drafts"]);
        let out = render(
            &one,
            None,
            &o,
            terminal::TerminalStyle::plain(),
            false,
            100,
            70,
        )
        .unwrap();
        assert!(out.contains(long), "{out}");
        assert!(out.lines().all(|l| display_width(l) <= 70), "{out}");
    }

    #[test]
    fn emoji_sequences_measure_as_one_wide_unit() {
        assert_eq!(display_width("❤️"), 2);
        assert_eq!(display_width("1️⃣"), 2);
        assert_eq!(display_width("a❤️b"), 4);
        assert_eq!(display_width("🇺🇸🇯🇵"), 4);
        assert_eq!(
            text(&json!("🇺🇸".repeat(8)), 12),
            format!("{}…", "🇺🇸".repeat(5))
        );
        assert_eq!(display_width("👩‍💻"), 2);
        assert_eq!(text(&json!("❤️❤️❤️❤️"), 5), "❤️❤️…");
        let name = "❤️".repeat(20);
        let mut s = snapshot();
        let mut second = s["rows"][0].clone();
        second["agent_id"] = json!("b");
        second["name"] = json!(name);
        s["rows"].as_array_mut().unwrap().push(second);
        s["recent_events"][0]["agent_name"] = json!(name);
        s["recent_events"][0]["rule_ids"] = json!(["1️⃣".repeat(30)]);
        let o = options(vec![]).unwrap();
        for width in [70, 100] {
            let out = render(
                &s,
                None,
                &o,
                terminal::TerminalStyle::plain(),
                false,
                100,
                width,
            )
            .unwrap();
            assert!(
                out.lines().all(|l| display_width(l) <= width),
                "{width}: {out}"
            );
            // Truncation never splits a sequence from its variation selector.
            assert!(!out.contains("\u{2764}…"), "{out}");
        }
    }
}
