//! Parse + validate the JSON body of an `http_request` `ActionRequest`.

use std::collections::HashMap;

/// Hard ceiling on `timeout_ms`; matches the kernel's default action
/// timeout. A caller-supplied value above this is clamped down, never
/// rejected.
pub const MAX_TIMEOUT_MS: u64 = 30_000;

/// Hard ceiling on `max_retries`. Retries are opt-in per request — a caller
/// that doesn't set `max_retries` gets none, so an unmodified caller sees no
/// behavior change.
pub const MAX_RETRIES: u32 = 5;

/// Default initial backoff between retry attempts, used when the caller
/// omits `retry_backoff_ms`.
pub const DEFAULT_RETRY_BACKOFF_MS: u64 = 200;

/// Hard ceiling on `retry_backoff_ms` (also caps the exponential growth
/// between attempts), so a caller can't turn a retry into a multi-minute
/// stall.
pub const MAX_RETRY_BACKOFF_MS: u64 = 5_000;

/// Hard ceiling on URL length. Rejected outright, never truncated.
pub const MAX_URL_LEN: usize = 8 * 1024;

/// Hard ceiling on header count. Rejected outright.
pub const MAX_HEADER_COUNT: usize = 100;

/// Hard ceiling on total header bytes (sum of every key+value length).
/// Rejected outright — this bounds worst-case memory for a request with
/// many small headers as well as a few huge ones.
pub const MAX_HEADERS_TOTAL_BYTES: usize = 32 * 1024;

/// Redirects are disabled unless `follow_redirects` is set, and even then
/// capped at this many hops (not caller-configurable — see main.rs, which
/// builds one fixed redirect-enabled client rather than one per request).
/// Every hop still resolves through `SsrfSafeResolver`.
pub const MAX_REDIRECTS: usize = 10;

#[derive(Debug, PartialEq, Eq)]
pub struct HttpRequestParams {
    pub method: String,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub body: Option<String>,
    pub timeout_ms: u64,
    pub max_retries: u32,
    pub retry_backoff_ms: u64,
    pub follow_redirects: bool,
}

const ALLOWED_METHODS: &[&str] = &[
    "GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS",
];

