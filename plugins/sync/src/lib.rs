//! `sync` plugin library crate.
//!
//! The [`ConcurrentHandler`] implementation for [`SyncHandler`] lives here
//! (not in the binary crate) because of the orphan rule: the trait comes
//! from `veyron-sdk` and the type from this crate, so the impl must be
//! written where the type is defined. It wires the SDK's concurrent message
//! loop to this plugin's request dispatcher, and turns each mutation delta
//! into a best-effort `sync.delta` event publish sent only after the
//! response.

pub mod db;
pub mod handler;
pub mod request;

use veyron_sdk::concurrent::response_envelope;
use veyron_sdk::proto::{envelope, ActionRequest, Envelope, EventPublish, PluginManifest};
use veyron_sdk::ConcurrentHandler;

use handler::{Delta, SyncHandler};

/// Event type this plugin publishes on every mutation. The kernel prepends
/// `plugin.<sender_id>.` at delivery, so subscribers must watch
/// `plugin.sync.sync.delta`.
const DELTA_EVENT_TYPE: &str = "sync.delta";

impl ConcurrentHandler for SyncHandler {
    fn id(&self) -> &str {
        "sync"
    }

    fn version(&self) -> &str {
        "0.1.0"
    }

    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            permissions: vec![
                "PERMISSION_STORAGE".into(),
                "PERMISSION_EVENT_PUBLISH".into(),
            ],
            actions: vec![
                "sync_get_snapshot".into(),
                "sync_get".into(),
                "sync_set".into(),
                "sync_del".into(),
            ],
            events: vec![DELTA_EVENT_TYPE.into()],
            ..Default::default()
        }
    }

    async fn on_action(&self, req: ActionRequest) -> Vec<Envelope> {
        let mut envelopes = Vec::new();
        match self
            .handle(&req.caller_plugin_id, &req.action, &req.params_json)
            .await
        {
            Ok((response_json, deltas)) => {
                // Response first — the caller's reply never waits on the
                // event publishes that follow.
                envelopes.push(response_envelope(req.action_id, Ok(response_json)));
                // Deltas are already ordered ascending by version (prune
                // deltas before the mutation's own delta).
                envelopes.extend(deltas.into_iter().map(delta_envelope));
            }
            Err(error) => {
                envelopes.push(response_envelope(req.action_id, Err(error)));
            }
        }
        envelopes
    }
}

fn delta_envelope(delta: Delta) -> Envelope {
    Envelope {
        payload: Some(envelope::Payload::EventPublish(EventPublish {
            event_type: DELTA_EVENT_TYPE.to_string(),
            payload_json: delta.payload_json(),
        })),
        ..Default::default()
    }
}
