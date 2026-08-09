use crate::db::{self, DbConfig, DbPools};
use crate::request::{parse_request, DbRequest};
use futures_util::TryStreamExt;
use serde_json::{json, Value};
use sqlx::{Column, Either, Executor, Row, SqlitePool, TypeInfo, ValueRef};

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
        // Track the serialized size as we go and reject once it crosses
        // max_response_bytes, matching db_query. Without this a caller could
        // pull an unbounded blob back in one batch even though every
        // individual value passed the db_set cap (bug #2).
        let mut running_bytes: usize = 0;
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
            // Account for the value plus its key and JSON punctuation.
            running_bytes += serde_json::to_vec(&value).map_err(|e| e.to_string())?.len()
                + key.len()
                + 4;
            if running_bytes > self.max_response_bytes {
                return Err(format!(
                    "batch_get result exceeds max_response_bytes (> {})",
                    self.max_response_bytes
                ));
            }
            values.insert(key.clone(), value);
        }
        Ok(json!({"values": Value::Object(values)}))
    }

    async fn handle_query(&self, pool: &SqlitePool, sql: &str, params: &[Value]) -> Result<Value, String> {
        if db::rejects_attach(sql) {
            return Err("ATTACH is not permitted in db_query statements".to_string());
        }

        // One streaming pass over `fetch_many`, which yields `Either::Right`
        // for each result row and `Either::Left` for the statement's final
        // query result (carrying `rows_affected`). This replaces the old
        // `starts_with("select")` sniff, which misrouted `INSERT ...
        // RETURNING` (a row-producing write) to the execute-only path and
        // dropped its rows (bug #4). Streaming also lets us enforce
        // `max_response_bytes` incrementally instead of after materializing
        // every row (bug #3): `running_bytes` tracks the serialized size of
        // the rows collected so far and bails the moment it crosses the cap,
        // so a runaway `SELECT` can't balloon memory before the check fires.
        let mut query = sqlx::query(sql);
        for p in params {
            query = bind_json_param(query, p);
        }

        let mut stream = pool.fetch_many(query);
        let mut json_rows: Vec<Value> = Vec::new();
        let mut running_bytes: usize = 0;
        let mut rows_affected: u64 = 0;

        while let Some(item) = stream.try_next().await.map_err(|e| e.to_string())? {
            match item {
                Either::Left(result) => {
                    rows_affected += result.rows_affected();
                }
                Either::Right(row) => {
                    let value = row_to_json(&row)?;
                    // Account for the row plus its `,` separator against the
                    // cap before pushing, so we never hold more than one
                    // oversized row's worth past the limit.
                    running_bytes += serde_json::to_vec(&value)
                        .map_err(|e| e.to_string())?
                        .len()
                        + 1;
                    if running_bytes > self.max_response_bytes {
                        return Err(format!(
                            "query result exceeds max_response_bytes (> {})",
                            self.max_response_bytes
                        ));
                    }
                    json_rows.push(value);
                }
            }
        }

        Ok(json!({"rows": json_rows, "rows_affected": rows_affected}))
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
                max_db_bytes: 0,
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

    // Fix #4: a write with a RETURNING clause is still a row-producing
    // statement. The old `starts_with("select")` heuristic routed it to the
    // `execute()` path, so the returned rows were silently dropped and the
    // caller only ever saw `rows_affected`. This asserts the rows come back.
    #[tokio::test]
    async fn insert_returning_yields_the_returned_rows() {
        let dir = tempfile::tempdir().unwrap();
        let h = handler(dir.path());
        let out = h
            .handle(
                "caller_a",
                "db_query",
                br#"{"sql": "insert into kv (key, value, updated_at) values ('rk', '\"v\"', 0) returning key, updated_at"}"#,
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["rows"][0]["key"], "rk", "RETURNING rows were dropped: {v}");
        assert_eq!(v["rows"][0]["updated_at"], 0);
    }

    // Fix #2: db_batch_get had no response-size cap, unlike db_query. A caller
    // storing many values (each individually under max_value_bytes) could pull
    // an unbounded blob back in one batch. With max_response_bytes = 4096 in
    // the test handler, ten ~900-byte values must trip the cap.
    #[tokio::test]
    async fn batch_get_rejects_oversized_total_response() {
        let dir = tempfile::tempdir().unwrap();
        let h = handler(dir.path());
        let big = "x".repeat(900);
        let mut keys = Vec::new();
        for i in 0..10 {
            let key = format!("k{i}");
            let params =
                serde_json::to_vec(&serde_json::json!({"key": key, "value": big})).unwrap();
            h.handle("caller_a", "db_set", &params).await.unwrap();
            keys.push(key);
        }
        let params = serde_json::to_vec(&serde_json::json!({"keys": keys})).unwrap();
        let err = h.handle("caller_a", "db_batch_get", &params).await.unwrap_err();
        assert!(err.contains("max_response_bytes"), "error was: {err}");
    }

    // Fix #3 regression guard: the streaming rewrite of handle_query must keep
    // rejecting oversized SELECT results. Ten ~900-byte rows serialized exceed
    // the 4096-byte test cap.
    #[tokio::test]
    async fn query_rejects_oversized_select_result() {
        let dir = tempfile::tempdir().unwrap();
        let h = handler(dir.path());
        let big = "x".repeat(900);
        for i in 0..10 {
            let params =
                serde_json::to_vec(&serde_json::json!({"key": format!("k{i}"), "value": big}))
                    .unwrap();
            h.handle("caller_a", "db_set", &params).await.unwrap();
        }
        let err = h
            .handle("caller_a", "db_query", br#"{"sql": "select key, value from kv"}"#)
            .await
            .unwrap_err();
        assert!(err.contains("max_response_bytes"), "error was: {err}");
    }

    // Fix #1: max_value_bytes only guards db_set; a caller can bypass it with a
    // raw INSERT. The real cap is a per-caller disk quota enforced by SQLite
    // (PRAGMA max_page_count). With a 64 KiB quota, a loop of raw-ish writes
    // must eventually be rejected rather than growing the file unbounded.
    #[tokio::test]
    async fn per_caller_disk_quota_rejects_writes_past_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let h = Handler::new(
            DbConfig {
                data_dir: dir.path().to_path_buf(),
                pool_size: 2,
                busy_timeout_ms: 1000,
                max_db_bytes: 64 * 1024,
            },
            1024,
            4096,
        );
        let big = "x".repeat(900);
        let mut hit_full = false;
        for i in 0..2000 {
            let params =
                serde_json::to_vec(&serde_json::json!({"key": format!("k{i}"), "value": big}))
                    .unwrap();
            if h.handle("caller_full", "db_set", &params).await.is_err() {
                hit_full = true;
                break;
            }
        }
        assert!(hit_full, "expected a disk-full rejection under the 64 KiB quota");
    }

    // Characterization test backing the transactions section of USAGE.md: a
    // single db_query carrying several statements (a `begin; …; commit;`
    // block) runs all of them on one pooled connection, in order, atomically.
    // This is the documented way to get atomicity, since separate db_query
    // calls may each land on a different connection from the pool. It also
    // pins the multi-statement behavior so a future sqlx bump that drops it
    // can't silently break the documented pattern.
    #[tokio::test]
    async fn multi_statement_query_runs_every_statement() {
        let dir = tempfile::tempdir().unwrap();
        let h = handler(dir.path());
        let out = h
            .handle(
                "caller_a",
                "db_query",
                br#"{"sql": "begin; insert into kv (key, value, updated_at) values ('a', '1', 0); insert into kv (key, value, updated_at) values ('b', '2', 0); commit;"}"#,
            )
            .await
            .unwrap();
        // The block committed both rows.
        let count = h
            .handle("caller_a", "db_query", br#"{"sql": "select count(*) as n from kv"}"#)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&count).unwrap();
        assert_eq!(v["rows"][0]["n"], 2, "multi-statement block did not commit both rows: {}, out was {}", v, String::from_utf8_lossy(&out));
    }
}
