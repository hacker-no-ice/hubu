//! Serialized stdio transport and initialized-lifecycle capability monitoring.

use std::{
    io::{self, BufRead, Write},
    sync::mpsc,
    thread,
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
    let (stop_tx, stop_rx) = mpsc::channel();
    let handle = thread::spawn(move || loop {
        match stop_rx.recv_timeout(poll_interval) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                server.refresh_capabilities();
                if event_tx.send(RunEvent::RefreshComplete).is_err() {
                    return;
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
