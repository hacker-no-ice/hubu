//! THROWAWAY PROTOTYPE for HUB-344. Do not merge, do not reuse as is.
//!
//! Question: can a locally built, ad hoc signed Rust binary raise the macOS
//! Touch ID dialog with custom text and get a trustworthy yes or no back?
//!
//! Subcommands:
//!   probe                       Non-interactive. Preflight both policies with
//!                               canEvaluatePolicy and report biometry type.
//!   noninteractive [--policy P] Non-interactive. Calls evaluatePolicy with
//!                               interactionNotAllowed = true, so the system
//!                               must fail with LAErrorNotInteractive (-1004)
//!                               instead of showing UI. Exercises the whole
//!                               call path without prompting anyone.
//!   ask --reason TEXT [...]     INTERACTIVE. Shows the real dialog and waits.
//!                               Owner only; refuses to run unless
//!                               HUB344_INTERACTIVE=1 is set.
//!
//! `ask` options:
//!   --policy biometrics|owner   biometrics = Touch ID only, no password
//!                               fallback (LAPolicyDeviceOwnerAuthentication-
//!                               WithBiometrics). owner = Touch ID or the login
//!                               password (LAPolicyDeviceOwnerAuthentication).
//!                               Default: biometrics.
//!   --cancel-title TEXT         Custom Cancel button title.
//!   --fallback-title TEXT       Custom fallback button title ("" hides it).
//!   --invalidate-after SECS     Call LAContext.invalidate() from this process
//!                               after SECS, to see the prompt dismissed and the
//!                               result come back as AppCancel (-9).
//!   --timeout SECS              Give up waiting after SECS (default 120).
//!
//! Every subcommand prints exactly one JSON line on stdout. Exit code 0 means
//! the owner approved (ask) or the probe ran; 1 means denied or failed; 2 means
//! usage error.

use std::process::ExitCode;
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_foundation::{NSError, NSString};
use objc2_local_authentication::{LABiometryType, LAContext, LAPolicy};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else {
        eprintln!(
            "usage: hub-344-touchid probe | noninteractive [--policy P] | ask --reason TEXT [...]"
        );
        return ExitCode::from(2);
    };
    let opts = match Opts::parse(&args[1..]) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    match cmd {
        "probe" => probe(),
        "noninteractive" => evaluate(&opts, false),
        "ask" => {
            if std::env::var("HUB344_INTERACTIVE").as_deref() != Ok("1") {
                eprintln!(
                    "refusing: `ask` shows a real Touch ID dialog and waits for a person. \
                     Set HUB344_INTERACTIVE=1 to run it."
                );
                return ExitCode::from(2);
            }
            evaluate(&opts, true)
        }
        other => {
            eprintln!("unknown subcommand: {other}");
            ExitCode::from(2)
        }
    }
}

#[derive(Default)]
struct Opts {
    policy: Option<String>,
    reason: Option<String>,
    cancel_title: Option<String>,
    fallback_title: Option<String>,
    invalidate_after: Option<u64>,
    timeout: Option<u64>,
}

impl Opts {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut o = Opts::default();
        let mut it = args.iter();
        while let Some(flag) = it.next() {
            let mut val = || it.next().cloned().ok_or(format!("{flag} needs a value"));
            match flag.as_str() {
                "--policy" => o.policy = Some(val()?),
                "--reason" => o.reason = Some(val()?),
                "--cancel-title" => o.cancel_title = Some(val()?),
                "--fallback-title" => o.fallback_title = Some(val()?),
                "--invalidate-after" => {
                    o.invalidate_after = Some(val()?.parse().map_err(|e| format!("{e}"))?)
                }
                "--timeout" => o.timeout = Some(val()?.parse().map_err(|e| format!("{e}"))?),
                other => return Err(format!("unknown option {other}")),
            }
        }
        Ok(o)
    }

    fn policy(&self) -> Result<(LAPolicy, &'static str), String> {
        match self.policy.as_deref().unwrap_or("biometrics") {
            "biometrics" => Ok((
                LAPolicy::DeviceOwnerAuthenticationWithBiometrics,
                "biometrics",
            )),
            "owner" => Ok((LAPolicy::DeviceOwnerAuthentication, "owner")),
            other => Err(format!("unknown policy {other}")),
        }
    }
}

fn probe() -> ExitCode {
    let ctx = unsafe { LAContext::new() };
    let mut parts = vec![format!("\"cmd\":\"probe\"")];
    for (name, policy) in [
        (
            "biometrics",
            LAPolicy::DeviceOwnerAuthenticationWithBiometrics,
        ),
        ("owner", LAPolicy::DeviceOwnerAuthentication),
    ] {
        let res = unsafe { ctx.canEvaluatePolicy_error(policy) };
        parts.push(format!(
            "\"can_evaluate_{name}\":{}",
            result_json(res.err())
        ));
    }
    // biometryType is only meaningful after canEvaluatePolicy has been called.
    let bt = unsafe { ctx.biometryType() };
    parts.push(format!("\"biometry_type\":\"{}\"", biometry_name(bt)));
    parts.push(format!("\"env\":{}", env_json()));
    println!("{{{}}}", parts.join(","));
    ExitCode::SUCCESS
}

