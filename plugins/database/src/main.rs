//! `database` plugin — per-caller-namespaced KV + raw SQL storage, gated by
//! `PERMISSION_STORAGE`. See
//! docs/superpowers/specs/2026-07-15-database-plugin-design.md for the design.
//!
//! Unlike `ai`/`network` (sequential `Plugin::run`), this plugin expects
//! higher call volume (roadmap: "database will be called far more often and
//! needs real concurrency") so it hand-rolls a concurrent loop instead of
//! using the SDK's sequential `serve()`: one reader pulls frames and spawns
//! a handler task per `ActionRequest`; handlers run concurrently and each
//! sends its own response back through a shared, mutex-guarded client.
//! Out-of-order replies are fine — the kernel matches on `action_id`.

use database_plugin::db::DbConfig;
use database_plugin::handler::Handler;
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;
use veyron_sdk::proto::{envelope, ActionResponse, ActionStatus, Envelope, Pong};
use veyron_sdk::proto::PluginManifest;
use veyron_sdk::{VeyronClient, VeyronError};

fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn load_config() -> DbConfig {
    let data_dir = std::env::var("DATABASE_PLUGIN_DATA_DIR")
        .unwrap_or_else(|_| panic!("DATABASE_PLUGIN_DATA_DIR must be set (see config.example.yaml's data_dir)"));
    let pool_size = std::env::var("DATABASE_PLUGIN_POOL_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);
    let busy_timeout_ms = std::env::var("DATABASE_PLUGIN_BUSY_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5000);
    DbConfig {
        data_dir: data_dir.into(),
        pool_size,
        busy_timeout_ms,
    }
}

fn manifest() -> PluginManifest {
    PluginManifest {
        permissions: vec!["PERMISSION_STORAGE".into()],
        actions: vec![
            "db_get".into(),
            "db_set".into(),
            "db_delete".into(),
            "db_batch_get".into(),
            "db_query".into(),
        ],
        ..Default::default()
    }
}

async fn respond(client: &AsyncMutex<VeyronClient>, action_id: String, result: Result<Vec<u8>, String>) {
    let response = match result {
        Ok(data_json) => ActionResponse {
            action_id,
            status: ActionStatus::ActionOk as i32,
            data_json,
            error: String::new(),
        },
        Err(error) => ActionResponse {
            action_id,
            status: ActionStatus::ActionError as i32,
            data_json: Vec::new(),
            error,
        },
    };
    let envelope = Envelope {
        payload: Some(envelope::Payload::ActionResponse(response)),
        ..Default::default()
    };
    let mut c = client.lock().await;
    let _ = c.send("kernel", envelope).await;
}

#[tokio::main]
async fn main() -> Result<(), VeyronError> {
    let max_value_bytes = std::env::var("DATABASE_PLUGIN_MAX_VALUE_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1024 * 1024);
    let max_response_bytes = std::env::var("DATABASE_PLUGIN_MAX_RESPONSE_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4 * 1024 * 1024);

    let handler = Arc::new(Handler::new(load_config(), max_value_bytes, max_response_bytes));

    let mut client = VeyronClient::connect_from_env().await?;
    let token = std::env::var("VEYRON_JWT_TOKEN").unwrap_or_default();
    let ack = client
        .register_full("database", "0.1.0", manifest(), &token)
        .await?;
    if !ack.accepted {
        return Err(VeyronError::PermissionDenied(format!(
            "registration rejected: {}",
            ack.reject_reason
        )));
    }
    println!("[database] registered with kernel");

    let client = Arc::new(AsyncMutex::new(client));

    loop {
        let envelope = {
            let mut c = client.lock().await;
            match c.recv().await {
                Ok(env) => env,
                Err(_) => break, // disconnect / EOF
            }
        };

        match envelope.payload {
            Some(envelope::Payload::Ping(ping)) => {
                let pong = Envelope {
                    payload: Some(envelope::Payload::Pong(Pong {
                        original_timestamp: ping.timestamp,
                        server_timestamp: unix_millis(),
                    })),
                    ..Default::default()
                };
                let mut c = client.lock().await;
                let _ = c.send("kernel", pong).await;
            }
            Some(envelope::Payload::PluginShutdown(_)) => break,
            Some(envelope::Payload::ActionRequest(req)) => {
                let handler = handler.clone();
                let client = client.clone();
                tokio::spawn(async move {
                    let result = handler
                        .handle(&req.caller_plugin_id, &req.action, &req.params_json)
                        .await;
                    respond(&client, req.action_id, result).await;
                });
            }
            other => {
                println!("[database] unhandled message: {other:?}");
            }
        }
    }

    println!("[database] shutting down");
    Ok(())
}

#[cfg(test)]
mod tests {
    use database_plugin::db::DbConfig;
    use database_plugin::handler::Handler;
    use std::sync::Arc;

    #[tokio::test]
    async fn concurrent_sets_and_gets_across_callers_do_not_cross_talk() {
        let dir = tempfile::tempdir().unwrap();
        let handler = Arc::new(Handler::new(
            DbConfig {
                data_dir: dir.path().to_path_buf(),
                pool_size: 4,
                busy_timeout_ms: 2000,
            },
            1024 * 1024,
            4 * 1024 * 1024,
        ));

        let mut tasks = Vec::new();
        for caller_n in 0..8 {
            let h = handler.clone();
            tasks.push(tokio::spawn(async move {
                let caller_id = format!("caller_{caller_n}");
                for i in 0..20 {
                    let params = serde_json::to_vec(&serde_json::json!({
                        "key": format!("k{i}"),
                        "value": caller_n,
                    }))
                    .unwrap();
                    h.handle(&caller_id, "db_set", &params).await.unwrap();
                }
                for i in 0..20 {
                    let params = serde_json::to_vec(&serde_json::json!({"key": format!("k{i}")})).unwrap();
                    let out = h.handle(&caller_id, "db_get", &params).await.unwrap();
                    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
                    assert_eq!(v["value"], caller_n, "cross-talk detected for {caller_id}");
                }
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
    }
}
