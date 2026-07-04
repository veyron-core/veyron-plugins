//! Parse + validate the JSON body of an `http_request` `ActionRequest`.

use std::collections::HashMap;

/// Hard ceiling on `timeout_ms`; matches the kernel's default action
/// timeout. A caller-supplied value above this is clamped down, never
/// rejected.
pub const MAX_TIMEOUT_MS: u64 = 30_000;

#[derive(Debug, PartialEq, Eq)]
pub struct HttpRequestParams {
    pub method: String,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub body: Option<String>,
    pub timeout_ms: u64,
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

    Ok(HttpRequestParams {
        method,
        url: url_str,
        headers: raw.headers,
        body: raw.body,
        timeout_ms,
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
}
