//! `ai` plugin — provider-agnostic chat completion for other plugins, routed
//! through `network`'s `http_request` action rather than opening its own
//! sockets (see ROADMAP.md, "Decision: reuse `network`, don't reinvent").
//!
//! Doesn't use the SDK's `Plugin::run`/`serve` loop: `Plugin::on_message`
//! only gets `&mut self`, not `&mut VeyronClient`, and there is no way to
//! get a second client for the outbound `send_action` call into `network`
//! — the kernel rejects a second connection under the same `plugin_id`
//! (`veyron/src/plugins/registry.rs`) and rejects any traffic from an
//! unregistered connection (`veyron/src/ipc/protocol.rs`). So this plugin
//! drives its own loop, near-identical to the SDK's `serve()`, but calls
//! the `chat_completion` handler with the loop's own `&mut VeyronClient` in
//! hand. Sequential, one request at a time — same model `network` and
//! `ping-pong-rs` already use.

use ai_plugin::handler;
use veyron_sdk::proto::{
    envelope, ActionResponse, ActionStatus, Envelope, PluginManifest, Pong,
};
use veyron_sdk::{VeyronClient, VeyronError};

const PLUGIN_ID: &str = "ai";
const PLUGIN_VERSION: &str = "0.1.0";

fn manifest() -> PluginManifest {
    PluginManifest {
        permissions: vec![],
        actions: vec!["chat_completion".to_string()],
        ..Default::default()
    }
}

fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

async fn handle_action_request(
    client: &mut VeyronClient,
    req: veyron_sdk::proto::ActionRequest,
) -> Envelope {
    let reply = if req.action == "chat_completion" {
        match handler::handle_chat_completion(client, &req.params_json).await {
            Ok(data_json) => ActionResponse {
                action_id: req.action_id,
                status: ActionStatus::ActionOk as i32,
                data_json,
                error: String::new(),
            },
            Err(error) => ActionResponse {
                action_id: req.action_id,
                status: ActionStatus::ActionError as i32,
                data_json: Vec::new(),
                error,
            },
        }
    } else {
        ActionResponse {
            action_id: req.action_id,
            status: ActionStatus::ActionNotFound as i32,
            data_json: Vec::new(),
            error: format!("unknown action: {}", req.action),
        }
    };
    Envelope {
        payload: Some(envelope::Payload::ActionResponse(reply)),
        ..Default::default()
    }
}

async fn serve(mut client: VeyronClient) -> Result<(), VeyronError> {
    let jwt_token = std::env::var("VEYRON_JWT_TOKEN").unwrap_or_default();
    let ack = client
        .register_full(PLUGIN_ID, PLUGIN_VERSION, manifest(), &jwt_token)
        .await?;
    if !ack.accepted {
        return Err(VeyronError::PermissionDenied(format!(
            "registration rejected: {}",
            ack.reject_reason
        )));
    }
    println!("[{PLUGIN_ID}] registered with kernel");

    loop {
        let env = match client.recv().await {
            Ok(env) => env,
            Err(_) => break, // disconnect / EOF
        };
        match env.payload {
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
            Some(envelope::Payload::Event(event)) => {
                // ai declares no event subscriptions; ack defensively so the
                // kernel doesn't retry anything unexpectedly delivered.
                let _ = client.ack_event(&event.event_id).await;
            }
            Some(envelope::Payload::ActionRequest(req)) => {
                let resp = handle_action_request(&mut client, req).await;
                let _ = client.send("kernel", resp).await;
            }
            other => {
                println!("[{PLUGIN_ID}] unhandled message: {other:?}");
            }
        }
    }

    println!("[{PLUGIN_ID}] shutting down");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), VeyronError> {
    let socket_path = std::env::var("VEYRON_SOCKET_PATH")
        .unwrap_or_else(|_| veyron_wire::socket::default_socket_path());
    let secret = std::env::var("VEYRON_JWT_SECRET")
        .ok()
        .filter(|s| !s.is_empty());
    let client = match secret {
        Some(s) => VeyronClient::connect_with_secret(&socket_path, s.as_bytes()).await?,
        None => VeyronClient::connect(&socket_path).await?,
    };
    serve(client).await
}
