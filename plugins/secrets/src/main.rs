//! `secrets` plugin entry point.
//!
//! Connects to the kernel over `VEYRON_SOCKET_PATH` (via
//! `VeyronClient::connect_from_env`), then drives the SDK's concurrent
//! message loop — requests run concurrently, replies may come back out of
//! order, and the kernel correlates them by `action_id`.

use secrets_plugin::{handler_from_env, PLUGIN_ID};
use veyron_sdk::concurrent::serve_concurrent;
use veyron_sdk::{VeyronClient, VeyronError};

#[tokio::main]
async fn main() -> Result<(), VeyronError> {
    let handler = handler_from_env();

    let client = VeyronClient::connect_from_env().await?;
    let token = std::env::var("VEYRON_JWT_TOKEN").unwrap_or_default();
    serve_concurrent(client, &token, handler).await?;

    println!("[{PLUGIN_ID}] shutting down");
    Ok(())
}

#[cfg(test)]
mod tests {
    use secrets_plugin::handler::Handler;
    use secrets_plugin::request;
    use std::sync::Arc;
    use tokio::net::UnixStream;
    use veyron_sdk::concurrent::run_concurrent_loop;
    use veyron_sdk::proto::{envelope, ActionRequest, ActionStatus, Envelope, PluginShutdown};
    use veyron_sdk::VeyronClient;

    fn test_handler() -> Arc<Handler> {
        let dir = tempfile::tempdir().unwrap();
        Arc::new(Handler::new(
            dir.keep(),
            [7u8; 32],
            request::DEFAULT_MAX_NAME_BYTES,
            request::DEFAULT_MAX_VALUE_BYTES,
        ))
    }

    fn action_req(action_id: &str, action: &str, params: &[u8], caller: &str) -> Envelope {
        Envelope {
            payload: Some(envelope::Payload::ActionRequest(ActionRequest {
                action_id: action_id.into(),
                action: action.into(),
                params_json: params.to_vec(),
                timeout_ms: 0,
                streaming: false,
                caller_plugin_id: caller.into(),
            })),
            ..Default::default()
        }
    }

    fn shutdown_env() -> Envelope {
        Envelope {
            payload: Some(envelope::Payload::PluginShutdown(PluginShutdown {
                reason: "test done".into(),
                grace_seconds: 0,
            })),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn concurrent_set_get_responds_for_each_request() {
        let handler = test_handler();
        let (plugin_side, kernel_side) = UnixStream::pair().unwrap();
        let client = VeyronClient::from_stream(plugin_side, None);
        let mut kernel = VeyronClient::from_stream(kernel_side, None);

        let loop_task = tokio::spawn(run_concurrent_loop(client, handler));

        // Fire N concurrent secret_set requests back-to-back; the loop must
        // answer every one (deadlock/ordering regression).
        const N: usize = 20;
        for i in 0..N {
            let params =
                serde_json::json!({ "name": format!("key{i}"), "value": format!("val{i}") });
            kernel
                .send(
                    "secrets",
                    action_req(
                        &format!("set-{i}"),
                        "secret_set",
                        &serde_json::to_vec(&params).unwrap(),
                        "test-caller",
                    ),
                )
                .await
                .unwrap();
        }

        let mut ok = 0usize;
        for _ in 0..N {
            let env = tokio::time::timeout(std::time::Duration::from_secs(5), kernel.recv())
                .await
                .expect("timed out waiting for response — loop likely deadlocked")
                .unwrap();
            match env.payload {
                Some(envelope::Payload::ActionResponse(resp)) => {
                    assert_eq!(resp.status, ActionStatus::ActionOk as i32);
                    assert_eq!(resp.data_json, br#"{"stored":true}"#);
                    ok += 1;
                }
                other => panic!("unexpected payload: {other:?}"),
            }
        }
        assert_eq!(ok, N);

        kernel.send("secrets", shutdown_env()).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), loop_task)
            .await
            .expect("run_loop did not exit after PluginShutdown")
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn error_becomes_action_error_envelope() {
        let handler = test_handler();
        let (plugin_side, kernel_side) = UnixStream::pair().unwrap();
        let client = VeyronClient::from_stream(plugin_side, None);
        let mut kernel = VeyronClient::from_stream(kernel_side, None);

        let loop_task = tokio::spawn(run_concurrent_loop(client, handler));

        // Empty caller id must produce an ACTION_ERROR, not a dropped reply.
        kernel
            .send("secrets", action_req("bad-1", "secret_list", b"", ""))
            .await
            .unwrap();

        let env = tokio::time::timeout(std::time::Duration::from_secs(5), kernel.recv())
            .await
            .expect("timed out waiting for error response")
            .unwrap();
        match env.payload {
            Some(envelope::Payload::ActionResponse(resp)) => {
                assert_eq!(resp.status, ActionStatus::ActionError as i32);
                assert!(
                    resp.error.contains("missing caller_plugin_id"),
                    "error was: {:?}",
                    resp.error
                );
            }
            other => panic!("unexpected payload: {other:?}"),
        }

        kernel.send("secrets", shutdown_env()).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), loop_task)
            .await
            .expect("run_loop did not exit after PluginShutdown")
            .unwrap()
            .unwrap();
    }
}
