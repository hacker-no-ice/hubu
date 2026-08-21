//! Serialized stdio transport and initialized-lifecycle capability monitoring.

use std::{
    io::{self, BufRead, Write},
    sync::mpsc,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::Value;

use crate::{notification::tools_list_changed_notification, Server};

enum RunEvent {
    Input(String),
    InputError(io::Error),
    RefreshComplete,
    Eof,
}

struct CapabilityMonitor {
    stop: mpsc::Sender<()>,
    handle: Option<thread::JoinHandle<()>>,
}

const MAX_FAILURE_BACKOFF: Duration = Duration::from_secs(5 * 60);

struct PollSchedule {
    base: Duration,
    failure_streak: u32,
    jitter_state: u64,
}

impl PollSchedule {
    fn new(base: Duration, jitter_seed: u64) -> Self {
        Self {
            base,
            failure_streak: 0,
            jitter_state: jitter_seed.max(1),
        }
    }

    fn next_delay(&mut self, failed: bool) -> Duration {
        let multiplier = if failed {
            let multiplier = 1_u32 << self.failure_streak.min(4);
            self.failure_streak = self.failure_streak.saturating_add(1);
            multiplier
        } else {
            self.failure_streak = 0;
            1
        };
        let backed_off = self
            .base
            .saturating_mul(multiplier)
            .min(MAX_FAILURE_BACKOFF);
        let jitter_percent = 80 + self.next_jitter_value() % 41;
        let millis = backed_off
            .as_millis()
            .saturating_mul(u128::from(jitter_percent))
            / 100;
        Duration::from_millis(
            u64::try_from(millis.max(1))
                .unwrap_or(u64::MAX)
                .min(MAX_FAILURE_BACKOFF.as_millis() as u64),
        )
    }

    fn next_jitter_value(&mut self) -> u64 {
        self.jitter_state ^= self.jitter_state << 13;
        self.jitter_state ^= self.jitter_state >> 7;
        self.jitter_state ^= self.jitter_state << 17;
        self.jitter_state
    }
}

impl Drop for CapabilityMonitor {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

pub(super) fn run(
    server: Server,
    input: impl BufRead + Send,
    mut output: impl Write,
) -> io::Result<()> {
    thread::scope(|scope| {
        let (event_tx, event_rx) = mpsc::channel();
        let input_tx = event_tx.clone();
        scope.spawn(move || {
            for line in input.lines() {
                match line {
                    Ok(line) => {
                        if input_tx.send(RunEvent::Input(line)).is_err() {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = input_tx.send(RunEvent::InputError(error));
                        return;
                    }
                }
            }
            let _ = input_tx.send(RunEvent::Eof);
        });

        let mut initialize_response_seen = false;
        let mut initialized = false;
        let mut monitor = None;
        let result = loop {
            let event = match event_rx.recv() {
                Ok(event) => event,
                Err(_) => break Ok(()),
            };
            match event {
                RunEvent::Input(line) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let lifecycle_ready = is_initialized_notification(&line);
                    let initialize_request = is_initialize_request(&line);
                    if let Some(response) = server.handle_line(&line) {
                        if let Err(error) = write_message(&mut output, &response) {
                            break Err(error);
                        }
                        if initialize_request && response.get("result").is_some() {
                            initialize_response_seen = true;
                        }
                    }
                    if lifecycle_ready && initialize_response_seen && !initialized {
                        // Establish the post-handshake baseline without emitting
                        // a transition for changes that happened during setup.
                        server.refresh_capabilities();
                        server.reset_catalog_tracking();
                        initialized = true;
                        monitor = Some(spawn_capability_monitor(&server, event_tx.clone()));
                    }
                    if initialized {
                        if let Err(error) = write_pending_notifications(&server, &mut output) {
                            break Err(error);
                        }
                    }
                }
                RunEvent::RefreshComplete => {
                    if initialized {
                        if let Err(error) = write_pending_notifications(&server, &mut output) {
                            break Err(error);
                        }
                    }
                }
                RunEvent::InputError(error) => break Err(error),
                RunEvent::Eof => break Ok(()),
            }
        };
        drop(monitor);
        result
    })
}

fn spawn_capability_monitor(
    server: &Server,
    event_tx: mpsc::Sender<RunEvent>,
) -> CapabilityMonitor {
    let server = server.clone();
    let poll_interval = server.capability_poll_interval();
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
        ^ u64::from(std::process::id());
    let (stop_tx, stop_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let mut schedule = PollSchedule::new(poll_interval, seed);
        loop {
            let delay = schedule.next_delay(server.capability_probe_failed());
            match stop_rx.recv_timeout(delay) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    server.refresh_capabilities();
                    if event_tx.send(RunEvent::RefreshComplete).is_err() {
                        return;
                    }
                }
            }
        }
    });
    CapabilityMonitor {
        stop: stop_tx,
        handle: Some(handle),
    }
}

fn write_pending_notifications(server: &Server, output: &mut impl Write) -> io::Result<()> {
    for _ in 0..server.take_pending_catalog_transitions() {
        write_message(output, &tools_list_changed_notification())?;
    }
    Ok(())
}

fn is_initialized_notification(line: &str) -> bool {
    let Ok(request) = serde_json::from_str::<Value>(line) else {
        return false;
    };
    request.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
        && request.get("id").is_none()
        && request.get("method").and_then(Value::as_str) == Some("notifications/initialized")
}

fn is_initialize_request(line: &str) -> bool {
    let Ok(request) = serde_json::from_str::<Value>(line) else {
        return false;
    };
    request.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
        && request.get("id").is_some()
        && request.get("method").and_then(Value::as_str) == Some("initialize")
}

fn write_message(output: &mut impl Write, message: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *output, message)?;
    output.write_all(b"\n")?;
    output.flush()
}

#[cfg(test)]
mod schedule_tests {
    use super::*;

    #[test]
    fn healthy_polls_are_jittered_around_the_base_interval() {
        let mut schedule = PollSchedule::new(Duration::from_secs(30), 7);
        for _ in 0..20 {
            let delay = schedule.next_delay(false);
            assert!((Duration::from_secs(24)..=Duration::from_secs(36)).contains(&delay));
        }
    }

    #[test]
    fn failures_back_off_and_success_resets_the_schedule() {
        let mut schedule = PollSchedule::new(Duration::from_secs(30), 11);
        let first = schedule.next_delay(true);
        let second = schedule.next_delay(true);
        let third = schedule.next_delay(true);
        assert!(second > first);
        assert!(third > second);

        let recovered = schedule.next_delay(false);
        assert!((Duration::from_secs(24)..=Duration::from_secs(36)).contains(&recovered));
    }
}
