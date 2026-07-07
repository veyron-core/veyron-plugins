//! Executes an already-validated [`HttpRequestParams`] and maps the result
//! into the JSON shape the plugin returns in `ActionResponse.data_json`.
//!
//! SSRF protection lives in [`SsrfSafeResolver`], plugged in as the
//! `reqwest::Client`'s DNS resolver (see `main.rs`) rather than as a
//! pre-flight check. A separate pre-flight resolve-then-connect has a
//! DNS-rebinding TOCTOU: the name can re-resolve to a different (blocked)
//! IP between the check and the actual connect, and it doesn't cover
//! redirects to a new host. Gating at the resolver makes every connection
//! reqwest makes — initial request and any followed redirect — resolve
//! through the same authoritative, filtered lookup. Redirects are also
//! disabled at the client level (`main.rs`) as defense in depth.
//!
//! `SsrfSafeResolver` only runs for hostnames, though — `reqwest`/`hyper`
//! skip DNS resolution entirely when a URL's host is already a literal IP,
//! so it never reaches this resolver at all. `main.rs` covers that gap with
//! an explicit `ssrf::check_literal_ip_host` call, both before the initial
//! request (`handle_http_request`) and on every redirect hop
//! (`redirect_policy`).
//!
//! `fetch` itself has no SSRF gate, so its tests exercise the HTTP-send
//! logic directly against a loopback mock server with a plain
//! `reqwest::Client`, without depending on `ssrf::is_blocked_ip` (left as a
//! TODO for the plugin author) and without loopback being rejected.

use std::collections::HashMap;
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use crate::request::HttpRequestParams;
use crate::ssrf::{self, Allowlist, Blocklist};

/// Response bodies larger than this are rejected outright (`ACTION_ERROR`),
/// never silently truncated.
pub const MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub struct HttpResponseJson {
    pub status: u16,
    pub headers: HashMap<String, String>,
    /// Text body as-is when valid UTF-8, otherwise base64 — see
    /// `body_encoding`. Never lossily mangled: binary responses (images,
    /// protobuf, etc.) round-trip exactly via the base64 path.
    pub body: String,
    /// `"utf8"` or `"base64"`, telling the caller how to interpret `body`.
    pub body_encoding: &'static str,
}

/// DNS resolver that filters out any IP blocked by [`ssrf::is_blocked_ip`].
/// Install via `Client::builder().dns_resolver(...)` so it's the single,
/// authoritative resolution used for both the initial connect and any
/// redirect hop — no separate pre-flight check to go stale.
#[derive(Clone, Default)]
pub struct SsrfSafeResolver {
    pub extra_blocklist: Blocklist,
    /// When non-empty, switches from default-block (built-in ranges) to
    /// default-deny: only hosts/IPs listed here (or in neither, minus
    /// `extra_blocklist`) may be reached — see [`Allowlist`] docs.
    pub allowlist: Allowlist,
}

impl Resolve for SsrfSafeResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let extra_blocklist = self.extra_blocklist.clone();
        let allowlist = self.allowlist.clone();
        Box::pin(async move {
            let host = name.as_str().to_string();
            if extra_blocklist.blocks_host(&host) {
                return Err(format!("host {host} is blocked by operator blocklist").into());
            }
            let resolved = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;

            let allowed: Vec<_> = resolved
                .filter(|a| {
                    if extra_blocklist.blocks_ip(&a.ip()) {
                        return false;
                    }
                    if !allowlist.is_empty() {
                        allowlist.allows_host(&host) || allowlist.allows_ip(&a.ip())
                    } else {
                        !ssrf::is_blocked_ip(a.ip())
                    }
                })
                .collect();
            if allowed.is_empty() {
                return Err(format!("all resolved IPs for {host} are blocked by SSRF policy").into());
            }
            Ok(Box::new(allowed.into_iter()) as Addrs)
        })
    }
}

/// Response statuses worth retrying: rate-limited or transient server-side
/// failure. Anything else (including other 4xx) is the caller's problem, not
/// a transient one, so it's returned as-is on the first attempt.
fn is_retryable_status(status: u16) -> bool {
    matches!(status, 429 | 500 | 502 | 503 | 504)
}

