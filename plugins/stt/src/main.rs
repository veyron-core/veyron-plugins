//! `stt` plugin — speech-to-text for other plugins.
//!
//! Same shape as `tts` (see plugins/tts/src/main.rs): doesn't use the
//! SDK's `Plugin::run`/`serve` loop because `Plugin::on_message` only gets
//! `&mut self`, not `&mut VeyronClient`, and the kernel rejects a second
//! connection under the same `plugin_id`. So this plugin drives its own
//! loop, near-identical to the SDK's `serve()`, calling the handlers with
//! the loop's own `&mut VeyronClient` in hand. Sequential, one request at
//! a time — same model `network`, `ai`, `tts`, and `ping-pong-rs` already
//! use.
//!
//! The cloud provider (`openai`) routes its multipart upload through the
//! `network` plugin's `http_request` action. The local provider (`sherpa`)
//! transcribes in-process and never touches the network. See ROADMAP.md
//! for the design rationale.

use stt_plugin::handler;
use veyron_sdk::proto::{
    envelope, ActionResponse, ActionStatus, Envelope, PluginManifest, Pong,
};
use veyron_sdk::{VeyronClient, VeyronError};

const PLUGIN_ID: &str = "stt";
const PLUGIN_VERSION: &str = "0.2.0";

fn manifest() -> PluginManifest {
    PluginManifest {
        // `network`: the cloud provider invokes `network`'s gated
        // `http_request`, and T-19 requires callers of a gated action to hold
        // its permission too (matches plugin.json `permissions`; Manifest v2
        // per-action model).
        permissions: vec!["PERMISSION_NETWORK".into()],
        actions: vec!["stt_transcribe".to_string(), "stt_models".to_string()],
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
    let reply = match req.action.as_str() {
        "stt_transcribe" => {
            match handler::handle_stt_transcribe(client, &req.params_json).await {
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
        }
        "stt_models" => match handler::handle_stt_models(client, &req.params_json).await {
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
        },
        other => ActionResponse {
            action_id: req.action_id,
            status: ActionStatus::ActionNotFound as i32,
            data_json: Vec::new(),
            error: format!("unknown action: {other}"),
        },
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
                // stt declares no event subscriptions; ack defensively so
                // the kernel doesn't retry anything unexpectedly delivered.
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
