use crate::db::{self, DbConfig, DbPools};
use crate::request::{parse_request, DbRequest};
use serde_json::{json, Value};
use sqlx::{Column, Row, SqlitePool, TypeInfo, ValueRef};

pub struct Handler {
    pools: DbPools,
    max_value_bytes: usize,
    max_response_bytes: usize,
}

impl Handler {
    pub fn new(config: DbConfig, max_value_bytes: usize, max_response_bytes: usize) -> Self {
        Self {
            pools: DbPools::new(config),
            max_value_bytes,
            max_response_bytes,
        }
    }

    pub async fn handle(
        &self,
        caller_plugin_id: &str,
        action: &str,
        params_json: &[u8],
    ) -> Result<Vec<u8>, String> {
        if caller_plugin_id.is_empty() {
            return Err("missing caller_plugin_id (rejected before touching any database)".to_string());
        }
        let req = parse_request(action, params_json)?;
        let pool = self.pools.pool_for(caller_plugin_id).await?;

        let result = match req {
            DbRequest::Get { key } => self.handle_get(&pool, &key).await?,
            DbRequest::Set { key, value } => self.handle_set(&pool, &key, &value).await?,
            DbRequest::Delete { key } => self.handle_delete(&pool, &key).await?,
            DbRequest::BatchGet { keys } => self.handle_batch_get(&pool, &keys).await?,
            DbRequest::Query { sql, params } => self.handle_query(&pool, &sql, &params).await?,
        };

        serde_json::to_vec(&result).map_err(|e| format!("failed to encode response: {e}"))
    }

