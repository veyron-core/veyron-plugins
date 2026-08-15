use crate::db::{self, DbConfig};
use crate::request::{parse_request, SyncRequest};
use serde_json::{json, Value};
use sqlx::{SqliteConnection, SqlitePool};

/// A single state mutation to be published as a `sync.delta` event.
///
/// `value` is `Some` for `op: "set"` and `None` for `op: "del"` (serialized
/// as JSON `null`, per the sync-client contract). `version` is the store
/// version *after* this mutation and `updated_at` the mutation's unix-millis
/// timestamp.
#[derive(Debug)]
pub struct Delta {
    pub op: &'static str,
    pub key: String,
    pub value: Option<Value>,
    pub version: i64,
    pub updated_at: i64,
}

impl Delta {
    /// Serialize this delta into the `payload_json` bytes of an
    /// `EventPublish`. Built with serde_json (never `format!`) so the JSON
    /// is guaranteed well-formed even for keys/values containing quotes or
    /// control characters.
    pub fn payload_json(&self) -> Vec<u8> {
        let obj = json!({
            "op": self.op,
            "key": self.key,
            "value": self.value,
            "version": self.version,
            "updated_at": self.updated_at,
        });
        // Serializing a `serde_json::Value` cannot fail; the fallback only
        // exists to keep a best-effort event publish from ever panicking a
        // handler task on the impossible error.
        serde_json::to_vec(&obj).unwrap_or_default()
    }
}

pub struct SyncHandler {
    pool: SqlitePool,
    max_value_bytes: usize,
    max_response_bytes: usize,
    heartbeat_ttl_secs: u64,
}

