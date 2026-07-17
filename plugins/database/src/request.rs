use serde::Deserialize;
use serde_json::Value;

#[derive(Debug)]
pub enum DbRequest {
    Get { key: String },
    Set { key: String, value: Value },
    Delete { key: String },
    BatchGet { keys: Vec<String> },
    Query { sql: String, params: Vec<Value> },
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

#[derive(Deserialize)]
struct BatchGetParams {
    keys: Vec<String>,
}

#[derive(Deserialize)]
struct QueryParams {
    sql: String,
    #[serde(default)]
    params: Vec<Value>,
}

fn require_nonempty_key(key: String) -> Result<String, String> {
    if key.is_empty() {
        Err("params.key must be a non-empty string".to_string())
    } else {
        Ok(key)
    }
}

pub fn parse_request(action: &str, params_json: &[u8]) -> Result<DbRequest, String> {
    match action {
        "db_get" => {
            let p: KeyParams = serde_json::from_slice(params_json)
                .map_err(|e| format!("invalid params for db_get, expected {{key}}: {e}"))?;
            Ok(DbRequest::Get {
                key: require_nonempty_key(p.key)?,
            })
        }
        "db_set" => {
            let p: SetParams = serde_json::from_slice(params_json)
                .map_err(|e| format!("invalid params for db_set, expected {{key, value}}: {e}"))?;
            Ok(DbRequest::Set {
                key: require_nonempty_key(p.key)?,
                value: p.value,
            })
        }
        "db_delete" => {
            let p: KeyParams = serde_json::from_slice(params_json)
                .map_err(|e| format!("invalid params for db_delete, expected {{key}}: {e}"))?;
            Ok(DbRequest::Delete {
                key: require_nonempty_key(p.key)?,
            })
        }
        "db_batch_get" => {
            let p: BatchGetParams = serde_json::from_slice(params_json).map_err(|e| {
                format!("invalid params for db_batch_get, expected {{keys: [..]}}: {e}")
            })?;
            if p.keys.is_empty() {
                return Err("params.keys must be a non-empty array".to_string());
            }
            Ok(DbRequest::BatchGet { keys: p.keys })
        }
        "db_query" => {
            let p: QueryParams = serde_json::from_slice(params_json).map_err(|e| {
                format!("invalid params for db_query, expected {{sql, params?}}: {e}")
            })?;
            if p.sql.trim().is_empty() {
                return Err("params.sql must be a non-empty string".to_string());
            }
            Ok(DbRequest::Query {
                sql: p.sql,
                params: p.params,
            })
        }
        other => Err(format!("unknown action: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_db_get() {
        let req = parse_request("db_get", br#"{"key": "foo"}"#).unwrap();
        match req {
            DbRequest::Get { key } => assert_eq!(key, "foo"),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn parses_db_set() {
        let req = parse_request("db_set", br#"{"key": "foo", "value": {"a": 1}}"#).unwrap();
        match req {
            DbRequest::Set { key, value } => {
                assert_eq!(key, "foo");
                assert_eq!(value, serde_json::json!({"a": 1}));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn parses_db_delete() {
        let req = parse_request("db_delete", br#"{"key": "foo"}"#).unwrap();
        assert!(matches!(req, DbRequest::Delete { key } if key == "foo"));
    }

    #[test]
    fn parses_db_batch_get() {
        let req = parse_request("db_batch_get", br#"{"keys": ["a", "b"]}"#).unwrap();
        match req {
            DbRequest::BatchGet { keys } => assert_eq!(keys, vec!["a".to_string(), "b".to_string()]),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn parses_db_query() {
        let req = parse_request(
            "db_query",
            br#"{"sql": "select * from kv where key = ?1", "params": ["foo"]}"#,
        )
        .unwrap();
        match req {
            DbRequest::Query { sql, params } => {
                assert_eq!(sql, "select * from kv where key = ?1");
                assert_eq!(params, vec![serde_json::json!("foo")]);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn db_query_defaults_params_to_empty() {
        let req = parse_request("db_query", br#"{"sql": "select 1"}"#).unwrap();
        assert!(matches!(req, DbRequest::Query { params, .. } if params.is_empty()));
    }

    #[test]
    fn rejects_unknown_action() {
        let err = parse_request("db_frobnicate", b"{}").unwrap_err();
        assert!(err.contains("db_frobnicate"), "error was: {err}");
    }

    #[test]
    fn rejects_malformed_json() {
        let err = parse_request("db_get", b"not json").unwrap_err();
        assert!(err.contains("key"), "error was: {err}");
    }

    #[test]
    fn rejects_empty_key() {
        let err = parse_request("db_get", br#"{"key": ""}"#).unwrap_err();
        assert!(err.contains("key"), "error was: {err}");
    }
}
