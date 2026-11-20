//! macOS LocalAuthentication presence provider (spec section 5.2).
//!
//! HUB-344 established that a locally built, ad hoc signed `hubu-server`
//! started in the owner's GUI session can raise this dialog and read the
//! result. macOS renders the text as `"hubu-server" is trying to <reason>.`
use std::sync::{mpsc, Mutex};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_foundation::{NSError, NSString};
use objc2_local_authentication::{LAContext, LAPolicy};

use super::{PresenceMethod, PresenceOutcome, PresenceProvider, PresenceRequest};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LocalAuthPolicy {
    /// Touch ID, or the login password when Touch ID is unavailable, locked
    /// out, or the lid is closed. The default.
    DeviceOwner,
    /// Touch ID only, no password fallback
    /// (`HUBU_OWNER_PRESENCE_POLICY=biometrics`).
    Biometrics,
}

pub(crate) struct LocalAuthProvider {
    policy: LocalAuthPolicy,
}

impl LocalAuthProvider {
    pub(crate) fn new(policy: LocalAuthPolicy) -> Self {
        Self { policy }
    }

    fn la_policy(&self) -> LAPolicy {
        match self.policy {
            LocalAuthPolicy::DeviceOwner => LAPolicy::DeviceOwnerAuthentication,
            LocalAuthPolicy::Biometrics => LAPolicy::DeviceOwnerAuthenticationWithBiometrics,
        }
    }
}

impl PresenceProvider for LocalAuthProvider {
    fn confirm(&self, request: &PresenceRequest) -> PresenceOutcome {
        let context: Retained<LAContext> = unsafe { LAContext::new() };
        if let Err(error) = unsafe { context.canEvaluatePolicy_error(self.la_policy()) } {
            return PresenceOutcome::Unavailable(format!(
                "LocalAuthentication cannot evaluate the owner policy here (code {})",
                error.code()
            ));
        }
        // LocalAuthentication does not report which factor the owner used.
        // Touch ID is offered first whenever it can be evaluated, so report
        // biometric when it was available and the device password otherwise.
        let method = if self.policy == LocalAuthPolicy::Biometrics
            || unsafe {
                context.canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthenticationWithBiometrics)
            }
            .is_ok()
        {
            PresenceMethod::Biometric
        } else {
            PresenceMethod::DevicePassword
        };
        unsafe {
            context.setLocalizedCancelTitle(Some(&NSString::from_str("Deny")));
        }

        let (sender, receiver) = mpsc::channel::<bool>();
        let sender = Mutex::new(Some(sender));
        // The reply runs once, on a private framework queue.
        let reply = RcBlock::new(move |success: Bool, error: *mut NSError| {
            let confirmed = success.as_bool() && error.is_null();
            if let Some(sender) = sender.lock().ok().and_then(|mut guard| guard.take()) {
                let _ = sender.send(confirmed);
            }
        });
        unsafe {
            context.evaluatePolicy_localizedReason_reply(
                self.la_policy(),
                &NSString::from_str(&request.reason),
                &reply,
            );
        }
        let outcome = match receiver.recv_timeout(request.timeout) {
            Ok(true) => PresenceOutcome::Confirmed { method },
            // Every LocalAuthentication error, including UserCancel,
            // AppCancel and SystemCancel, is a deny.
            Ok(false) | Err(mpsc::RecvTimeoutError::Disconnected) => PresenceOutcome::Denied,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Closes the dialog; the late reply is an AppCancel and is
                // dropped with the channel.
                unsafe { context.invalidate() };
                PresenceOutcome::TimedOut
            }
        };
        drop(context);
        outcome
    }

    fn name(&self) -> &'static str {
        match self.policy {
            LocalAuthPolicy::DeviceOwner => "local_authentication",
            LocalAuthPolicy::Biometrics => "local_authentication_biometrics",
        }
    }
}
