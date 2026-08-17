//! Central redaction for logs, API errors, and persistence-bound diagnostics.
use std::error::Error;

const REDACTED: &str = "[REDACTED]";
const SENSITIVE_QUERY_KEYS: &[&str] = &[
    "access_token",
    "api_key",
    "apikey",
    "key",
    "secret",
    "client_secret",
    "sig",
    "signature",
    "token",
];

pub struct Redactor {
    exact: Vec<String>,
    marker: &'static str,
}
impl Redactor {
    pub fn new<'a>(secrets: impl IntoIterator<Item = &'a [u8]>) -> Self {
        let mut exact: Vec<_> = secrets
            .into_iter()
            .filter_map(|v| std::str::from_utf8(v).ok())
            .filter(|v| !v.is_empty())
            .flat_map(|value| {
                let json = serde_json::to_string(value).expect("string serialization cannot fail");
                let debug = format!("{value:?}");
                let percent = percent_encode(value.as_bytes());
                [
                    value.to_owned(),
                    json[1..json.len() - 1].to_owned(),
                    debug[1..debug.len() - 1].to_owned(),
                    percent.clone(),
                    percent.to_ascii_lowercase(),
                ]
            })
            .collect();
        exact.sort_by_key(|value| std::cmp::Reverse(value.len()));
        exact.dedup();
        let marker = if exact.iter().any(|secret| REDACTED.contains(secret)) {
            ""
        } else {
            REDACTED
        };
        Self { exact, marker }
    }

    pub fn redact(&self, input: &str) -> String {
        let mut value = input.to_owned();
        loop {
            let before = value.clone();
            for secret in &self.exact {
                value = value.replace(secret, self.marker);
            }
            if value == before {
                break;
            }
        }
        value = redact_authorization(&value, self.marker);
        redact_query(&value, self.marker)
    }

    pub fn contains_registered_secret(&self, input: &str) -> bool {
        self.exact.iter().any(|secret| input.contains(secret))
    }

    pub fn json_contains_registered_secret(&self, value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::String(value) => self.contains_registered_secret(value),
            serde_json::Value::Array(values) => values
                .iter()
                .any(|value| self.json_contains_registered_secret(value)),
            serde_json::Value::Object(values) => values.iter().any(|(key, value)| {
                self.contains_registered_secret(key) || self.json_contains_registered_secret(value)
            }),
            _ => false,
        }
    }

    pub fn error_chain(&self, error: &(dyn Error + 'static)) -> String {
        let mut parts = Vec::new();
        let mut current = Some(error);
        while let Some(error) = current {
            parts.push(self.redact(&error.to_string()));
            current = error.source();
        }
        parts.join(": ")
    }
}

fn percent_encode(value: &[u8]) -> String {
    value
        .iter()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (*byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

impl Default for Redactor {
    fn default() -> Self {
        Self::new(std::iter::empty::<&'static [u8]>())
    }
}

fn redact_authorization(input: &str, marker: &str) -> String {
    input
        .lines()
        .map(|line| {
            let lower = line.to_ascii_lowercase();
            if let Some(index) = lower.find("authorization:") {
                format!("{}authorization: {marker}", &line[..index])
            } else if let Some(index) = lower.find("bearer ") {
                format!("{}Bearer {marker}", &line[..index])
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn redact_query(input: &str, marker: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(index) = rest.find(['?', '&']) {
        let (before, after) = rest.split_at(index + 1);
        output.push_str(before);
        rest = after;
        let end = rest.find(['&', ' ', '\n', '\r']).unwrap_or(rest.len());
        let item = &rest[..end];
        if let Some((key, _)) = item.split_once('=') {
            if SENSITIVE_QUERY_KEYS
                .iter()
                .any(|candidate| key.eq_ignore_ascii_case(candidate))
            {
                output.push_str(key);
                output.push('=');
                output.push_str(marker);
                rest = &rest[end..];
            }
        }
    }
    output.push_str(rest);
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{error::Error, fmt};
    const CANARY: &str = "gongbu-canary-vendor-secret-9f83";
    #[derive(Debug)]
    struct Inner;
    impl fmt::Display for Inner {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "SDK api_key={CANARY}")
        }
    }
    impl Error for Inner {}
    #[derive(Debug)]
    struct Outer(Inner);
    impl fmt::Display for Outer {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                f,
                "request https://x.test/run?token={CANARY}&ok=1 Authorization: Bearer {CANARY}"
            )
        }
    }
    impl Error for Outer {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(&self.0)
        }
    }
    #[test]
    fn redacts_exact_headers_query_and_nested_sdk_errors() {
        let value = Redactor::new([CANARY.as_bytes()]).error_chain(&Outer(Inner));
        assert!(!value.contains(CANARY));
        assert!(value.matches(REDACTED).count() >= 3);
        assert!(value.contains("ok=1"));
    }

    #[test]
    fn overlapping_secrets_are_fully_redacted_regardless_of_registration_order() {
        let value = Redactor::new([b"abc".as_slice(), b"abcdef".as_slice()]).redact("abcdef abc");
        assert_eq!(value, "[REDACTED] [REDACTED]");
    }

    #[test]
    fn escaped_secret_renderings_are_redacted() {
        let secret = "canary-\"slash\\newline\nsecret";
        let rendered = format!(
            "SDK api_key={}",
            &serde_json::to_string(secret).unwrap()
                [1..serde_json::to_string(secret).unwrap().len() - 1]
        );
        let value = Redactor::new([secret.as_bytes()]).redact(&rendered);
        assert!(!value.contains("canary"));
        assert!(value.contains(REDACTED));
    }

    #[test]
    fn rust_debug_control_character_renderings_are_redacted() {
        let secret = "canary-\0-\u{1b}-secret";
        let rendered = format!("SDK error: {secret:?}");
        let value = Redactor::new([secret.as_bytes()]).redact(&rendered);
        assert!(!value.contains("canary"));
        assert!(value.contains(REDACTED));
    }

    #[test]
    fn redaction_marker_cannot_reintroduce_a_registered_secret() {
        for secret in ["[REDACTED]", "REDACTED", "["] {
            let value = Redactor::new([secret.as_bytes()]).redact(secret);
            assert!(!value.contains(secret));
        }
    }

    #[test]
    fn sequential_replacements_cannot_synthesize_a_registered_secret() {
        let value = Redactor::new([b"a[REDACTED]b".as_slice(), b"x".as_slice()]).redact("axb");
        assert!(!value.contains("a[REDACTED]b"));
    }

    #[test]
    fn percent_encoded_secret_renderings_are_redacted() {
        let value =
            Redactor::new([b"abc/def?x".as_slice()]).redact("SDK client_secret=abc%2fdef%3fx");
        assert!(!value.contains("abc%2fdef%3fx"));
    }
}
