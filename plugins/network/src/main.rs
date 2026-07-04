//! `network` plugin — outbound HTTP for other plugins/kernel, gated by
//! `PERMISSION_NETWORK`. See
//! docs/superpowers/specs/2026-07-05-network-plugin-design.md for the design.
//!
//! v1 is HTTP only. Needs real network egress: run with `sandbox: false`
//! in the kernel's `config.yaml` (see README.md).

use std::sync::Arc;

use network_plugin::{handler, request};
use veyron_sdk::proto::{envelope, ActionResponse, ActionStatus, Envelope, Event, PluginManifest};
use veyron_sdk::{Plugin, VeyronClient, VeyronError};

struct NetworkPlugin {
    client: reqwest::Client,
}

impl NetworkPlugin {
    fn new() -> Self {
        // SSRF gating lives in `SsrfSafeResolver` (used for every connect,
        // including redirects); `Policy::none()` additionally disables
        // redirects outright since v1 has no need to follow them and it
        // keeps the SSRF-safe resolver as the sole point of trust rather
        // than relying on it alone for redirect hops too.
        let resolver = handler::SsrfSafeResolver {
            extra_blocklist: network_plugin::ssrf::Blocklist::from_env(),
        };
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .dns_resolver(Arc::new(resolver))
            .build()
            .expect("failed to build reqwest client");
        Self { client }
    }

    async fn handle_http_request(&self, params_json: &[u8]) -> Result<Vec<u8>, String> {
        let params = request::parse_request(params_json)?;
        let resp = handler::fetch(&self.client, &params).await?;
        serde_json::to_vec(&serde_json::json!({
            "status": resp.status,
            "headers": resp.headers,
            "body": resp.body,
        }))
        .map_err(|e| format!("failed to encode response: {e}"))
    }
}

impl Plugin for NetworkPlugin {
    fn id(&self) -> &str {
        "network"
    }

    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            permissions: vec!["PERMISSION_NETWORK".into()],
            actions: vec!["http_request".into()],
            ..Default::default()
        }
    }

    async fn on_init(&mut self, _client: &mut VeyronClient) -> Result<(), VeyronError> {
        println!("[{}] registered with kernel", self.id());
        Ok(())
    }

    async fn on_message(&mut self, envelope: Envelope) -> Result<Option<Envelope>, VeyronError> {
        match envelope.payload {
            Some(envelope::Payload::ActionRequest(req)) if req.action == "http_request" => {
                let reply = match self.handle_http_request(&req.params_json).await {
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
                };
                Ok(Some(Envelope {
                    payload: Some(envelope::Payload::ActionResponse(reply)),
                    ..Default::default()
                }))
            }
            Some(envelope::Payload::ActionRequest(req)) => Ok(Some(Envelope {
                payload: Some(envelope::Payload::ActionResponse(ActionResponse {
                    action_id: req.action_id,
                    status: ActionStatus::ActionNotFound as i32,
                    data_json: Vec::new(),
                    error: format!("unknown action: {}", req.action),
                })),
                ..Default::default()
            })),
            other => {
                println!("[{}] unhandled message: {other:?}", self.id());
                Ok(None)
            }
        }
    }

    async fn on_event(&mut self, _event: Event) -> Result<Option<Envelope>, VeyronError> {
        Ok(None)
    }

    async fn on_shutdown(&mut self) -> Result<(), VeyronError> {
        println!("[{}] shutting down", self.id());
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), VeyronError> {
    NetworkPlugin::new().run().await
}
