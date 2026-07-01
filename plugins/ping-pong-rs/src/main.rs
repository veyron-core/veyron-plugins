use veyron::proto::veyron::{envelope, ActionResponse, ActionStatus, Envelope, PluginManifest};
use veyron::utils::errors::VeyronError;
use veyron_sdk::{Plugin, VeyronClient};

/// Minimal reference plugin: replies "pong" to a "ping" action, rejects anything else.
struct PingPongPlugin;

impl Plugin for PingPongPlugin {
    fn id(&self) -> &str {
        "ping-pong"
    }

    fn manifest(&self) -> PluginManifest {
        PluginManifest::default()
    }

    async fn on_init(&mut self, _client: &mut VeyronClient) -> Result<(), VeyronError> {
        Ok(())
    }

    async fn on_message(&mut self, envelope: Envelope) -> Result<Option<Envelope>, VeyronError> {
        match envelope.payload {
            Some(envelope::Payload::ActionRequest(req)) if req.action == "ping" => {
                let response = Envelope {
                    payload: Some(envelope::Payload::ActionResponse(ActionResponse {
                        action_id: req.action_id,
                        status: ActionStatus::ActionOk as i32,
                        data_json: br#"{"reply":"pong"}"#.to_vec(),
                        error: String::new(),
                    })),
                    ..Default::default()
                };
                Ok(Some(response))
            }
            Some(envelope::Payload::ActionRequest(req)) => {
                let response = Envelope {
                    payload: Some(envelope::Payload::ActionResponse(ActionResponse {
                        action_id: req.action_id,
                        status: ActionStatus::ActionNotFound as i32,
                        data_json: Vec::new(),
                        error: format!("ping-pong only handles 'ping', got '{}'", req.action),
                    })),
                    ..Default::default()
                };
                Ok(Some(response))
            }
            _ => Ok(None),
        }
    }

    async fn on_shutdown(&mut self) -> Result<(), VeyronError> {
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), VeyronError> {
    PingPongPlugin.run().await
}
