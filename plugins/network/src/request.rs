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

#[derive(Debug, PartialEq, Eq)]
pub struct HttpRequestParams {
    pub method: String,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub body: Option<String>,
    pub timeout_ms: u64,
    pub max_retries: u32,
    pub retry_backoff_ms: u64,
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
    }

    let raw: Raw =
        serde_json::from_slice(params_json).map_err(|e| format!("invalid JSON: {e}"))?;

    let method = raw.method.ok_or("missing required field: method")?;
    let method = method.to_uppercase();
    if !ALLOWED_METHODS.contains(&method.as_str()) {
        return Err(format!("unsupported method: {method}"));
    }

    let url_str = raw.url.ok_or("missing required field: url")?;
    let parsed = url::Url::parse(&url_str).map_err(|e| format!("invalid url: {e}"))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(format!("blocked scheme: {}", parsed.scheme()));
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
    fn clamps_retry_backoff_above_cap() {
        let params = parse_request(
            br#"{"method": "GET", "url": "https://example.com", "retry_backoff_ms": 999999}"#,
        )
        .unwrap();
        assert_eq!(params.retry_backoff_ms, MAX_RETRY_BACKOFF_MS);
    }
}
