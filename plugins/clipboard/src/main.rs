//! `clipboard` plugin — read/write the system clipboard for vynkor plugins.
//!
//! v1 is text-only and local: it spawns host clipboard binaries directly
//! with argv (never a shell) — `wl-paste`/`wl-copy` on Wayland, `xclip`/`xsel`
//! on X11 — so it needs no network. Declares `PERMISSION_CLIPBOARD`.
//! Same thin shape as `media`: implements the SDK's `Plugin` trait
//! (sequential, one request at a time); all logic lives in the `handler` /
//! `providers` modules. See ROADMAP.md for scope and non-goals.

mod handler;
mod providers;

use serde_json::Value;
use veyron_sdk::proto::{
    envelope, ActionRequest, ActionResponse, ActionStatus, Envelope, PluginManifest,
};
use veyron_sdk::{Plugin, VeyronClient, VeyronError};

const PLUGIN_ID: &str = "clipboard";
const PLUGIN_VERSION: &str = "0.1.0";

struct ClipboardPlugin;

impl Plugin for ClipboardPlugin {
    fn id(&self) -> &str {
        PLUGIN_ID
    }

    fn version(&self) -> &str {
        PLUGIN_VERSION
    }

    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            actions: vec![
                "clipboard_read".to_string(),
                "clipboard_write".to_string(),
                "clipboard_providers".to_string(),
            ],
            ..Default::default()
        }
    }

    async fn on_init(&mut self, _client: &mut VeyronClient) -> Result<(), VeyronError> {
        Ok(())
    }

    async fn on_message(&mut self, envelope: Envelope) -> Result<Option<Envelope>, VeyronError> {
        match envelope.payload {
            Some(envelope::Payload::ActionRequest(req)) => {
                let response = handle_action_request(req).await;
                Ok(Some(Envelope {
                    payload: Some(envelope::Payload::ActionResponse(response)),
                    ..Default::default()
                }))
            }
            _ => Ok(None),
        }
    }

    async fn on_shutdown(&mut self) -> Result<(), VeyronError> {
        Ok(())
    }
}

async fn handle_action_request(req: ActionRequest) -> ActionResponse {
    let params: Value = match serde_json::from_slice(&req.params_json) {
        Ok(v) => v,
        Err(e) => {
            return ActionResponse {
                action_id: req.action_id,
                status: ActionStatus::ActionError as i32,
                data_json: Vec::new(),
                error: format!("invalid params_json: {e}"),
            };
        }
    };

    let cfg = handler::Config::from_env();
    let result: Result<Value, String> = match req.action.as_str() {
        "clipboard_read" => {
            let session = match providers::detect_session_from_env() {
                Ok(s) => s,
                Err(e) => return err_response(req.action_id, e),
            };
            let runner = providers::RealRunner;
            handler::handle_read(&runner, &cfg, session).await
        }
        "clipboard_write" => {
            let text = match params.get("text").and_then(Value::as_str) {
                Some(t) => t,
                None => {
                    return err_response(
                        req.action_id,
                        "ERR_CLIPBOARD_BAD_PARAMS: missing or invalid `text` (non-empty string)"
                            .to_string(),
                    )
                }
            };
            let session = match providers::detect_session_from_env() {
                Ok(s) => s,
                Err(e) => return err_response(req.action_id, e),
            };
            let runner = providers::RealRunner;
            handler::handle_write(&runner, &cfg, session, text).await
        }
        "clipboard_providers" => {
            let session = match providers::detect_session_from_env() {
                Ok(s) => s,
                Err(e) => return err_response(req.action_id, e),
            };
            Ok(handler::handle_providers(session))
        }
        other => {
            return ActionResponse {
                action_id: req.action_id,
                status: ActionStatus::ActionNotFound as i32,
                data_json: Vec::new(),
                error: format!("unknown action: {other}"),
            };
        }
    };

    match result {
        Ok(data) => ActionResponse {
            action_id: req.action_id,
            status: ActionStatus::ActionOk as i32,
            data_json: data.to_string().into_bytes(),
            error: String::new(),
        },
        Err(error) => err_response(req.action_id, error),
    }
}

fn err_response(action_id: String, error: String) -> ActionResponse {
    ActionResponse {
        action_id,
        status: ActionStatus::ActionError as i32,
        data_json: Vec::new(),
        error,
    }
}

#[tokio::main]
async fn main() -> Result<(), VeyronError> {
    ClipboardPlugin.run().await
}