/// Send the HTTP request and map the response, retrying transient failures
/// up to `params.max_retries` times with exponential backoff
/// (`params.retry_backoff_ms`, doubling, capped at
/// [`crate::request::MAX_RETRY_BACKOFF_MS`]). SSRF gating happens inside the
/// `client`'s DNS resolver, not here — see module docs.
pub async fn fetch(
    client: &reqwest::Client,
    params: &HttpRequestParams,
) -> Result<HttpResponseJson, String> {
    let host = reqwest::Url::parse(&params.url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_default();
    let started = std::time::Instant::now();
    let mut backoff_ms = params.retry_backoff_ms;
    let mut attempt = 0;

    loop {
        let result = fetch_once(client, params).await;
        let retry = attempt < params.max_retries
            && match &result {
                Ok(resp) => is_retryable_status(resp.status),
                Err(_) => true,
            };

        // One-line JSON per attempt so operators can pipe stdout straight
        // into normal log aggregation instead of parsing a custom format.
        let log_line = serde_json::json!({
            "plugin": "network",
            "method": params.method,
            "host": host,
            "attempt": attempt + 1,
            "status": result.as_ref().ok().map(|r| r.status),
            "error": result.as_ref().err(),
            "duration_ms": started.elapsed().as_millis(),
        });
        println!("{log_line}");

        if !retry {
            return result;
        }
        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
        backoff_ms = (backoff_ms * 2).min(crate::request::MAX_RETRY_BACKOFF_MS);
        attempt += 1;
    }
}

async fn fetch_once(
    client: &reqwest::Client,
    params: &HttpRequestParams,
) -> Result<HttpResponseJson, String> {
    let method = reqwest::Method::from_bytes(params.method.as_bytes())
        .map_err(|e| format!("invalid method: {e}"))?;

    let mut req = client
        .request(method, &params.url)
        .timeout(Duration::from_millis(params.timeout_ms));

    for (k, v) in &params.headers {
        req = req.header(k, v);
    }
    if let Some(body) = &params.body {
        req = req.body(body.clone());
    }

    let mut resp = req.send().await.map_err(|e| format!("request failed: {e}"))?;
    let status = resp.status().as_u16();
    let headers = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_string()))
        .collect();

    let mut body_bytes = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("body read error: {e}"))? {
        body_bytes.extend_from_slice(&chunk);
        if body_bytes.len() > MAX_BODY_BYTES {
            return Err("response body exceeds 10 MiB cap".into());
        }
    }

    let (body, body_encoding) = match String::from_utf8(body_bytes) {
        Ok(text) => (text, "utf8"),
        Err(e) => {
            use base64::Engine;
            (
                base64::engine::general_purpose::STANDARD.encode(e.into_bytes()),
                "base64",
            )
        }
    };

    Ok(HttpResponseJson {
        status,
        headers,
        body,
        body_encoding,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn params(url: String) -> HttpRequestParams {
        HttpRequestParams {
            method: "GET".into(),
            url,
            headers: HashMap::new(),
            body: None,
            timeout_ms: 5000,
            max_retries: 0,
            retry_backoff_ms: 1,
            follow_redirects: false,
        }
    }

    async fn mock_server(response: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let _ = socket.write_all(response.as_bytes()).await;
        });
        format!("http://{addr}/")
    }

    #[tokio::test]
    async fn fetch_returns_status_headers_body() {
        let url = mock_server(
            "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: 5\r\n\r\nhello",
        )
        .await;
        let client = reqwest::Client::new();
        let resp = fetch(&client, &params(url)).await.unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.headers.get("content-type").unwrap(), "text/plain");
        assert_eq!(resp.body, "hello");
    }

    #[tokio::test]
    async fn fetch_base64_encodes_non_utf8_body() {
        let raw_body: &[u8] = &[0xff, 0xfe, 0xfd, 0x00, 0x01];
        let mut response = b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\n".to_vec();
        response.extend_from_slice(raw_body);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let _ = socket.write_all(&response).await;
        });
        let client = reqwest::Client::new();
        let resp = fetch(&client, &params(format!("http://{addr}/"))).await.unwrap();
        assert_eq!(resp.body_encoding, "base64");
        use base64::Engine;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&resp.body)
            .unwrap();
        assert_eq!(decoded, raw_body);
    }

    #[tokio::test]
    async fn fetch_returns_utf8_encoding_for_text_body() {
        let url = mock_server("HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\nhello").await;
        let client = reqwest::Client::new();
        let resp = fetch(&client, &params(url)).await.unwrap();
        assert_eq!(resp.body_encoding, "utf8");
    }

    #[tokio::test]
    async fn fetch_errors_on_body_over_cap() {
        let big_body = "x".repeat(MAX_BODY_BYTES + 1);
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{}",
            big_body.len(),
            big_body
        );
        let url = mock_server(Box::leak(response.into_boxed_str())).await;
        let client = reqwest::Client::new();
        let err = fetch(&client, &params(url)).await.unwrap_err();
        assert!(err.contains("10 MiB"), "error was: {err}");
    }

    #[tokio::test]
    async fn fetch_does_not_follow_redirect_by_default() {
        let url = mock_server(
            "HTTP/1.1 302 Found\r\nlocation: http://example.invalid/\r\ncontent-length: 0\r\n\r\n",
        )
        .await;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let resp = fetch(&client, &params(url)).await.unwrap();
        assert_eq!(resp.status, 302);
    }

    #[tokio::test]
    async fn fetch_follows_redirect_when_client_allows_it() {
        let final_addr = {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 1024];
                let _ = socket.read(&mut buf).await;
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
                    .await;
            });
            addr
        };
        let redirect_response = format!(
            "HTTP/1.1 302 Found\r\nlocation: http://{final_addr}/\r\ncontent-length: 0\r\n\r\n"
        );
        let url = mock_server(Box::leak(redirect_response.into_boxed_str())).await;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .unwrap();
        let resp = fetch(&client, &params(url)).await.unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, "ok");
    }

    #[tokio::test]
    async fn fetch_retries_on_retryable_status_then_succeeds() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            for response in [
                "HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\n\r\n",
                "HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok",
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 1024];
                let _ = socket.read(&mut buf).await;
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        let client = reqwest::Client::new();
        let mut p = params(format!("http://{addr}/"));
        p.max_retries = 1;
        let resp = fetch(&client, &p).await.unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, "ok");
    }

    #[tokio::test]
    async fn fetch_does_not_retry_non_retryable_status() {
        let url = mock_server("HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n").await;
        let client = reqwest::Client::new();
        let mut p = params(url);
        p.max_retries = 3;
        let resp = fetch(&client, &p).await.unwrap();
        assert_eq!(resp.status, 404);
    }

    #[tokio::test]
    async fn fetch_errors_on_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            // Never write a response — force the client-side timeout.
            tokio::time::sleep(Duration::from_secs(5)).await;
            drop(socket);
        });
        let client = reqwest::Client::new();
        let mut p = params(format!("http://{addr}/"));
        p.timeout_ms = 100;
        let err = fetch(&client, &p).await.unwrap_err();
        assert!(err.contains("request failed"), "error was: {err}");
    }
}