fn unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl SyncHandler {
    /// Open the backing pool and build the handler. Unlike `database` (which
    /// lazily opens per-caller files), sync has exactly one shared store, so
    /// the pool is opened eagerly at startup.
    pub async fn open(
        config: DbConfig,
        max_value_bytes: usize,
        max_response_bytes: usize,
        heartbeat_ttl_secs: u64,
    ) -> Result<Self, String> {
        let pool = db::open_pool(&config).await?;
        Ok(Self {
            pool,
            max_value_bytes,
            max_response_bytes,
            heartbeat_ttl_secs,
        })
    }

    /// Dispatch one action, returning the serialized response JSON plus the
    /// list of delta payloads to publish (response is always sent first by
    /// the caller; deltas follow in version order).
    pub async fn handle(
        &self,
        caller_plugin_id: &str,
        action: &str,
        params_json: &[u8],
    ) -> Result<(Vec<u8>, Vec<Delta>), String> {
        if caller_plugin_id.is_empty() {
            return Err(
                "missing caller_plugin_id (rejected before touching any database)".to_string(),
            );
        }
        let req = parse_request(action, params_json)?;

        let (result, deltas) = match req {
            SyncRequest::Snapshot => self.handle_snapshot().await?,
            SyncRequest::Get { key } => (self.handle_get(&key).await?, Vec::new()),
            SyncRequest::Set { key, value } => self.handle_set(&key, &value).await?,
            SyncRequest::Del { key } => self.handle_del(&key).await?,
        };

        let response_json =
            serde_json::to_vec(&result).map_err(|e| format!("failed to encode response: {e}"))?;
        Ok((response_json, deltas))
    }

    async fn handle_get(&self, key: &str) -> Result<Value, String> {
        let row: Option<(String,)> = sqlx::query_as("select value from state where key = ?1")
            .bind(key)
            .fetch_optional(&self.pool)
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

    async fn handle_set(&self, key: &str, value: &Value) -> Result<(Value, Vec<Delta>), String> {
        let raw =
            serde_json::to_string(value).map_err(|e| format!("failed to encode value: {e}"))?;
        if raw.len() > self.max_value_bytes {
            return Err(format!(
                "value exceeds max_value_bytes ({} > {})",
                raw.len(),
                self.max_value_bytes
            ));
        }

        let now_ms = unix_millis();
        let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;
        // Prune before the mutation so prune deltas carry lower versions
        // than the set's own delta (versions ascending).
        let mut deltas = prune_heartbeats(&mut tx, self.heartbeat_ttl_secs).await?;

        sqlx::query(
            "insert into state (key, value, updated_at) values (?1, ?2, ?3) \
             on conflict(key) do update set value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(key)
        .bind(&raw)
        .bind(now_ms)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        let version = bump_version(&mut tx).await?;

        tx.commit().await.map_err(|e| e.to_string())?;

        deltas.push(Delta {
            op: "set",
            key: key.to_string(),
            value: Some(value.clone()),
            version,
            updated_at: now_ms,
        });
        Ok((json!({"ok": true, "version": version}), deltas))
    }

    async fn handle_del(&self, key: &str) -> Result<(Value, Vec<Delta>), String> {
        let now_ms = unix_millis();
        let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;

        let result = sqlx::query("delete from state where key = ?1")
            .bind(key)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        let deleted = result.rows_affected() > 0;

        // A del that removed nothing is not a mutation: leave the version
        // untouched and publish no delta, so the client's version stays in
        // step with a state that did not change.
        let (version, deltas) = if deleted {
            let version = bump_version(&mut tx).await?;
            (
                version,
                vec![Delta {
                    op: "del",
                    key: key.to_string(),
                    value: None,
                    version,
                    updated_at: now_ms,
                }],
            )
        } else {
            (read_version(&mut tx).await?, Vec::new())
        };

        tx.commit().await.map_err(|e| e.to_string())?;
        Ok((json!({"deleted": deleted, "version": version}), deltas))
    }

    async fn handle_snapshot(&self) -> Result<(Value, Vec<Delta>), String> {
        let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;
        // Snapshot is the other prune trigger (the client pulls it on
        // reconnect, so it must observe expired heartbeats already gone).
        let deltas = prune_heartbeats(&mut tx, self.heartbeat_ttl_secs).await?;
        let version = read_version(&mut tx).await?;
        let rows: Vec<(String, String)> = sqlx::query_as("select key, value from state")
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        tx.commit().await.map_err(|e| e.to_string())?;

        let mut state = serde_json::Map::new();
        for (key, raw) in rows {
            let value: Value = serde_json::from_str(&raw)
                .map_err(|e| format!("corrupt stored value for key {key:?}: {e}"))?;
            state.insert(key, value);
        }
        let result = json!({"version": version, "state": Value::Object(state)});

        // Reject (never truncate) a snapshot that would blow past the
        // response cap, matching the other size guards.
        let size = serde_json::to_vec(&result)
            .map_err(|e| format!("failed to encode response: {e}"))?
            .len();
        if size > self.max_response_bytes {
            return Err(format!(
                "snapshot exceeds max_response_bytes ({} > {})",
                size, self.max_response_bytes
            ));
        }

        Ok((result, deltas))
    }
}

/// Read the persisted version counter without bumping it.
async fn read_version(conn: &mut SqliteConnection) -> Result<i64, String> {
    let value: String = sqlx::query_scalar("select value from meta where key = 'version'")
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| e.to_string())?;
    value
        .parse::<i64>()
        .map_err(|e| format!("corrupt version meta value {value:?}: {e}"))
}

/// Bump the version counter by one and return the new value. The read of the
/// old value and the write of the new one happen in a single `UPDATE`, so
/// concurrent transactions serialize on the write lock instead of racing a
/// read-then-write (which would lose increments).
async fn bump_version(conn: &mut SqliteConnection) -> Result<i64, String> {
    sqlx::query("update meta set value = cast(value as integer) + 1 where key = 'version'")
        .execute(&mut *conn)
        .await
        .map_err(|e| e.to_string())?;
    read_version(conn).await
}

/// Lazily delete stale heartbeat rows, bumping the version (and emitting a
/// delta) per pruned row. Runs only on `sync_set` and `sync_get_snapshot`;
/// `heartbeat_ttl_secs == 0` disables it. Returns deltas in ascending
/// version order (rows ordered by key so the ordering is deterministic).
async fn prune_heartbeats(
    conn: &mut SqliteConnection,
    heartbeat_ttl_secs: u64,
) -> Result<Vec<Delta>, String> {
    if heartbeat_ttl_secs == 0 {
        return Ok(Vec::new());
    }
    let now_ms = unix_millis();
    let cutoff = now_ms - (heartbeat_ttl_secs as i64) * 1000;

    let rows: Vec<(String,)> = sqlx::query_as(
        "select key from state \
         where key like 'heartbeat.%' and updated_at < ?1 order by key",
    )
    .bind(cutoff)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| e.to_string())?;

    let mut deltas = Vec::with_capacity(rows.len());
    for (key,) in rows {
        sqlx::query("delete from state where key = ?1")
            .bind(&key)
            .execute(&mut *conn)
            .await
            .map_err(|e| e.to_string())?;
        let version = bump_version(conn).await?;
        deltas.push(Delta {
            op: "del",
            key,
            value: None,
            version,
            // The deletion is the mutation; its timestamp is the prune time,
            // not the stale row's original `updated_at` (that only chose the
            // row for eviction).
            updated_at: now_ms,
        });
    }
    Ok(deltas)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbConfig;

    async fn handler(dir: &std::path::Path) -> SyncHandler {
        handler_with_ttl(dir, 0).await
    }

    async fn handler_with_ttl(dir: &std::path::Path, ttl_secs: u64) -> SyncHandler {
        SyncHandler::open(
            DbConfig {
                data_dir: dir.to_path_buf(),
                pool_size: 2,
                busy_timeout_ms: 1000,
                max_db_bytes: 0,
            },
            1024,
            4096,
            ttl_secs,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn set_then_get_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let h = handler(dir.path()).await;

        let (out, deltas) = h
            .handle(
                "caller_a",
                "sync_set",
                br#"{"key": "foo", "value": {"n": 1}}"#,
            )
            .await
            .unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v, serde_json::json!({"ok": true, "version": 1}));
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].op, "set");
        assert_eq!(deltas[0].version, 1);

        let (out, deltas) = h
            .handle("caller_a", "sync_get", br#"{"key": "foo"}"#)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&out).unwrap(),
            serde_json::json!({"found": true, "value": {"n": 1}})
        );
        assert!(deltas.is_empty());
    }

    #[tokio::test]
    async fn get_missing_key_returns_found_false() {
        let dir = tempfile::tempdir().unwrap();
        let h = handler(dir.path()).await;
        let (out, _) = h
            .handle("caller_a", "sync_get", br#"{"key": "nope"}"#)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&out).unwrap(),
            serde_json::json!({"found": false, "value": null})
        );
    }

    #[tokio::test]
    async fn version_monotonically_increases() {
        let dir = tempfile::tempdir().unwrap();
        let h = handler(dir.path()).await;
        let (out1, _) = h
            .handle("caller_a", "sync_set", br#"{"key": "a", "value": 1}"#)
            .await
            .unwrap();
        let (out2, _) = h
            .handle("caller_a", "sync_set", br#"{"key": "b", "value": 2}"#)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&out1).unwrap()["version"],
            1
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&out2).unwrap()["version"],
            2
        );
    }

    #[tokio::test]
    async fn del_reports_whether_a_row_was_removed() {
        let dir = tempfile::tempdir().unwrap();
        let h = handler(dir.path()).await;
        h.handle("caller_a", "sync_set", br#"{"key": "foo", "value": 1}"#)
            .await
            .unwrap();

        let (out1, d1) = h
            .handle("caller_a", "sync_del", br#"{"key": "foo"}"#)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&out1).unwrap(),
            serde_json::json!({"deleted": true, "version": 2})
        );
        assert_eq!(d1.len(), 1);
        assert_eq!(d1[0].op, "del");
        assert_eq!(d1[0].value, None);

        // Second del of the same key removed nothing: version unchanged, no
        // delta.
        let (out2, d2) = h
            .handle("caller_a", "sync_del", br#"{"key": "foo"}"#)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&out2).unwrap(),
            serde_json::json!({"deleted": false, "version": 2})
        );
        assert!(d2.is_empty());
    }

    #[tokio::test]
    async fn snapshot_returns_version_and_state() {
        let dir = tempfile::tempdir().unwrap();
        let h = handler(dir.path()).await;
        h.handle("caller_a", "sync_set", br#"{"key": "a", "value": 1}"#)
            .await
            .unwrap();
        h.handle("caller_a", "sync_set", br#"{"key": "b", "value": "x"}"#)
            .await
            .unwrap();

        let (out, deltas) = h
            .handle("caller_a", "sync_get_snapshot", b"{}")
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&out).unwrap(),
            serde_json::json!({"version": 2, "state": {"a": 1, "b": "x"}})
        );
        assert!(deltas.is_empty());
    }

    #[tokio::test]
    async fn set_rejects_oversized_value() {
        let dir = tempfile::tempdir().unwrap();
        let h = handler(dir.path()).await;
        let big_value = serde_json::json!("x".repeat(2000));
        let params =
            serde_json::to_vec(&serde_json::json!({"key": "foo", "value": big_value})).unwrap();
        let err = h.handle("caller_a", "sync_set", &params).await.unwrap_err();
        assert!(err.contains("max_value_bytes"), "error was: {err}");
    }

    #[tokio::test]
    async fn snapshot_rejects_oversized_response() {
        let dir = tempfile::tempdir().unwrap();
        let h = handler(dir.path()).await;
        let big = "x".repeat(900);
        for i in 0..10 {
            let params =
                serde_json::to_vec(&serde_json::json!({"key": format!("k{i}"), "value": big}))
                    .unwrap();
            h.handle("caller_a", "sync_set", &params).await.unwrap();
        }
        let err = h
            .handle("caller_a", "sync_get_snapshot", b"{}")
            .await
            .unwrap_err();
        assert!(err.contains("max_response_bytes"), "error was: {err}");
    }

    #[tokio::test]
    async fn rejects_missing_caller_id() {
        let dir = tempfile::tempdir().unwrap();
        let h = handler(dir.path()).await;
        let err = h
            .handle("", "sync_get", br#"{"key": "foo"}"#)
            .await
            .unwrap_err();
        assert!(err.contains("caller_plugin_id"), "error was: {err}");
    }

    #[tokio::test]
    async fn rejects_unknown_action() {
        let dir = tempfile::tempdir().unwrap();
        let h = handler(dir.path()).await;
        let err = h
            .handle("caller_a", "sync_frobnicate", b"{}")
            .await
            .unwrap_err();
        assert!(err.contains("sync_frobnicate"), "error was: {err}");
    }

    #[tokio::test]
    async fn heartbeat_prune_on_set_emits_del_deltas_before_the_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let h = handler_with_ttl(dir.path(), 300).await;

        let now_ms = unix_millis();
        let stale = now_ms - 400_000; // 400s ago, past the 300s ttl
        sqlx::query(
            "insert into state (key, value, updated_at) values ('heartbeat.dev1', '1', ?1)",
        )
        .bind(stale)
        .execute(&h.pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into state (key, value, updated_at) values ('heartbeat.dev2', '2', ?1)",
        )
        .bind(stale)
        .execute(&h.pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into state (key, value, updated_at) values ('heartbeat.dev3', '3', ?1)",
        )
        .bind(now_ms)
        .execute(&h.pool)
        .await
        .unwrap();

        let (out, deltas) = h
            .handle("caller_a", "sync_set", br#"{"key": "k", "value": 1}"#)
            .await
            .unwrap();

        // two prune delts, then the set delta.
        assert_eq!(deltas.len(), 3, "deltas: {deltas:?}");
        assert_eq!(deltas[0].op, "del");
        assert_eq!(deltas[0].key, "heartbeat.dev1");
        assert_eq!(deltas[1].op, "del");
        assert_eq!(deltas[1].key, "heartbeat.dev2");
        assert_eq!(deltas[2].op, "set");
        assert_eq!(deltas[2].key, "k");
        // versions strictly ascending.
        assert!(deltas[0].version < deltas[1].version);
        assert!(deltas[1].version < deltas[2].version);
        // the set response reports the post-prune, post-set version.
        assert_eq!(serde_json::from_slice::<Value>(&out).unwrap()["version"], 3);

        // the fresh heartbeat survived.
        let (get_out, _) = h
            .handle("caller_a", "sync_get", br#"{"key": "heartbeat.dev3"}"#)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&get_out).unwrap()["found"],
            true
        );
    }

    #[tokio::test]
    async fn heartbeat_prune_on_snapshot_removes_stale_rows() {
        let dir = tempfile::tempdir().unwrap();
        let h = handler_with_ttl(dir.path(), 300).await;

        let stale = unix_millis() - 400_000;
        sqlx::query("insert into state (key, value, updated_at) values ('heartbeat.old', '1', ?1)")
            .bind(stale)
            .execute(&h.pool)
            .await
            .unwrap();

        let (out, deltas) = h
            .handle("caller_a", "sync_get_snapshot", b"{}")
            .await
            .unwrap();
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].op, "del");
        assert_eq!(deltas[0].key, "heartbeat.old");
        // state is empty after the prune.
        assert_eq!(
            serde_json::from_slice::<Value>(&out).unwrap(),
            serde_json::json!({"version": 1, "state": {}})
        );
    }

    #[tokio::test]
    async fn heartbeat_ttl_zero_disables_prune() {
        let dir = tempfile::tempdir().unwrap();
        let h = handler_with_ttl(dir.path(), 0).await;

        let stale = unix_millis() - 400_000;
        sqlx::query("insert into state (key, value, updated_at) values ('heartbeat.old', '1', ?1)")
            .bind(stale)
            .execute(&h.pool)
            .await
            .unwrap();

        let (_, deltas) = h
            .handle("caller_a", "sync_set", br#"{"key": "k", "value": 1}"#)
            .await
            .unwrap();
        assert_eq!(deltas.len(), 1, "only the set delta, no prune: {deltas:?}");
        assert_eq!(deltas[0].op, "set");
    }

    #[tokio::test]
    async fn version_persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let h = handler(dir.path()).await;
        h.handle("caller_a", "sync_set", br#"{"key": "a", "value": 1}"#)
            .await
            .unwrap();

        // Reopen the same data_dir: the version counter must resume at 1,
        // not reset to 0.
        let h2 = handler(dir.path()).await;
        let (out, _) = h2
            .handle("caller_a", "sync_set", br#"{"key": "b", "value": 2}"#)
            .await
            .unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&out).unwrap()["version"], 2);
    }
}
