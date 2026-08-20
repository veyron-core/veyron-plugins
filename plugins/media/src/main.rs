//! `media` plugin — local MPRIS media playback control for vynkor plugins.
//!
//! v1 is local-only: it drives `org.mpris.MediaPlayer2.*` players on the
//! session D-Bus (Spotify, mpv, VLC, browsers, …), so it needs no network
//! permission. Same shape as `ping-pong-rs`: implements the SDK's `Plugin`
//! trait (sequential, one request at a time) and delegates every D-Bus call
//! to the `mpris` module. See ROADMAP.md for the design rationale.

mod mpris;

use serde_json::Value;
use veyron_sdk::proto::{
    envelope, ActionRequest, ActionResponse, ActionStatus, Envelope, PluginManifest,
};
use veyron_sdk::{Plugin, VeyronClient, VeyronError};

const PLUGIN_ID: &str = "media";
const PLUGIN_VERSION: &str = "0.1.0";

struct MediaPlugin;

impl Plugin for MediaPlugin {
    fn id(&self) -> &str {
        PLUGIN_ID
    }

    fn version(&self) -> &str {
        PLUGIN_VERSION
    }

    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            actions: vec![
                "media_play".to_string(),
                "media_pause".to_string(),
                "media_play_pause".to_string(),
                "media_next".to_string(),
                "media_prev".to_string(),
                "media_stop".to_string(),
                "media_seek".to_string(),
                "media_volume".to_string(),
                "media_status".to_string(),
                "media_list_players".to_string(),
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

/// Parse the request params, extract the optional `player` selector, and
/// dispatch to the matching `mpris` stub. Every path returns an
/// [`ActionResponse`]; errors map to `ACTION_ERROR`, unknown actions to
/// `ACTION_NOT_FOUND`.
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

    let player = params
        .get("player")
        .and_then(Value::as_str)
        .map(str::to_string);

    let result: Result<Value, String> = match req.action.as_str() {
        "media_play" => {
            let uri = params.get("uri").and_then(Value::as_str).map(str::to_string);
            mpris::play(player.as_deref(), uri.as_deref()).await
        }
        "media_pause" => mpris::pause(player.as_deref()).await,
        "media_play_pause" => mpris::play_pause(player.as_deref()).await,
        "media_next" => mpris::next(player.as_deref()).await,
        "media_prev" => mpris::prev(player.as_deref()).await,
        "media_stop" => mpris::stop(player.as_deref()).await,
        "media_seek" => match params.get("position_ms").and_then(Value::as_u64) {
            Some(position_ms) => mpris::seek(player.as_deref(), position_ms).await,
            None => Err("missing or invalid `position_ms`".to_string()),
        },
        "media_volume" => match parse_volume(&params) {
            Some(level) => mpris::set_volume(player.as_deref(), level).await,
            None => Err("missing or invalid `level`".to_string()),
        },
        "media_status" => mpris::status(player.as_deref()).await,
        "media_list_players" => mpris::list_players()
            .await
            .map(|players| serde_json::json!({ "players": players })),
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
        Err(error) => ActionResponse {
            action_id: req.action_id,
            status: ActionStatus::ActionError as i32,
            data_json: Vec::new(),
            error,
        },
    }
}

/// Normalize a `media_volume` `level` to a `0.0..=1.0` fraction. Accepts a
/// fractional `0.0`..=`1.0` value or a `0`..=`100` integer percentage. JSON
/// `1` (integer) reads as 1% while `1.0` (float) reads as 100% — an
/// inherent ambiguity of "fraction or percentage" that callers resolve by
/// sending floats for fractions and integers for percentages.
fn parse_volume(params: &Value) -> Option<f64> {
    let level = params.get("level")?;
    if let Some(n) = level.as_f64() {
        if (0.0..=1.0).contains(&n) {
            return Some(n);
        }
        if (1.0..=100.0).contains(&n) {
            return Some(n / 100.0);
        }
        return None;
    }
    if let Some(n) = level.as_u64() {
        if n <= 100 {
            return Some(n as f64 / 100.0);
        }
    }
    None
}

#[tokio::main]
async fn main() -> Result<(), VeyronError> {
    MediaPlugin.run().await
}