fn evaluate(opts: &Opts, interactive: bool) -> ExitCode {
    let (policy, policy_name) = match opts.policy() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    let reason = match (&opts.reason, interactive) {
        (Some(r), _) if !r.is_empty() => r.clone(),
        (_, false) => "run the HUB-344 non-interactive probe".to_string(),
        _ => {
            eprintln!("error: ask needs a non-empty --reason");
            return ExitCode::from(2);
        }
    };

    let ctx: Retained<LAContext> = unsafe { LAContext::new() };
    unsafe {
        ctx.setInteractionNotAllowed(!interactive);
        if let Some(t) = &opts.cancel_title {
            ctx.setLocalizedCancelTitle(Some(&NSString::from_str(t)));
        }
        if let Some(t) = &opts.fallback_title {
            ctx.setLocalizedFallbackTitle(Some(&NSString::from_str(t)));
        }
    }

    let (tx, rx) = mpsc::channel::<(bool, Option<(isize, String, String)>)>();
    let tx = Mutex::new(Some(tx));
    // The reply runs once, on a private framework queue.
    let reply = RcBlock::new(move |success: Bool, err: *mut NSError| {
        let err = unsafe { err.as_ref() }.map(|e| {
            (
                e.code(),
                e.domain().to_string(),
                e.localizedDescription().to_string(),
            )
        });
        if let Some(tx) = tx.lock().ok().and_then(|mut g| g.take()) {
            let _ = tx.send((success.as_bool(), err));
        }
    });

    let started = Instant::now();
    unsafe {
        ctx.evaluatePolicy_localizedReason_reply(policy, &NSString::from_str(&reason), &reply);
    }

    let timeout = Duration::from_secs(opts.timeout.unwrap_or(120));
    let outcome = match opts.invalidate_after {
        Some(secs) => match rx.recv_timeout(Duration::from_secs(secs)) {
            Ok(v) => Some(v),
            Err(_) => {
                unsafe { ctx.invalidate() };
                rx.recv_timeout(timeout).ok()
            }
        },
        None => rx.recv_timeout(timeout).ok(),
    };
    let elapsed_ms = started.elapsed().as_millis();

    let (approved, err_json) = match &outcome {
        Some((ok, err)) => (*ok && err.is_none(), err_tuple_json(err.as_ref())),
        None => (false, "\"timed out waiting for reply\"".to_string()),
    };
    println!(
        "{{\"cmd\":\"{}\",\"policy\":\"{policy_name}\",\"reason\":{},\"approved\":{approved},\"error\":{err_json},\"elapsed_ms\":{elapsed_ms},\"env\":{}}}",
        if interactive { "ask" } else { "noninteractive" },
        json_str(&reason),
        env_json(),
    );
    // Keep the context alive until the reply has been handled.
    drop(ctx);
    if approved {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn result_json(err: Option<Retained<NSError>>) -> String {
    match err {
        None => "{\"ok\":true}".to_string(),
        Some(e) => format!(
            "{{\"ok\":false,\"error\":{}}}",
            err_tuple_json(Some(&(
                e.code(),
                e.domain().to_string(),
                e.localizedDescription().to_string()
            )))
        ),
    }
}

fn err_tuple_json(err: Option<&(isize, String, String)>) -> String {
    match err {
        None => "null".to_string(),
        Some((code, domain, desc)) => format!(
            "{{\"code\":{code},\"name\":\"{}\",\"domain\":{},\"description\":{}}}",
            la_error_name(*code),
            json_str(domain),
            json_str(desc)
        ),
    }
}

fn la_error_name(code: isize) -> &'static str {
    match code {
        -1 => "AuthenticationFailed",
        -2 => "UserCancel",
        -3 => "UserFallback",
        -4 => "SystemCancel",
        -5 => "PasscodeNotSet",
        -6 => "BiometryNotAvailable",
        -7 => "BiometryNotEnrolled",
        -8 => "BiometryLockout",
        -9 => "AppCancel",
        -10 => "InvalidContext",
        -11 => "CompanionNotAvailable",
        -12 => "BiometryNotPaired",
        -13 => "BiometryDisconnected",
        -14 => "InvalidDimensions",
        -1004 => "NotInteractive",
        _ => "Unknown",
    }
}

fn biometry_name(bt: LABiometryType) -> &'static str {
    match bt {
        LABiometryType::None => "none",
        LABiometryType::TouchID => "touch_id",
        LABiometryType::FaceID => "face_id",
        LABiometryType::OpticID => "optic_id",
        _ => "unknown",
    }
}

fn env_json() -> String {
    let ssh = std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some();
    let term = std::env::var("TERM_PROGRAM").unwrap_or_default();
    format!(
        "{{\"ssh\":{ssh},\"term_program\":{},\"pid\":{},\"ppid\":{}}}",
        json_str(&term),
        std::process::id(),
        parent_pid()
    )
}

fn parent_pid() -> i64 {
    extern "C" {
        fn getppid() -> i32;
    }
    unsafe { getppid() as i64 }
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
