//! `database` plugin library crate.
//!
//! The [`ConcurrentHandler`] implementation for [`Handler`] lives here (not
//! in the binary crate) because of the orphan rule: the trait comes from
//! `veyron-sdk` and the type from this crate, so the impl must be written
//! where the type is defined. It wires the SDK's concurrent message loop to
//! this plugin's request dispatcher.

pub mod db;
pub mod handler;
pub mod request;

use veyron_sdk::concurrent::response_envelope;
use veyron_sdk::proto::{ActionRequest, Envelope, PluginManifest};
use veyron_sdk::ConcurrentHandler;

use handler::Handler;

impl ConcurrentHandler for Handler {
    fn id(&self) -> &str {
        "database"
    }

    fn version(&self) -> &str {
        "0.2.0"
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

    async fn on_action(&self, req: ActionRequest) -> Vec<Envelope> {
        let result = self
            .handle(&req.caller_plugin_id, &req.action, &req.params_json)
            .await;
        vec![response_envelope(req.action_id, result)]
    }
}