    async fn handle_get(&self, pool: &SqlitePool, key: &str) -> Result<Value, String> {
        let row: Option<(String,)> = sqlx::query_as("select value from kv where key = ?1")
            .bind(key)
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())?;
        match row {
            Some((raw,)) => {
                let value: Value = serde_json::from_str(&raw)
                    .map_err(|e| format!("corrupt stored value for key {key:?}: {e}"))?;
                Ok(json!({"found": true, "value": value}))
            }
            None => Ok(json!({"found": false, "value": null})),
        }
    }

    async fn handle_set(&self, pool: &SqlitePool, key: &str, value: &Value) -> Result<Value, String> {
        let raw = serde_json::to_string(value).map_err(|e| format!("failed to encode value: {e}"))?;
        if raw.len() > self.max_value_bytes {
            return Err(format!(
                "value exceeds max_value_bytes ({} > {})",
                raw.len(),
                self.max_value_bytes
            ));
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        sqlx::query(
            "insert into kv (key, value, updated_at) values (?1, ?2, ?3) \
             on conflict(key) do update set value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(key)
        .bind(&raw)
        .bind(now_ms)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(json!({"ok": true}))
    }

    async fn handle_delete(&self, pool: &SqlitePool, key: &str) -> Result<Value, String> {
        let result = sqlx::query("delete from kv where key = ?1")
            .bind(key)
            .execute(pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(json!({"deleted": result.rows_affected() > 0}))
    }

    async fn handle_batch_get(&self, pool: &SqlitePool, keys: &[String]) -> Result<Value, String> {
        let mut values = serde_json::Map::new();
        for key in keys {
            let row: Option<(String,)> = sqlx::query_as("select value from kv where key = ?1")
                .bind(key)
                .fetch_optional(pool)
                .await
                .map_err(|e| e.to_string())?;
            let value = match row {
                Some((raw,)) => serde_json::from_str(&raw)
                    .map_err(|e| format!("corrupt stored value for key {key:?}: {e}"))?,
                None => Value::Null,
            };
            values.insert(key.clone(), value);
        }
        Ok(json!({"values": Value::Object(values)}))
    }

    async fn handle_query(&self, pool: &SqlitePool, sql: &str, params: &[Value]) -> Result<Value, String> {
        if db::rejects_attach(sql) {
            return Err("ATTACH is not permitted in db_query statements".to_string());
        }

        let is_select = sql.trim_start().to_ascii_lowercase().starts_with("select")
            || sql.trim_start().to_ascii_lowercase().starts_with("pragma")
            || sql.trim_start().to_ascii_lowercase().starts_with("with");

        if is_select {
            let mut query = sqlx::query(sql);
            for p in params {
                query = bind_json_param(query, p);
            }
            let rows = query.fetch_all(pool).await.map_err(|e| e.to_string())?;
            let mut json_rows = Vec::with_capacity(rows.len());
            for row in &rows {
                json_rows.push(row_to_json(row)?);
            }
            let encoded = serde_json::to_vec(&json_rows).map_err(|e| e.to_string())?;
            if encoded.len() > self.max_response_bytes {
                return Err(format!(
                    "query result exceeds max_response_bytes ({} > {})",
                    encoded.len(),
                    self.max_response_bytes
                ));
            }
            Ok(json!({"rows": json_rows, "rows_affected": 0}))
        } else {
            let mut query = sqlx::query(sql);
            for p in params {
                query = bind_json_param(query, p);
            }
            let outcome = query.execute(pool).await.map_err(|e| e.to_string())?;
            Ok(json!({"rows": [], "rows_affected": outcome.rows_affected()}))
        }
    }
}

fn bind_json_param<'q>(
    query: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
    value: &'q Value,
) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>> {
    match value {
        Value::Null => query.bind(None::<String>),
        Value::Bool(b) => query.bind(*b as i64),
        Value::Number(n) if n.is_i64() => query.bind(n.as_i64().unwrap()),
        Value::Number(n) => query.bind(n.as_f64().unwrap_or_default()),
        Value::String(s) => query.bind(s.clone()),
        other => query.bind(other.to_string()),
    }
}

fn row_to_json(row: &sqlx::sqlite::SqliteRow) -> Result<Value, String> {
    let mut obj = serde_json::Map::new();
    for (i, col) in row.columns().iter().enumerate() {
        let raw = row.try_get_raw(i).map_err(|e| e.to_string())?;
        let value = if raw.is_null() {
            Value::Null
        } else {
            match raw.type_info().name() {
                "TEXT" => Value::String(row.try_get::<String, _>(i).map_err(|e| e.to_string())?),
                "INTEGER" => json!(row.try_get::<i64, _>(i).map_err(|e| e.to_string())?),
                "REAL" => json!(row.try_get::<f64, _>(i).map_err(|e| e.to_string())?),
                "BLOB" => {
                    let bytes = row.try_get::<Vec<u8>, _>(i).map_err(|e| e.to_string())?;
                    json!(base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes))
                }
                other => return Err(format!("unsupported SQLite column type: {other}")),
            }
        };
        obj.insert(col.name().to_string(), value);
    }
    Ok(Value::Object(obj))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbConfig;

    fn handler(dir: &std::path::Path) -> Handler {
        Handler::new(
            DbConfig {
                data_dir: dir.to_path_buf(),
                pool_size: 2,
                busy_timeout_ms: 1000,
            },
            1024,
            4096,
        )
    }

    #[tokio::test]
    async fn set_then_get_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let h = handler(dir.path());

        let set_out = h
            .handle("caller_a", "db_set", br#"{"key": "foo", "value": {"n": 1}}"#)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&set_out).unwrap(),
            serde_json::json!({"ok": true})
        );

        let get_out = h.handle("caller_a", "db_get", br#"{"key": "foo"}"#).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&get_out).unwrap(),
            serde_json::json!({"found": true, "value": {"n": 1}})
        );
    }

    #[tokio::test]
    async fn get_missing_key_returns_found_false() {
        let dir = tempfile::tempdir().unwrap();
        let h = handler(dir.path());
        let out = h.handle("caller_a", "db_get", br#"{"key": "nope"}"#).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&out).unwrap(),
            serde_json::json!({"found": false, "value": null})
        );
    }

    #[tokio::test]
    async fn delete_reports_whether_a_row_was_removed() {
        let dir = tempfile::tempdir().unwrap();
        let h = handler(dir.path());
        h.handle("caller_a", "db_set", br#"{"key": "foo", "value": 1}"#)
            .await
            .unwrap();

        let out1 = h.handle("caller_a", "db_delete", br#"{"key": "foo"}"#).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&out1).unwrap(),
            serde_json::json!({"deleted": true})
        );

        let out2 = h.handle("caller_a", "db_delete", br#"{"key": "foo"}"#).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&out2).unwrap(),
            serde_json::json!({"deleted": false})
        );
    }

    #[tokio::test]
    async fn batch_get_returns_map_with_nulls_for_missing() {
        let dir = tempfile::tempdir().unwrap();
        let h = handler(dir.path());
        h.handle("caller_a", "db_set", br#"{"key": "a", "value": 1}"#).await.unwrap();

        let out = h
            .handle("caller_a", "db_batch_get", br#"{"keys": ["a", "b"]}"#)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&out).unwrap(),
            serde_json::json!({"values": {"a": 1, "b": null}})
        );
    }

    #[tokio::test]
    async fn query_reads_back_through_kv_table() {
        let dir = tempfile::tempdir().unwrap();
        let h = handler(dir.path());
        h.handle("caller_a", "db_set", br#"{"key": "foo", "value": "bar"}"#)
            .await
            .unwrap();

        let out = h
            .handle(
                "caller_a",
                "db_query",
                br#"{"sql": "select key, value from kv where key = ?1", "params": ["foo"]}"#,
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["rows"][0]["key"], "foo");
        assert_eq!(v["rows"][0]["value"], "\"bar\"");
    }

    #[tokio::test]
    async fn query_rejects_attach() {
        let dir = tempfile::tempdir().unwrap();
        let h = handler(dir.path());
        let err = h
            .handle(
                "caller_a",
                "db_query",
                br#"{"sql": "attach database 'x.db' as evil"}"#,
            )
            .await
            .unwrap_err();
        assert!(err.to_lowercase().contains("attach"), "error was: {err}");
    }

    #[tokio::test]
    async fn callers_are_isolated() {
        let dir = tempfile::tempdir().unwrap();
        let h = handler(dir.path());
        h.handle("caller_a", "db_set", br#"{"key": "k", "value": "from_a"}"#)
            .await
            .unwrap();
        h.handle("caller_b", "db_set", br#"{"key": "k", "value": "from_b"}"#)
            .await
            .unwrap();

        let a = h.handle("caller_a", "db_get", br#"{"key": "k"}"#).await.unwrap();
        let b = h.handle("caller_b", "db_get", br#"{"key": "k"}"#).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&a).unwrap()["value"],
            "from_a"
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&b).unwrap()["value"],
            "from_b"
        );
    }

    #[tokio::test]
    async fn set_rejects_oversized_value() {
        let dir = tempfile::tempdir().unwrap();
        let h = handler(dir.path());
        let big_value = serde_json::json!("x".repeat(2000));
        let params = serde_json::to_vec(&serde_json::json!({"key": "foo", "value": big_value})).unwrap();
        let err = h.handle("caller_a", "db_set", &params).await.unwrap_err();
        assert!(err.contains("max_value_bytes"), "error was: {err}");
    }

    #[tokio::test]
    async fn rejects_missing_caller_id() {
        let dir = tempfile::tempdir().unwrap();
        let h = handler(dir.path());
        let err = h.handle("", "db_get", br#"{"key": "foo"}"#).await.unwrap_err();
        assert!(err.contains("caller_plugin_id"), "error was: {err}");
    }

    #[tokio::test]
    async fn rejects_unknown_action() {
        let dir = tempfile::tempdir().unwrap();
        let h = handler(dir.path());
        let err = h.handle("caller_a", "db_frobnicate", b"{}").await.unwrap_err();
        assert!(err.contains("db_frobnicate"), "error was: {err}");
    }
}
