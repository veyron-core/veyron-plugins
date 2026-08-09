//! `database` plugin — per-caller-namespaced KV + raw SQL storage, gated by
//! `PERMISSION_STORAGE`. See
//! docs/superpowers/specs/2026-07-15-database-plugin-design.md for the design.
//!
//! Unlike `ai`/`network` (sequential `Plugin::run`), this plugin expects
//! higher call volume (roadmap: "database will be called far more often and
//! needs real concurrency") so it hand-rolls a concurrent loop instead of
//! using the SDK's sequential `serve()`: one task owns the `VeyronClient`
//! exclusively and `tokio::select!`s between `client.recv()` (inbound frames
//! from the kernel) and an `mpsc::Receiver<Envelope>` that spawned handler
//! tasks push completed response envelopes into. The client is never shared
//! behind a lock, so there is no way for a blocking `recv()` to hold a mutex
//! that a handler needs in order to reply — see the module-level comment on
//! `run_loop` for the deadlock this replaced and why the new design can't
//! reproduce it. Handler tasks are double-spawned (inner task does the real
//! work, outer task awaits its `JoinHandle`) so a panic inside
//! `Handler::handle` is converted into an `ACTION_ERROR` response instead of
//! silently dropping the reply. Out-of-order replies are fine — the kernel
//! matches on `action_id`.

