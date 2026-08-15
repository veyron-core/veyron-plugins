use serde::Deserialize;
use serde_json::Value;

#[derive(Debug)]
pub enum SyncRequest {
    Get { key: String },
    Set { key: String, value: Value },
    Del { key: String },
    Snapshot,
}

#[derive(Deserialize)]
struct KeyParams {
    key: String,
}

#[derive(Deserialize)]
struct SetParams {
    key: String,
    value: Value,
}

fn require_nonempty_key(key: String) -> Result<String, String> {
    if key.is_empty() {
        Err("params.key must be a non-empty string".to_string())
    } else {
        Ok(key)
    }
}

pub fn parse_request(action: &str, params_json: &[u8]) -> Result<SyncRequest, String> {
    match action {
        "sync_get_snapshot" => Ok(SyncRequest::Snapshot),
        "sync_get" => {
            let p: KeyParams = serde_json::from_slice(params_json)
                .map_err(|e| format!("invalid params for sync_get, expected {{key}}: {e}"))?;
            Ok(SyncRequest::Get {
                key: require_nonempty_key(p.key)?,
            })
        }
        "sync_set" => {
            let p: SetParams = serde_json::from_slice(params_json).map_err(|e| {
                format!("invalid params for sync_set, expected {{key, value}}: {e}")
            })?;
            Ok(SyncRequest::Set {
                key: require_nonempty_key(p.key)?,
                value: p.value,
            })
        }
        "sync_del" => {
            let p: KeyParams = serde_json::from_slice(params_json)
                .map_err(|e| format!("invalid params for sync_del, expected {{key}}: {e}"))?;
            Ok(SyncRequest::Del {
                key: require_nonempty_key(p.key)?,
            })
        }
        other => Err(format!("unknown action: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sync_get() {
        let req = parse_request("sync_get", br#"{"key": "foo"}"#).unwrap();
        match req {
            SyncRequest::Get { key } => assert_eq!(key, "foo"),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn parses_sync_set() {
        let req = parse_request("sync_set", br#"{"key": "foo", "value": {"a": 1}}"#).unwrap();
        match req {
            SyncRequest::Set { key, value } => {
                assert_eq!(key, "foo");
                assert_eq!(value, serde_json::json!({"a": 1}));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn parses_sync_del() {
        let req = parse_request("sync_del", br#"{"key": "foo"}"#).unwrap();
        assert!(matches!(req, SyncRequest::Del { key } if key == "foo"));
    }

    #[test]
    fn parses_sync_get_snapshot_with_any_params() {
        // params_json is ignored for snapshot (no params to validate), but
        // an empty object must parse.
        let req = parse_request("sync_get_snapshot", b"{}").unwrap();
        assert!(matches!(req, SyncRequest::Snapshot));
    }

    #[test]
    fn rejects_unknown_action() {
        let err = parse_request("sync_frobnicate", b"{}").unwrap_err();
        assert!(err.contains("sync_frobnicate"), "error was: {err}");
    }

    #[test]
    fn rejects_malformed_json() {
        let err = parse_request("sync_get", b"not json").unwrap_err();
        assert!(err.contains("key"), "error was: {err}");
    }

    #[test]
    fn rejects_empty_key() {
        let err = parse_request("sync_get", br#"{"key": ""}"#).unwrap_err();
        assert!(err.contains("key"), "error was: {err}");
    }
}
