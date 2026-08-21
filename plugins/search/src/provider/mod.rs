//! Per-provider request building and response parsing. Each adapter
//! translates between `search`'s normalized shapes and the provider's own
//! wire format; the actual HTTP send happens in `network`'s `http_request`
//! action (see `crate::handler`), not here.

pub mod brave;
pub mod tavily;

use std::collections::HashMap;

use crate::request::WebSearchParams;

/// Mirrors `network`'s `http_request` action params — built by an adapter,
/// serialized as-is into the `ActionRequest.params_json` sent to `network`.
#[derive(Debug, serde::Serialize)]
pub struct HttpRequestJson {
    pub method: &'static str,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub body: String,
    pub timeout_ms: u64,
}

/// One normalized search hit — the shape `search` returns to its callers,
/// regardless of provider.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Normalized search response: the caller's query plus the (possibly empty)
/// list of hits.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub results: Vec<SearchResult>,
}

/// Adapters are zero-sized unit structs, so `Send + Sync` holds trivially;
/// it's required because the handler's `&dyn Provider` crosses the await
/// boundary inside the spawned serve loop.
pub trait Provider: Send + Sync {
    /// Build the `network` `http_request` params for this search call.
    /// `api_key` is the resolved secret value (never logged, never echoed
    /// back in any error).
    fn build_http_request(&self, params: &WebSearchParams, api_key: &str) -> HttpRequestJson;

    /// Parse the provider's raw HTTP response body into the normalized hits.
    /// Called only on a 2xx status — non-2xx is handled by `crate::handler`
    /// before this is reached. An empty `results` list is a valid outcome.
    fn parse_response(&self, body: &[u8]) -> Result<Vec<SearchResult>, String>;
}