use database_plugin::db::DbConfig;
use database_plugin::handler::Handler;
use std::sync::Arc;
use tokio::sync::mpsc;
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
    let max_db_bytes = std::env::var("DATABASE_PLUGIN_MAX_DB_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(256 * 1024 * 1024);
    DbConfig {
        data_dir: data_dir.into(),
        pool_size,
        busy_timeout_ms,
        max_db_bytes,
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

/// Build the response envelope for a completed (or failed) action.
fn response_envelope(action_id: String, result: Result<Vec<u8>, String>) -> Envelope {
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
    Envelope {
        payload: Some(envelope::Payload::ActionResponse(response)),
        ..Default::default()
    }
}

/// Spawn a handler task for `req` that always produces exactly one response
/// envelope on `tx`, even if `Handler::handle` panics.
///
/// This double-spawns: the inner `tokio::spawn` runs the actual handler and
/// its `JoinHandle` is awaited by the outer task. A panic inside the inner
/// task is caught by Tokio and surfaced as `Err(JoinError)` to the outer
/// task rather than unwinding it, so the outer task can always reach the
/// `tx.send(...)` at the end — a panicking handler becomes an `ACTION_ERROR`
/// response instead of a silently dropped reply.
fn spawn_handler(
    handler: Arc<Handler>,
    tx: mpsc::Sender<Envelope>,
    action_id: String,
    caller_plugin_id: String,
    action: String,
    params_json: Vec<u8>,
) {
    tokio::spawn(async move {
        let inner_handler = handler.clone();
        let join = tokio::spawn(async move {
            inner_handler
                .handle(&caller_plugin_id, &action, &params_json)
                .await
        });
        let result = match join.await {
            Ok(result) => result,
            Err(join_err) => Err(format!("handler panicked: {join_err}")),
        };
        let envelope = response_envelope(action_id, result);
        // Receiver side only goes away when the main loop exits, at which
        // point dropping the reply is the correct behavior anyway.
        let _ = tx.send(envelope).await;
    });
}

/// Drive the plugin's message loop to completion (disconnect, EOF, or an
/// explicit `PluginShutdown`).
///
/// `client` is owned exclusively by this function — never shared behind a
/// lock. Each loop iteration is a single `tokio::select!` between two
/// futures:
///
/// - `client.recv()`: the next inbound frame from the kernel. This is the
///   only place `client` is touched for reading, and nothing else needs the
///   client while this future is pending.
/// - `rx.recv()`: the next completed response envelope pushed by a spawned
///   handler task (see `spawn_handler`). Handler tasks never touch `client`
///   directly — they only need a clone of `tx`, which is a plain `mpsc`
///   sender with no relationship to the client's lock (there is no lock).
///
/// Because `client` is never wrapped in a `Mutex`, a handler that finishes
/// while this function is parked inside `client.recv().await` does not need
/// to acquire anything the loop task holds: it just calls `tx.send(...)`,
/// which only needs the channel's internal queue lock (a different,
/// short-lived, always-available lock unrelated to `client`). That send
/// completing wakes the `select!`, which then polls the `rx.recv()` branch,
/// picks up the envelope, and calls `client.send(...)` on its next
/// iteration — no task is ever waiting on a resource held by a task that is
/// itself waiting on it. This is what makes the old
/// `Arc<Mutex<VeyronClient>>` deadlock (a handler blocked on `client.lock()`
/// while the loop task held that same lock parked inside a possibly
/// long-pending `recv()`) impossible in this design: the two futures
/// `client.recv()` and `rx.recv()` never contend for the same lock, because
/// there isn't one.
async fn run_loop(mut client: VeyronClient, handler: Arc<Handler>) -> Result<(), VeyronError> {
    let (tx, mut rx) = mpsc::channel::<Envelope>(256);

    loop {
        tokio::select! {
            envelope = client.recv() => {
                let envelope = match envelope {
                    Ok(env) => env,
                    Err(_) => break, // disconnect / EOF
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
                        let _ = client.send("kernel", pong).await;
                    }
                    Some(envelope::Payload::PluginShutdown(_)) => break,
                    Some(envelope::Payload::ActionRequest(req)) => {
                        spawn_handler(
                            handler.clone(),
                            tx.clone(),
                            req.action_id,
                            req.caller_plugin_id,
                            req.action,
                            req.params_json,
                        );
                    }
                    other => {
                        println!("[database] unhandled message: {other:?}");
                    }
                }
            }
            Some(response_envelope) = rx.recv() => {
                let _ = client.send("kernel", response_envelope).await;
            }
        }
    }

    Ok(())
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

    run_loop(client, handler).await?;

    println!("[database] shutting down");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::run_loop;
    use database_plugin::db::DbConfig;
    use database_plugin::handler::Handler;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::net::UnixStream;
    use veyron_sdk::proto::{envelope, ActionRequest, ActionStatus, Envelope, PluginShutdown};
    use veyron_sdk::VeyronClient;

    /// Regression test for the deadlock this task fixes: drives `run_loop`
    /// over a real `VeyronClient` (no live kernel needed — `UnixStream::pair`
    /// plus `VeyronClient::from_stream` is the SDK's own test pattern, see
    /// `veyron/sdk/rust/tests/protocol.rs`).
    ///
    /// The fake "kernel" fires a batch of `ActionRequest`s back-to-back and
    /// then does *not* send anything else until it has read back every
    /// response. Under the old `Arc<Mutex<VeyronClient>>` design this would
    /// deadlock: the loop task's `recv()` holds the lock while waiting for a
    /// frame that (by construction, in this test) never arrives until the
    /// responses are drained — but draining them is exactly what's blocked
    /// on that lock. The `tokio::time::timeout` wrapper turns "would have
    /// hung forever" into a clean test failure instead of an actual CI hang.
    #[tokio::test]
    async fn concurrent_requests_get_responses_without_deadlocking() {
        let dir = tempfile::tempdir().unwrap();
        let handler = Arc::new(Handler::new(
            DbConfig {
                data_dir: dir.path().to_path_buf(),
                pool_size: 4,
                busy_timeout_ms: 2000,
                max_db_bytes: 0,
            },
            1024 * 1024,
            4 * 1024 * 1024,
        ));

        let (plugin_side, kernel_side) = UnixStream::pair().unwrap();
        let client = VeyronClient::from_stream(plugin_side, None);
        let mut kernel = VeyronClient::from_stream(kernel_side, None);

        let loop_task = tokio::spawn(run_loop(client, handler));

        const N: usize = 20;
        for i in 0..N {
            let req = Envelope {
                payload: Some(envelope::Payload::ActionRequest(ActionRequest {
                    action_id: format!("action-{i}"),
                    action: "db_set".into(),
                    params_json: serde_json::to_vec(&serde_json::json!({
                        "key": format!("k{i}"),
                        "value": i,
                    }))
                    .unwrap(),
                    timeout_ms: 0,
                    streaming: false,
                    caller_plugin_id: "caller_x".into(),
                })),
                ..Default::default()
            };
            kernel.send("database", req).await.unwrap();
        }

        let mut seen = std::collections::HashSet::new();
        for _ in 0..N {
            let env = tokio::time::timeout(Duration::from_secs(5), kernel.recv())
                .await
                .expect("timed out waiting for response — loop likely deadlocked")
                .unwrap();
            match env.payload {
                Some(envelope::Payload::ActionResponse(resp)) => {
                    assert_eq!(resp.status, ActionStatus::ActionOk as i32);
                    assert!(seen.insert(resp.action_id), "duplicate response");
                }
                other => panic!("unexpected payload: {other:?}"),
            }
        }

        // Ask the loop to exit cleanly and make sure it does.
        let shutdown = Envelope {
            payload: Some(envelope::Payload::PluginShutdown(PluginShutdown {
                reason: "test done".into(),
                grace_seconds: 0,
            })),
            ..Default::default()
        };
        kernel.send("database", shutdown).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), loop_task)
            .await
            .expect("run_loop did not exit after PluginShutdown")
            .unwrap()
            .unwrap();
    }

    /// Direct test of the panic-isolation mechanism `spawn_handler` relies
    /// on: a panic inside a `tokio::spawn`ed task does not unwind the task
    /// that `.await`s its `JoinHandle` — it surfaces as `Err(JoinError)`
    /// with `is_panic() == true`. This is what lets `spawn_handler` turn a
    /// panicking `Handler::handle` into an `ACTION_ERROR` response instead
    /// of dropping the reply on the floor.
    #[tokio::test]
    async fn spawned_task_panic_is_observable_as_a_join_error() {
        let join = tokio::spawn(async { panic!("boom") });
        let result: Result<(), _> = join.await;
        let err = result.expect_err("panicking task should yield Err(JoinError)");
        assert!(err.is_panic(), "expected a panic JoinError, got: {err:?}");
    }

    #[tokio::test]
    async fn concurrent_sets_and_gets_across_callers_do_not_cross_talk() {
        let dir = tempfile::tempdir().unwrap();
        let handler = Arc::new(Handler::new(
            DbConfig {
                data_dir: dir.path().to_path_buf(),
                pool_size: 4,
                busy_timeout_ms: 2000,
                max_db_bytes: 0,
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
