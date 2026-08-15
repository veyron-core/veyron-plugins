//! request parsing for the sync-client plugin. One action, no params —
//! accepts `{}` or an empty body, rejects anything else.

#[derive(Debug, PartialEq)]
pub enum SyncClientRequest {
    GetState,
}

pub fn parse_request(action: &str, params_json: &[u8]) -> Result<SyncClientRequest, String> {
    match action {
        "sync_client_get_state" => {
            if params_json.is_empty() {
                return Ok(SyncClientRequest::GetState);
            }
            let v: serde_json::Value = serde_json::from_slice(params_json)
                .map_err(|e| format!("invalid params for sync_client_get_state: {e}"))?;
            if !v.is_object() || !v.as_object().is_some_and(|o| o.is_empty()) {
                return Err("sync_client_get_state takes no params (expected {})".to_string());
            }
            Ok(SyncClientRequest::GetState)
        }
        other => Err(format!("unknown action: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_empty_body() {
        assert_eq!(
            parse_request("sync_client_get_state", b"").unwrap(),
            SyncClientRequest::GetState
        );
    }

    #[test]
    fn accepts_empty_object() {
        assert_eq!(
            parse_request("sync_client_get_state", b"{}").unwrap(),
            SyncClientRequest::GetState
        );
    }

    #[test]
    fn rejects_unknown_action() {
        let err = parse_request("sync_client_frobnicate", b"{}").unwrap_err();
        assert!(err.contains("sync_client_frobnicate"), "error was: {err}");
    }

    #[test]
    fn rejects_malformed_json() {
        let err = parse_request("sync_client_get_state", b"not json").unwrap_err();
        assert!(err.contains("sync_client_get_state"), "error was: {err}");
    }

    #[test]
    fn rejects_nonempty_params() {
        let err = parse_request("sync_client_get_state", br#"{"x": 1}"#).unwrap_err();
        assert!(err.contains("no params"), "error was: {err}");
    }
}
