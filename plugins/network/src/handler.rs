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
//! `fetch` itself has no SSRF gate, so its tests exercise the HTTP-send
//! logic directly against a loopback mock server with a plain
//! `reqwest::Client`, without depending on `ssrf::is_blocked_ip` (left as a
//! TODO for the plugin author) and without loopback being rejected.

use std::collections::HashMap;
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use crate::request::HttpRequestParams;
use crate::ssrf::{self, Blocklist};

/// Response bodies larger than this are rejected outright (`ACTION_ERROR`),
/// never silently truncated.
pub const MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub struct HttpResponseJson {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String,
}

/// DNS resolver that filters out any IP blocked by [`ssrf::is_blocked_ip`].
/// Install via `Client::builder().dns_resolver(...)` so it's the single,
/// authoritative resolution used for both the initial connect and any
/// redirect hop — no separate pre-flight check to go stale.
#[derive(Clone, Default)]
pub struct SsrfSafeResolver {
    pub extra_blocklist: Blocklist,
}

impl Resolve for SsrfSafeResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let extra_blocklist = self.extra_blocklist.clone();
        Box::pin(async move {
            let host = name.as_str().to_string();
            if extra_blocklist.blocks_host(&host) {
                return Err(format!("host {host} is blocked by operator blocklist").into());
            }

            let resolved = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;

            let allowed: Vec<_> = resolved
                .filter(|a| !ssrf::is_blocked_ip(a.ip()) && !extra_blocklist.blocks_ip(&a.ip()))
                .collect();
            if allowed.is_empty() {
                return Err(format!("all resolved IPs for {host} are blocked by SSRF policy").into());
            }
            Ok(Box::new(allowed.into_iter()) as Addrs)
        })
    }
}

/// Send the HTTP request and map the response. SSRF gating happens inside
/// the `client`'s DNS resolver, not here — see module docs.
pub async fn fetch(
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

    Ok(HttpResponseJson {
        status,
        headers,
        body: String::from_utf8_lossy(&body_bytes).into_owned(),
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
