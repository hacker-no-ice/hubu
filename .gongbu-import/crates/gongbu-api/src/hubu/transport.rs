use std::{
    io::{Read, Write},
    net::{Shutdown, TcpStream},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

const READ_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Error)]
pub enum HttpClientError {
    #[error("unsupported URL scheme for {url}")]
    UnsupportedScheme { url: String },
    #[error("invalid URL: {url}")]
    InvalidUrl { url: String },
    #[error("HTTP IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("server returned HTTP {status}: {body}")]
    Status { status: u16, body: String },
}

pub fn post_json<T, R>(url: &str, body: &T) -> Result<R, HttpClientError>
where
    T: Serialize,
    R: for<'de> Deserialize<'de>,
{
    post_json_authenticated(url, body, None)
}

pub fn post_json_authenticated<T, R>(
    url: &str,
    body: &T,
    bearer_token: Option<&[u8]>,
) -> Result<R, HttpClientError>
where
    T: Serialize,
    R: for<'de> Deserialize<'de>,
{
    let target = HttpTarget::parse(url)?;
    let body = serde_json::to_string(body)?;
    let authorization = authorization_header(bearer_token)?;
    let mut stream = TcpStream::connect((target.host.as_str(), target.port))?;
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    write!(
        stream,
        "POST {} HTTP/1.1\r\nHost: {}\r\n{}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        target.path,
        target.host_header(),
        authorization,
        body.len(),
        body
    )?;
    stream.flush()?;
    stream.shutdown(Shutdown::Write)?;

    let mut raw = String::new();
    stream.read_to_string(&mut raw)?;
    let (status, response_body) = parse_response(&raw)?;
    if !(200..300).contains(&status) {
        return Err(HttpClientError::Status {
            status,
            body: response_body.to_string(),
        });
    }
    Ok(serde_json::from_str(response_body)?)
}

pub fn get_json<R>(url: &str) -> Result<R, HttpClientError>
where
    R: for<'de> Deserialize<'de>,
{
    get_json_authenticated(url, None)
}

pub fn get_json_authenticated<R>(
    url: &str,
    bearer_token: Option<&[u8]>,
) -> Result<R, HttpClientError>
where
    R: for<'de> Deserialize<'de>,
{
    let target = HttpTarget::parse(url)?;
    let authorization = authorization_header(bearer_token)?;
    let mut stream = TcpStream::connect((target.host.as_str(), target.port))?;
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    write!(
        stream,
        "GET {} HTTP/1.1\r\nHost: {}\r\n{}Connection: close\r\n\r\n",
        target.path,
        target.host_header(),
        authorization,
    )?;
    stream.flush()?;
    stream.shutdown(Shutdown::Write)?;

    let mut raw = String::new();
    stream.read_to_string(&mut raw)?;
    let (status, response_body) = parse_response(&raw)?;
    if !(200..300).contains(&status) {
        return Err(HttpClientError::Status {
            status,
            body: response_body.to_string(),
        });
    }
    Ok(serde_json::from_str(response_body)?)
}

fn authorization_header(token: Option<&[u8]>) -> Result<String, HttpClientError> {
    let Some(token) = token else {
        return Ok(String::new());
    };
    let token = std::str::from_utf8(token).map_err(|_| HttpClientError::InvalidUrl {
        url: "invalid bearer token".into(),
    })?;
    if token.is_empty()
        || token
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
    {
        return Err(HttpClientError::InvalidUrl {
            url: "invalid bearer token".into(),
        });
    }
    Ok(format!("Authorization: Bearer {token}\r\n"))
}

fn split_head_and_body(raw: &str) -> (&str, &str) {
    raw.split_once("\r\n\r\n")
        .or_else(|| raw.split_once("\n\n"))
        .unwrap_or((raw, ""))
}

fn parse_response(raw: &str) -> Result<(u16, &str), HttpClientError> {
    let (head, body) = split_head_and_body(raw);
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .ok_or_else(|| HttpClientError::InvalidUrl {
            url: "malformed HTTP response".to_string(),
        })?;
    Ok((status, body))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HttpTarget {
    host: String,
    port: u16,
    path: String,
}

impl HttpTarget {
    fn parse(url: &str) -> Result<Self, HttpClientError> {
        let rest = url
            .strip_prefix("http://")
            .ok_or_else(|| HttpClientError::UnsupportedScheme { url: url.into() })?;
        let (authority, path) = match rest.split_once('/') {
            Some((authority, path)) => (authority, format!("/{path}")),
            None => (rest, "/".to_string()),
        };
        if authority.is_empty() || authority.contains('@') {
            return Err(HttpClientError::InvalidUrl { url: url.into() });
        }
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => {
                let port = port
                    .parse::<u16>()
                    .map_err(|_| HttpClientError::InvalidUrl { url: url.into() })?;
                (host.to_string(), port)
            }
            None => (authority.to_string(), 80),
        };
        if host.is_empty() {
            return Err(HttpClientError::InvalidUrl { url: url.into() });
        }
        Ok(Self { host, port, path })
    }

    fn host_header(&self) -> String {
        if self.port == 80 {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, Mutex},
        thread,
    };

    #[test]
    fn parses_http_url_with_port_and_path() {
        let target = HttpTarget::parse("http://127.0.0.1:8787/spend/executor/validate")
            .expect("url should parse");
        assert_eq!(target.host, "127.0.0.1");
        assert_eq!(target.port, 8787);
        assert_eq!(target.path, "/spend/executor/validate");
    }

    #[test]
    fn rejects_userinfo_urls() {
        let error =
            HttpTarget::parse("http://localhost@vendor.example/spend").expect_err("reject url");
        assert!(error.to_string().contains("invalid URL"));
    }

    #[test]
    fn authenticated_requests_send_one_bearer_header() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let request = Arc::new(Mutex::new(String::new()));
        let captured = Arc::clone(&request);
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .read_to_string(&mut captured.lock().unwrap())
                .unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\n\r\n{\"ok\":true}",
                )
                .unwrap();
        });
        let value: serde_json::Value = post_json_authenticated(
            &format!("http://{address}/test"),
            &serde_json::json!({}),
            Some(b"secret-token"),
        )
        .unwrap();

        assert_eq!(value, serde_json::json!({"ok": true}));
        assert_eq!(
            request
                .lock()
                .unwrap()
                .matches("Authorization: Bearer secret-token")
                .count(),
            1
        );
    }
}