/// Parse and validate `params_json` for the `http_request` action.
/// Returns a human-readable error message on any validation failure —
/// caller maps that straight into `ActionResponse.error`.
pub fn parse_request(params_json: &[u8]) -> Result<HttpRequestParams, String> {
    #[derive(serde::Deserialize)]
    struct Raw {
        method: Option<String>,
        url: Option<String>,
        #[serde(default)]
        headers: HashMap<String, String>,
        body: Option<String>,
        timeout_ms: Option<u64>,
        max_retries: Option<u32>,
        retry_backoff_ms: Option<u64>,
        follow_redirects: Option<bool>,
    }

    let raw: Raw =
        serde_json::from_slice(params_json).map_err(|e| format!("invalid JSON: {e}"))?;

    let method = raw.method.ok_or("missing required field: method")?;
    let method = method.to_uppercase();
    if !ALLOWED_METHODS.contains(&method.as_str()) {
        return Err(format!("unsupported method: {method}"));
    }

    let url_str = raw.url.ok_or("missing required field: url")?;
    if url_str.len() > MAX_URL_LEN {
        return Err(format!("url exceeds {MAX_URL_LEN}-byte cap"));
    }
    let parsed = url::Url::parse(&url_str).map_err(|e| format!("invalid url: {e}"))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(format!("blocked scheme: {}", parsed.scheme()));
    }

    if raw.headers.len() > MAX_HEADER_COUNT {
        return Err(format!("too many headers: max {MAX_HEADER_COUNT}"));
    }
    let headers_total_bytes: usize = raw.headers.iter().map(|(k, v)| k.len() + v.len()).sum();
    if headers_total_bytes > MAX_HEADERS_TOTAL_BYTES {
        return Err(format!(
            "headers exceed {MAX_HEADERS_TOTAL_BYTES}-byte total cap"
        ));
    }

    let timeout_ms = raw.timeout_ms.unwrap_or(MAX_TIMEOUT_MS).min(MAX_TIMEOUT_MS);
    let max_retries = raw.max_retries.unwrap_or(0).min(MAX_RETRIES);
    let retry_backoff_ms = raw
        .retry_backoff_ms
        .unwrap_or(DEFAULT_RETRY_BACKOFF_MS)
        .min(MAX_RETRY_BACKOFF_MS);

    Ok(HttpRequestParams {
        method,
        url: url_str,
        headers: raw.headers,
        body: raw.body,
        timeout_ms,
        max_retries,
        retry_backoff_ms,
        follow_redirects: raw.follow_redirects.unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_missing_url() {
        let err = parse_request(br#"{"method": "GET"}"#).unwrap_err();
        assert!(err.contains("url"), "error was: {err}");
    }

    #[test]
    fn rejects_bad_scheme() {
        let err = parse_request(br#"{"method": "GET", "url": "file:///etc/passwd"}"#)
            .unwrap_err();
        assert!(err.contains("scheme"), "error was: {err}");
    }

    #[test]
    fn rejects_bad_method() {
        let err =
            parse_request(br#"{"method": "TRACE", "url": "https://example.com"}"#).unwrap_err();
        assert!(err.contains("method"), "error was: {err}");
    }

    #[test]
    fn accepts_minimal_valid_request() {
        let params =
            parse_request(br#"{"method": "get", "url": "https://example.com/thing"}"#).unwrap();
        assert_eq!(params.method, "GET");
        assert_eq!(params.url, "https://example.com/thing");
        assert_eq!(params.timeout_ms, MAX_TIMEOUT_MS);
        assert!(params.body.is_none());
    }

    #[test]
    fn clamps_timeout_above_cap() {
        let params = parse_request(
            br#"{"method": "GET", "url": "https://example.com", "timeout_ms": 999999}"#,
        )
        .unwrap();
        assert_eq!(params.timeout_ms, MAX_TIMEOUT_MS);
    }

    #[test]
    fn preserves_timeout_below_cap() {
        let params = parse_request(
            br#"{"method": "GET", "url": "https://example.com", "timeout_ms": 500}"#,
        )
        .unwrap();
        assert_eq!(params.timeout_ms, 500);
    }

    #[test]
    fn defaults_to_no_retries() {
        let params =
            parse_request(br#"{"method": "GET", "url": "https://example.com"}"#).unwrap();
        assert_eq!(params.max_retries, 0);
        assert_eq!(params.retry_backoff_ms, DEFAULT_RETRY_BACKOFF_MS);
    }

    #[test]
    fn clamps_max_retries_above_cap() {
        let params = parse_request(
            br#"{"method": "GET", "url": "https://example.com", "max_retries": 99}"#,
        )
        .unwrap();
        assert_eq!(params.max_retries, MAX_RETRIES);
    }

    #[test]
    fn rejects_url_over_length_cap() {
        let long_path = "a".repeat(MAX_URL_LEN);
        let url = format!("https://example.com/{long_path}");
        let body = serde_json::json!({"method": "GET", "url": url}).to_string();
        let err = parse_request(body.as_bytes()).unwrap_err();
        assert!(err.contains("url"), "error was: {err}");
    }

    #[test]
    fn rejects_too_many_headers() {
        let headers: HashMap<String, String> = (0..MAX_HEADER_COUNT + 1)
            .map(|i| (format!("h{i}"), "v".to_string()))
            .collect();
        let body = serde_json::json!({
            "method": "GET",
            "url": "https://example.com",
            "headers": headers,
        })
        .to_string();
        let err = parse_request(body.as_bytes()).unwrap_err();
        assert!(err.contains("too many headers"), "error was: {err}");
    }

    #[test]
    fn rejects_headers_over_total_byte_cap() {
        let mut headers = HashMap::new();
        headers.insert("h".to_string(), "v".repeat(MAX_HEADERS_TOTAL_BYTES + 1));
        let body = serde_json::json!({
            "method": "GET",
            "url": "https://example.com",
            "headers": headers,
        })
        .to_string();
        let err = parse_request(body.as_bytes()).unwrap_err();
        assert!(err.contains("byte total cap"), "error was: {err}");
    }

    #[test]
    fn clamps_retry_backoff_above_cap() {
        let params = parse_request(
            br#"{"method": "GET", "url": "https://example.com", "retry_backoff_ms": 999999}"#,
        )
        .unwrap();
        assert_eq!(params.retry_backoff_ms, MAX_RETRY_BACKOFF_MS);
    }
}
