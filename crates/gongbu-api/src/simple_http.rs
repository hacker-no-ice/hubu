use std::{
    fmt,
    io::{Read, Write},
    net::{Shutdown, TcpStream},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;

const READ_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Value,
}

impl HttpResponse {
    pub fn ok(body: Value) -> Self {
        Self { status: 200, body }
    }

    pub fn bad_request(error: impl fmt::Display) -> Self {
        Self {
            status: 400,
            body: json!({ "error": error.to_string() }),
        }
    }

    pub fn not_found(method: &str, path: &str) -> Self {
        Self {
            status: 404,
            body: json!({ "error": format!("no route for {method} {path}") }),
        }
    }
}

#[derive(Debug, Error)]
pub enum HttpParseError {
    #[error("missing request line")]
    MissingRequestLine,
    #[error("invalid request line")]
    InvalidRequestLine,
}

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

pub fn parse_request(raw: &str) -> Result<HttpRequest, HttpParseError> {
    let (head, body) = split_head_and_body(raw);
    let request_line = head
        .lines()
        .next()
        .ok_or(HttpParseError::MissingRequestLine)?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or(HttpParseError::InvalidRequestLine)?
        .to_string();
    let path = parts
        .next()
        .ok_or(HttpParseError::InvalidRequestLine)?
        .to_string();
    if parts.next().is_none() {
        return Ok(HttpRequest {
            method,
            path,
            body: body.to_string(),
        });
    }
    Ok(HttpRequest {
        method,
        path,
        body: body.to_string(),
    })
}

pub fn write_response(stream: &mut impl Write, response: &HttpResponse) -> std::io::Result<()> {
    let body = response.body.to_string();
    write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        response.status,
        reason_phrase(response.status),
        body.len(),
        body
    )
}

pub fn post_json<T, R>(url: &str, body: &T) -> Result<R, HttpClientError>
where
    T: Serialize,
    R: for<'de> Deserialize<'de>,
{
    let target = HttpTarget::parse(url)?;
    let body = serde_json::to_string(body)?;
    let mut stream = TcpStream::connect((target.host.as_str(), target.port))?;
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    write!(
        stream,
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        target.path,
        target.host_header(),
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

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    }
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
}
