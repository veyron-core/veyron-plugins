//! `system` plugin binary: SDK `Plugin` trait impl over the shared
//! dispatch in [`system_plugin::handle_action`].
//!
//! Uses the stock SDK serve loop (no outbound RPC — the single-reader
//! rule never comes into play), same shape as `ping-pong-rs`. Backends
//! are probed once before registration; undetected capabilities answer
//! `ERR_SYS_NOT_SUPPORTED` at call time.

use std::sync::Arc;

use veyron_sdk::proto::{envelope, ActionResponse, ActionStatus, Envelope, PluginManifest};
use veyron_sdk::{Plugin, VeyronError};

use system_plugin::{backends::SystemBackends, detect, handle_action, ACTIONS, PLUGIN_ID, PLUGIN_VERSION};

struct SystemPlugin {
    backends: Arc<SystemBackends>,
}

impl Plugin for SystemPlugin {
    fn id(&self) -> &str {
        PLUGIN_ID
    }

    fn version(&self) -> &str {
        PLUGIN_VERSION
    }

    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            // Broad access by design — keep strict (root ROADMAP): this is
            // the one permission covering local system state queries and
            // (P2) simple reversible setters.
            permissions: vec!["PERMISSION_SYSTEM".into()],
            actions: ACTIONS.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    async fn on_message(&mut self, envelope: Envelope) -> Result<Option<Envelope>, VeyronError> {
        let Some(envelope::Payload::ActionRequest(req)) = envelope.payload else {
            return Ok(None);
        };
        let reply = match handle_action(&req.action, &req.params_json, &self.backends).await {
            Ok(value) => ActionResponse {
                action_id: req.action_id,
                status: ActionStatus::ActionOk as i32,
                data_json: value.to_string().into_bytes(),
                error: String::new(),
            },
            Err(e) => {
                let not_found = e.code().as_str() == "ERR_SYS_NOT_FOUND";
                ActionResponse {
                    action_id: req.action_id,
                    status: if not_found {
                        ActionStatus::ActionNotFound as i32
                    } else {
                        ActionStatus::ActionError as i32
                    },
                    data_json: Vec::new(),
                    error: e.to_string(),
                }
            }
        };
        Ok(Some(Envelope {
            payload: Some(envelope::Payload::ActionResponse(reply)),
            ..Default::default()
        }))
    }
}

#[tokio::main]
async fn main() -> Result<(), VeyronError> {
    let backends = Arc::new(detect::detect().await);
    println!(
        "[{PLUGIN_ID}] backends detected: battery={}, volume={}",
        backends.battery.is_some(),
        backends.volume.is_some()
    );
    SystemPlugin { backends }.run().await
}
