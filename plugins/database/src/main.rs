//! `database` plugin — per-caller-namespaced KV + raw SQL storage, gated by
//! `PERMISSION_STORAGE`. See
//! docs/superpowers/specs/2026-07-15-database-plugin-design.md for the design.

use veyron_sdk::proto::{envelope, ActionResponse, ActionStatus, Envelope, Event, PluginManifest};
use veyron_sdk::{Plugin, VeyronClient, VeyronError};

struct DatabasePlugin;

impl Plugin for DatabasePlugin {
    fn id(&self) -> &str {
        "database"
    }

    fn manifest(&self) -> PluginManifest {
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

    async fn on_init(&mut self, _client: &mut VeyronClient) -> Result<(), VeyronError> {
        println!("[{}] registered with kernel", self.id());
        Ok(())
    }

    async fn on_message(&mut self, envelope: Envelope) -> Result<Option<Envelope>, VeyronError> {
        match envelope.payload {
            Some(envelope::Payload::ActionRequest(req)) => Ok(Some(Envelope {
                payload: Some(envelope::Payload::ActionResponse(ActionResponse {
                    action_id: req.action_id,
                    status: ActionStatus::ActionNotFound as i32,
                    data_json: Vec::new(),
                    error: format!("not yet implemented: {}", req.action),
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
    DatabasePlugin.run().await
}
