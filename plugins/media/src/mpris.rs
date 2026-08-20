//! Local MPRIS implementation for the `media` plugin.
//!
//! Talks to `org.mpris.MediaPlayer2.*` players on the session D-Bus via
//! `zbus` (session bus, object `/org/mpris/MediaPlayer2`,
//! interface `org.mpris.MediaPlayer2.Player`). All public functions are
//! testable: the D-Bus is accessed only through `MprisBackend` so tests use
//! `MockBackend`.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use async_trait::async_trait;
use serde::Serialize;
use zbus::Connection;
use zbus::fdo::{DBusProxy, PropertiesProxy};
use zbus::names::InterfaceName;
use zvariant::{ObjectPath, OwnedValue, Value};

const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";
const NO_TRACK: &str = "/TrackList/NoTrack";

// ---------------------------------------------------------------------------
// Backend trait
// ---------------------------------------------------------------------------

#[async_trait]
pub trait MprisBackend: Send + Sync {
    async fn list_names(&self) -> Result<Vec<String>, String>;
    async fn call_player_method(
        &self,
        player: &str,
        method: &str,
        arg: Option<&str>,
    ) -> Result<(), String>;
    async fn seek_offset(&self, player: &str, offset_us: i64) -> Result<(), String>;
    async fn set_position(&self, player: &str, track_id: &str, position_us: i64) -> Result<(), String>;
    async fn get_position(&self, player: &str) -> Result<i64, String>;
    async fn get_volume(&self, player: &str) -> Result<f64, String>;
    async fn set_volume(&self, player: &str, volume: f64) -> Result<(), String>;
    async fn get_playback_status(&self, player: &str) -> Result<String, String>;
    async fn get_metadata(&self, player: &str) -> Result<HashMap<String, OwnedValue>, String>;
    async fn get_rate(&self, player: &str) -> Result<f64, String>;
    async fn get_shuffle(&self, player: &str) -> Result<bool, String>;
    async fn set_shuffle(&self, player: &str, enabled: bool) -> Result<(), String>;
    async fn get_loop_status(&self, player: &str) -> Result<String, String>;
    async fn set_loop_status(&self, player: &str, status: &str) -> Result<(), String>;
}

// ---------------------------------------------------------------------------
// Real backend — zbus session bus
// ---------------------------------------------------------------------------

pub struct RealBackend;

#[async_trait]
impl MprisBackend for RealBackend {
    async fn list_names(&self) -> Result<Vec<String>, String> {
        let conn = Connection::session()
            .await
            .map_err(|e| format!("ERR_MEDIA_BUS_UNAVAILABLE: {e}"))?;
        let proxy = DBusProxy::new(&conn)
            .await
            .map_err(|e| format!("ERR_MEDIA_BUS_UNAVAILABLE: {e}"))?;
        let names = proxy
            .list_names()
            .await
            .map_err(|e| format!("ERR_MEDIA_BUS_UNAVAILABLE: {e}"))?;
        Ok(names.into_iter().map(|n| n.to_string()).collect())
    }

    async fn call_player_method(
        &self,
        player: &str,
        method: &str,
        arg: Option<&str>,
    ) -> Result<(), String> {
        let conn = Connection::session()
            .await
            .map_err(|e| format!("ERR_MEDIA_BUS_UNAVAILABLE: {e}"))?;
        if let Some(uri) = arg {
            conn.call_method(
                Some(player),
                MPRIS_PATH,
                Some(PLAYER_IFACE),
                method,
                &(uri,),
            )
            .await
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' {method} failed: {e}"))?;
        } else {
            conn.call_method(
                Some(player),
                MPRIS_PATH,
                Some(PLAYER_IFACE),
                method,
                &(),
            )
            .await
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' {method} failed: {e}"))?;
        }
        Ok(())
    }

    async fn seek_offset(&self, player: &str, offset_us: i64) -> Result<(), String> {
        let conn = Connection::session()
            .await
            .map_err(|e| format!("ERR_MEDIA_BUS_UNAVAILABLE: {e}"))?;
        conn.call_method(
            Some(player),
            MPRIS_PATH,
            Some(PLAYER_IFACE),
            "Seek",
            &(offset_us,),
        )
        .await
        .map_err(|e| format!("ERR_MEDIA_SEEK_FAILED: player '{player}' Seek failed: {e}"))?;
        Ok(())
    }

    async fn set_position(&self, player: &str, track_id: &str, position_us: i64) -> Result<(), String> {
        let conn = Connection::session()
            .await
            .map_err(|e| format!("ERR_MEDIA_BUS_UNAVAILABLE: {e}"))?;
        let path = ObjectPath::try_from(track_id)
            .map_err(|e| format!("ERR_MEDIA_BAD_PARAMS: bad trackId '{track_id}': {e}"))?;
        conn.call_method(
            Some(player),
            MPRIS_PATH,
            Some(PLAYER_IFACE),
            "SetPosition",
            &(path, position_us),
        )
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("UnknownMethod") || msg.contains("NotSupported") {
                format!("ERR_MEDIA_NOT_SUPPORTED: SetPosition not supported: {msg}")
            } else if msg.contains("Invalid") {
                format!("ERR_MEDIA_BAD_PARAMS: SetPosition invalid trackId/position: {msg}")
            } else {
                format!("ERR_MEDIA_SEEK_FAILED: player '{player}' SetPosition failed: {msg}")
            }
        })?;
        Ok(())
    }

    async fn get_position(&self, player: &str) -> Result<i64, String> {
        let conn = Connection::session()
            .await
            .map_err(|e| format!("ERR_MEDIA_BUS_UNAVAILABLE: {e}"))?;
        let iface = InterfaceName::try_from(PLAYER_IFACE).unwrap();
        let props = PropertiesProxy::builder(&conn)
            .destination(player)
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get Position failed: {e}"))?
            .path(MPRIS_PATH)
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get Position failed: {e}"))?
            .build()
            .await
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get Position failed: {e}"))?;
        let v: OwnedValue = props
            .get(iface.clone(), "Position")
            .await
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get Position failed: {e}"))?;
        i64::try_from(v).map_err(|e| format!("ERR_MEDIA_BAD_PARAMS: player '{player}' bad Position type: {e}"))
    }

    async fn get_volume(&self, player: &str) -> Result<f64, String> {
        let conn = Connection::session()
            .await
            .map_err(|e| format!("ERR_MEDIA_BUS_UNAVAILABLE: {e}"))?;
        let iface = InterfaceName::try_from(PLAYER_IFACE).unwrap();
        let props = PropertiesProxy::builder(&conn)
            .destination(player)
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get Volume failed: {e}"))?
            .path(MPRIS_PATH)
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get Volume failed: {e}"))?
            .build()
            .await
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get Volume failed: {e}"))?;
        let v: OwnedValue = props
            .get(iface.clone(), "Volume")
            .await
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get Volume failed: {e}"))?;
        f64::try_from(v).map_err(|e| format!("ERR_MEDIA_BAD_PARAMS: player '{player}' bad Volume type: {e}"))
    }

    async fn set_volume(&self, player: &str, volume: f64) -> Result<(), String> {
        let conn = Connection::session()
            .await
            .map_err(|e| format!("ERR_MEDIA_BUS_UNAVAILABLE: {e}"))?;
        let iface = InterfaceName::try_from(PLAYER_IFACE).unwrap();
        let props = PropertiesProxy::builder(&conn)
            .destination(player)
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' set Volume failed: {e}"))?
            .path(MPRIS_PATH)
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' set Volume failed: {e}"))?
            .build()
            .await
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' set Volume failed: {e}"))?;
        let val = Value::new(volume);
        props
            .set(iface, "Volume", &val)
            .await
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' set Volume failed: {e}"))?;
        Ok(())
    }

    async fn get_playback_status(&self, player: &str) -> Result<String, String> {
        let conn = Connection::session()
            .await
            .map_err(|e| format!("ERR_MEDIA_BUS_UNAVAILABLE: {e}"))?;
        let iface = InterfaceName::try_from(PLAYER_IFACE).unwrap();
        let props = PropertiesProxy::builder(&conn)
            .destination(player)
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get PlaybackStatus failed: {e}"))?
            .path(MPRIS_PATH)
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get PlaybackStatus failed: {e}"))?
            .build()
            .await
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get PlaybackStatus failed: {e}"))?;
        let v: OwnedValue = props
            .get(iface.clone(), "PlaybackStatus")
            .await
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get PlaybackStatus failed: {e}"))?;
        String::try_from(v).map_err(|e| format!("ERR_MEDIA_BAD_PARAMS: player '{player}' bad PlaybackStatus: {e}"))
    }

    async fn get_metadata(&self, player: &str) -> Result<HashMap<String, OwnedValue>, String> {
        let conn = Connection::session()
            .await
            .map_err(|e| format!("ERR_MEDIA_BUS_UNAVAILABLE: {e}"))?;
        let iface = InterfaceName::try_from(PLAYER_IFACE).unwrap();
        let props = PropertiesProxy::builder(&conn)
            .destination(player)
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get Metadata failed: {e}"))?
            .path(MPRIS_PATH)
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get Metadata failed: {e}"))?
            .build()
            .await
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get Metadata failed: {e}"))?;
        let v: OwnedValue = props
            .get(iface.clone(), "Metadata")
            .await
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get Metadata failed: {e}"))?;
        HashMap::<String, OwnedValue>::try_from(v)
            .map_err(|e| format!("ERR_MEDIA_BAD_PARAMS: player '{player}' bad Metadata type: {e}"))
    }

    async fn get_rate(&self, player: &str) -> Result<f64, String> {
        let conn = Connection::session()
            .await
            .map_err(|e| format!("ERR_MEDIA_BUS_UNAVAILABLE: {e}"))?;
        let iface = InterfaceName::try_from(PLAYER_IFACE).unwrap();
        let props = PropertiesProxy::builder(&conn)
            .destination(player)
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get Rate failed: {e}"))?
            .path(MPRIS_PATH)
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get Rate failed: {e}"))?
            .build()
            .await
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get Rate failed: {e}"))?;
        let v: OwnedValue = props
            .get(iface.clone(), "Rate")
            .await
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get Rate failed: {e}"))?;
        f64::try_from(v).map_err(|e| format!("ERR_MEDIA_BAD_PARAMS: player '{player}' bad Rate type: {e}"))
    }

    async fn get_shuffle(&self, player: &str) -> Result<bool, String> {
        let conn = Connection::session()
            .await
            .map_err(|e| format!("ERR_MEDIA_BUS_UNAVAILABLE: {e}"))?;
        let iface = InterfaceName::try_from(PLAYER_IFACE).unwrap();
        let props = PropertiesProxy::builder(&conn)
            .destination(player)
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get Shuffle failed: {e}"))?
            .path(MPRIS_PATH)
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get Shuffle failed: {e}"))?
            .build()
            .await
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get Shuffle failed: {e}"))?;
        let v: OwnedValue = props
            .get(iface.clone(), "Shuffle")
            .await
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get Shuffle failed: {e}"))?;
        bool::try_from(v).map_err(|e| format!("ERR_MEDIA_BAD_PARAMS: player '{player}' bad Shuffle type: {e}"))
    }

    async fn set_shuffle(&self, player: &str, enabled: bool) -> Result<(), String> {
        let conn = Connection::session()
            .await
            .map_err(|e| format!("ERR_MEDIA_BUS_UNAVAILABLE: {e}"))?;
        let iface = InterfaceName::try_from(PLAYER_IFACE).unwrap();
        let props = PropertiesProxy::builder(&conn)
            .destination(player)
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' set Shuffle failed: {e}"))?
            .path(MPRIS_PATH)
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' set Shuffle failed: {e}"))?
            .build()
            .await
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' set Shuffle failed: {e}"))?;
        let val = Value::new(enabled);
        props
            .set(iface, "Shuffle", &val)
            .await
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' set Shuffle failed: {e}"))?;
        Ok(())
    }

    async fn get_loop_status(&self, player: &str) -> Result<String, String> {
        let conn = Connection::session()
            .await
            .map_err(|e| format!("ERR_MEDIA_BUS_UNAVAILABLE: {e}"))?;
        let iface = InterfaceName::try_from(PLAYER_IFACE).unwrap();
        let props = PropertiesProxy::builder(&conn)
            .destination(player)
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get LoopStatus failed: {e}"))?
            .path(MPRIS_PATH)
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get LoopStatus failed: {e}"))?
            .build()
            .await
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get LoopStatus failed: {e}"))?;
        let v: OwnedValue = props
            .get(iface.clone(), "LoopStatus")
            .await
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' get LoopStatus failed: {e}"))?;
        String::try_from(v).map_err(|e| format!("ERR_MEDIA_BAD_PARAMS: player '{player}' bad LoopStatus: {e}"))
    }

    async fn set_loop_status(&self, player: &str, status: &str) -> Result<(), String> {
        let normalized = match status.to_lowercase().as_str() {
            "none" => "None",
            "track" | "one" | "single" => "Track",
            "playlist" | "all" => "Playlist",
            _ => return Err(format!("ERR_MEDIA_BAD_PARAMS: invalid LoopStatus '{status}' (expected none/track/playlist)")),
        };
        let conn = Connection::session()
            .await
            .map_err(|e| format!("ERR_MEDIA_BUS_UNAVAILABLE: {e}"))?;
        let iface = InterfaceName::try_from(PLAYER_IFACE).unwrap();
        let props = PropertiesProxy::builder(&conn)
            .destination(player)
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' set LoopStatus failed: {e}"))?
            .path(MPRIS_PATH)
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' set LoopStatus failed: {e}"))?
            .build()
            .await
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' set LoopStatus failed: {e}"))?;
        let val = Value::new(normalized);
        props
            .set(iface, "LoopStatus", &val)
            .await
            .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: player '{player}' set LoopStatus failed: {e}"))?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers — player resolution
// ---------------------------------------------------------------------------

fn allowlist() -> Option<Vec<String>> {
    std::env::var("MEDIA_PLUGIN_PLAYERS")
        .ok()
        .map(|s| {
            s.split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty())
}

fn default_player() -> Option<String> {
    std::env::var("MEDIA_PLUGIN_DEFAULT_PLAYER")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn is_allowed_with(list: &Option<Vec<String>>, name: &str) -> bool {
    match list {
        None => true,
        Some(v) => v.iter().any(|a| a == name),
    }
}

fn resolve_player(requested: Option<&str>, available: &[String]) -> Result<String, String> {
    let list = allowlist();
    if let Some(r) = requested {
        if !is_allowed_with(&list, r) {
            return Err(format!("ERR_MEDIA_PLAYER_NOT_ALLOWED: '{r}' not in MEDIA_PLUGIN_PLAYERS"));
        }
        if !available.contains(&r.to_string()) {
            return Err(format!("ERR_MEDIA_PLAYER_NOT_FOUND: '{r}' not available (have: {available:?})"));
        }
        return Ok(r.to_string());
    }
    if let Some(def) = default_player() {
        if !is_allowed_with(&list, &def) {
            return Err(format!("ERR_MEDIA_PLAYER_NOT_ALLOWED: default '{def}' not in allowlist"));
        }
        if available.contains(&def) {
            return Ok(def);
        }
    }
    available
        .iter()
        .find(|n| is_allowed_with(&list, n))
        .cloned()
        .ok_or_else(|| "ERR_MEDIA_NO_PLAYERS: no MPRIS players available".to_string())
}

// ---------------------------------------------------------------------------
// Public free functions — used by main.rs
// ---------------------------------------------------------------------------

pub async fn list_players() -> Result<Vec<String>, String> {
    list_players_with(&RealBackend).await
}

async fn list_players_with<B: MprisBackend>(backend: &B) -> Result<Vec<String>, String> {
    let names = backend
        .list_names()
        .await
        .map_err(|e| format!("ERR_MEDIA_BUS_UNAVAILABLE: {e}"))?;
    let list = allowlist();
    let mut players: Vec<String> = names
        .into_iter()
        .filter(|n| n.starts_with(MPRIS_PREFIX) && is_allowed_with(&list, n))
        .collect();
    players.sort();
    Ok(players)
}

pub async fn play(player: Option<&str>, uri: Option<&str>) -> Result<serde_json::Value, String> {
    play_with(&RealBackend, player, uri).await
}

async fn play_with<B: MprisBackend>(
    backend: &B,
    player: Option<&str>,
    uri: Option<&str>,
) -> Result<serde_json::Value, String> {
    let available = list_players_with(backend).await?;
    let target = resolve_player(player, &available)?;
    if let Some(u) = uri {
        backend.call_player_method(&target, "OpenUri", Some(u)).await?;
    }
    backend.call_player_method(&target, "Play", None).await?;
    Ok(serde_json::json!({"ok": true}))
}

pub async fn pause(player: Option<&str>) -> Result<serde_json::Value, String> {
    pause_with(&RealBackend, player).await
}
async fn pause_with<B: MprisBackend>(backend: &B, player: Option<&str>) -> Result<serde_json::Value, String> {
    let available = list_players_with(backend).await?;
    let target = resolve_player(player, &available)?;
    backend.call_player_method(&target, "Pause", None).await?;
    Ok(serde_json::json!({"ok": true}))
}

pub async fn play_pause(player: Option<&str>) -> Result<serde_json::Value, String> {
    play_pause_with(&RealBackend, player).await
}
async fn play_pause_with<B: MprisBackend>(backend: &B, player: Option<&str>) -> Result<serde_json::Value, String> {
    let available = list_players_with(backend).await?;
    let target = resolve_player(player, &available)?;
    backend.call_player_method(&target, "PlayPause", None).await?;
    // BUG-3 fix: PlaybackStatus updates async via PropertiesChanged, so retry
    // briefly instead of reading stale value immediately.
    let mut status = backend.get_playback_status(&target).await.unwrap_or_else(|_| "Paused".into());
    if status != "Playing" && status != "Paused" {
        status = "Paused".into();
    }
    // If we toggled, the status should flip within ~300ms. Poll with backoff.
    // We don't know prior state, but we retry up to 3 times if first read
    // looks stale (best-effort). Final value is reported regardless.
    for delay_ms in [50, 100, 150] {
        // Only retry if we suspect stale read: sleep and re-check.
        // Cheap for mock, correct for browsers.
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        if let Ok(new_status) = backend.get_playback_status(&target).await {
            if new_status != status {
                status = new_status;
                break;
            }
        }
    }
    Ok(serde_json::json!({"ok": true, "playing": status == "Playing"}))
}

pub async fn next(player: Option<&str>) -> Result<serde_json::Value, String> {
    next_with(&RealBackend, player).await
}
async fn next_with<B: MprisBackend>(backend: &B, player: Option<&str>) -> Result<serde_json::Value, String> {
    let available = list_players_with(backend).await?;
    let target = resolve_player(player, &available)?;
    backend.call_player_method(&target, "Next", None).await?;
    Ok(serde_json::json!({"ok": true}))
}

pub async fn prev(player: Option<&str>) -> Result<serde_json::Value, String> {
    prev_with(&RealBackend, player).await
}
async fn prev_with<B: MprisBackend>(backend: &B, player: Option<&str>) -> Result<serde_json::Value, String> {
    let available = list_players_with(backend).await?;
    let target = resolve_player(player, &available)?;
    backend.call_player_method(&target, "Previous", None).await?;
    Ok(serde_json::json!({"ok": true}))
}

pub async fn stop(player: Option<&str>) -> Result<serde_json::Value, String> {
    stop_with(&RealBackend, player).await
}
async fn stop_with<B: MprisBackend>(backend: &B, player: Option<&str>) -> Result<serde_json::Value, String> {
    let available = list_players_with(backend).await?;
    let target = resolve_player(player, &available)?;
    backend.call_player_method(&target, "Stop", None).await?;
    Ok(serde_json::json!({"ok": true}))
}

pub async fn seek(player: Option<&str>, position_ms: u64) -> Result<serde_json::Value, String> {
    seek_with(&RealBackend, player, position_ms).await
}
async fn seek_with<B: MprisBackend>(
    backend: &B,
    player: Option<&str>,
    position_ms: u64,
) -> Result<serde_json::Value, String> {
    let available = list_players_with(backend).await?;
    let target = resolve_player(player, &available)?;
    let target_us = (position_ms as i64)
        .checked_mul(1000)
        .ok_or_else(|| "ERR_MEDIA_BAD_PARAMS: position_ms overflow".to_string())?;

    // Fetch trackId — required for SetPosition. If metadata unavailable,
    // fall back to Seek directly.
    let track_id_opt = match backend.get_metadata(&target).await {
        Ok(raw) => parse_metadata(&raw).track_id,
        Err(_) => None,
    };

    if let Some(track_id) = track_id_opt {
        if track_id == NO_TRACK {
            return Err("ERR_MEDIA_NO_TRACK: no track loaded, cannot seek".to_string());
        }
        match backend.set_position(&target, &track_id, target_us).await {
            Ok(()) => return Ok(serde_json::json!({"position_ms": position_ms})),
            Err(e) if e.contains("ERR_MEDIA_NOT_SUPPORTED") || e.contains("UnknownMethod") || e.contains("NotSupported") => {
                // Fall through to Seek fallback
            }
            Err(e) if e.contains("ERR_MEDIA_BAD_PARAMS") => return Err(e),
            Err(e) => return Err(e),
        }
    }

    let current = backend
        .get_position(&target)
        .await
        .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: get Position failed: {e}"))?;
    let offset = target_us
        .checked_sub(current)
        .ok_or_else(|| "ERR_MEDIA_BAD_PARAMS: seek offset overflow".to_string())?;
    backend.seek_offset(&target, offset).await?;
    Ok(serde_json::json!({"position_ms": position_ms}))
}

pub async fn set_volume(player: Option<&str>, level: f64) -> Result<serde_json::Value, String> {
    set_volume_with(&RealBackend, player, level).await
}
async fn set_volume_with<B: MprisBackend>(
    backend: &B,
    player: Option<&str>,
    level: f64,
) -> Result<serde_json::Value, String> {
    let available = list_players_with(backend).await?;
    let target = resolve_player(player, &available)?;
    let clamped = level.clamp(0.0, 1.0);
    backend.set_volume(&target, clamped).await?;
    Ok(serde_json::json!({"volume": clamped}))
}

pub async fn status(player: Option<&str>) -> Result<serde_json::Value, String> {
    status_with(&RealBackend, player).await
}
async fn status_with<B: MprisBackend>(
    backend: &B,
    player: Option<&str>,
) -> Result<serde_json::Value, String> {
    let available = list_players_with(backend).await?;
    if available.is_empty() {
        return Err("ERR_MEDIA_NO_PLAYERS: no MPRIS players available".to_string());
    }
    let target = resolve_player(player, &available)?;
    let playback = backend
        .get_playback_status(&target)
        .await
        .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: {e}"))?;
    let volume = backend
        .get_volume(&target)
        .await
        .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: {e}"))?;
    let raw_position_us = backend
        .get_position(&target)
        .await
        .map_err(|e| format!("ERR_MEDIA_PLAYER_VANISHED: {e}"))?;
    let metadata_raw = backend.get_metadata(&target).await.unwrap_or_default();
    let fresh_meta = parse_metadata(&metadata_raw);
    let meta = merge_metadata_cached(&target, fresh_meta);

    let rate = backend
        .get_rate(&target)
        .await
        .unwrap_or(if playback == "Playing" { 1.0 } else { 0.0 });
    let shuffle = backend.get_shuffle(&target).await.unwrap_or(false);
    let loop_status = backend.get_loop_status(&target).await.unwrap_or_else(|_| "None".into());

    let position_us = extrapolate_position(&target, raw_position_us, rate, &playback);

    Ok(serde_json::json!({
        "player": target,
        "status": playback,
        "metadata": {
            "title": meta.title,
            "artists": meta.artists,
            "album": meta.album,
            "length_micros": meta.length_micros,
            "track_id": meta.track_id,
            "art_url": meta.art_url,
        },
        "volume": volume,
        "position_ms": position_us / 1000,
        "rate": rate,
        "shuffle": shuffle,
        "loop_status": loop_status
    }))
}

fn extrapolate_position(player: &str, raw_pos: i64, rate: f64, playback: &str) -> i64 {
    let now = Instant::now();
    let mut cache = pos_cache().lock().unwrap();
    let result = if playback == "Playing" && rate != 0.0 {
        if raw_pos == 0 {
            if let Some((prev_pos, prev_rate, prev_time)) = cache.get(player) {
                if *prev_pos > 0 {
                    let elapsed_us = now.duration_since(*prev_time).as_micros() as i64;
                    let estimated = *prev_pos + (elapsed_us as f64 * prev_rate) as i64;
                    if estimated > 0 {
                        estimated
                    } else {
                        raw_pos
                    }
                } else {
                    raw_pos
                }
            } else {
                raw_pos
            }
        } else {
            raw_pos
        }
    } else {
        raw_pos
    };
    let to_store = if playback == "Playing" && rate != 0.0 && result > 0 {
        result
    } else {
        raw_pos
    };
    cache.insert(player.to_string(), (to_store, rate, now));
    result
}

pub async fn set_shuffle(player: Option<&str>, enabled: bool) -> Result<serde_json::Value, String> {
    set_shuffle_with(&RealBackend, player, enabled).await
}
async fn set_shuffle_with<B: MprisBackend>(backend: &B, player: Option<&str>, enabled: bool) -> Result<serde_json::Value, String> {
    let available = list_players_with(backend).await?;
    let target = resolve_player(player, &available)?;
    backend.set_shuffle(&target, enabled).await?;
    Ok(serde_json::json!({"shuffle": enabled}))
}

pub async fn set_loop(player: Option<&str>, mode: &str) -> Result<serde_json::Value, String> {
    set_loop_with(&RealBackend, player, mode).await
}
async fn set_loop_with<B: MprisBackend>(backend: &B, player: Option<&str>, mode: &str) -> Result<serde_json::Value, String> {
    let available = list_players_with(backend).await?;
    let target = resolve_player(player, &available)?;
    backend.set_loop_status(&target, mode).await?;
    let normalized = backend.get_loop_status(&target).await.unwrap_or_else(|_| {
        match mode.to_lowercase().as_str() {
            "none" => "None".into(),
            "track" | "one" | "single" => "Track".into(),
            _ => "Playlist".into(),
        }
    });
    Ok(serde_json::json!({"loop_status": normalized}))
}

// ---------------------------------------------------------------------------
// Metadata parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Serialize)]
pub struct MediaMetadata {
    pub title: Option<String>,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub length_micros: Option<i64>,
    pub track_id: Option<String>,
    pub art_url: Option<String>,
}

static META_CACHE: OnceLock<Mutex<HashMap<String, MediaMetadata>>> = OnceLock::new();
static POS_CACHE: OnceLock<Mutex<HashMap<String, (i64, f64, Instant)>>> = OnceLock::new();

fn meta_cache() -> &'static Mutex<HashMap<String, MediaMetadata>> {
    META_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn pos_cache() -> &'static Mutex<HashMap<String, (i64, f64, Instant)>> {
    POS_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn merge_metadata_cached(player: &str, fresh: MediaMetadata) -> MediaMetadata {
    let mut cache = meta_cache().lock().unwrap();
    let cached = cache.get(player).cloned().unwrap_or_default();
    let merged = MediaMetadata {
        title: fresh.title.or(cached.title),
        artists: if fresh.artists.is_empty() { cached.artists } else { fresh.artists },
        album: fresh.album.or(cached.album),
        length_micros: fresh.length_micros.or(cached.length_micros),
        track_id: fresh.track_id.or(cached.track_id),
        art_url: fresh.art_url.or(cached.art_url),
    };
    cache.insert(player.to_string(), merged.clone());
    merged
}

fn meta_string(metadata: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    let v = metadata.get(key)?.try_clone().ok()?;
    match Value::try_from(v).ok()? {
        Value::Str(s) => Some(s.to_string()),
        Value::ObjectPath(o) => Some(o.to_string()),
        _ => None,
    }
}

fn meta_string_array(metadata: &HashMap<String, OwnedValue>, key: &str) -> Vec<String> {
    let Some(v) = metadata.get(key).and_then(|v| v.try_clone().ok()) else {
        return Vec::new();
    };
    match Value::try_from(v).ok() {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|e| match e {
                Value::Str(s) => Some(s.to_string()),
                _ => String::try_from(e).ok(),
            })
            .collect(),
        Some(Value::Str(s)) => vec![s.to_string()],
        _ => Vec::new(),
    }
}

fn parse_length_value(v: OwnedValue) -> Option<i64> {
    let val = Value::try_from(v).ok()?;
    match val {
        Value::I64(n) => Some(n),
        Value::U64(n) => Some(n as i64),
        Value::I32(n) => Some(n as i64),
        Value::U32(n) => Some(n as i64),
        Value::I16(n) => Some(n as i64),
        Value::U16(n) => Some(n as i64),
        Value::U8(n) => Some(n as i64),
        _ => None,
    }
}

pub fn parse_metadata(metadata: &HashMap<String, OwnedValue>) -> MediaMetadata {
    MediaMetadata {
        title: meta_string(metadata, "xesam:title"),
        artists: meta_string_array(metadata, "xesam:artist"),
        album: meta_string(metadata, "xesam:album"),
        length_micros: metadata
            .get("mpris:length")
            .and_then(|v| v.try_clone().ok())
            .and_then(parse_length_value),
        track_id: meta_string(metadata, "mpris:trackid"),
        art_url: meta_string(metadata, "mpris:artUrl"),
    }
}

// ---------------------------------------------------------------------------
// Tests — pure parsing + mock backend, no D-Bus needed
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use zvariant::OwnedValue;

    fn s(v: &str) -> OwnedValue {
        OwnedValue::try_from(Value::new(v)).unwrap()
    }
    fn arr_str(vals: &[&str]) -> OwnedValue {
        let vec: Vec<String> = vals.iter().map(|s| s.to_string()).collect();
        OwnedValue::try_from(Value::new(vec)).unwrap()
    }
    fn i64v(n: i64) -> OwnedValue {
        OwnedValue::try_from(Value::new(n)).unwrap()
    }
    fn u64v(n: u64) -> OwnedValue {
        OwnedValue::try_from(Value::new(n)).unwrap()
    }
    fn u32v(n: u32) -> OwnedValue {
        OwnedValue::try_from(Value::new(n)).unwrap()
    }

    #[test]
    fn parse_empty() {
        let m: HashMap<String, OwnedValue> = HashMap::new();
        let p = parse_metadata(&m);
        assert!(p.title.is_none());
        assert!(p.artists.is_empty());
        assert!(p.length_micros.is_none());
    }

    #[test]
    fn parse_full() {
        let mut m = HashMap::new();
        m.insert("xesam:title".into(), s("Song"));
        m.insert("xesam:artist".into(), arr_str(&["Alice", "Bob"]));
        m.insert("xesam:album".into(), s("Album X"));
        m.insert("mpris:length".into(), i64v(210_000_000));
        m.insert("mpris:trackid".into(), s("/org/mpris/MediaPlayer2/Track/1"));
        m.insert("mpris:artUrl".into(), s("https://example.com/cover.jpg"));
        let p = parse_metadata(&m);
        assert_eq!(p.title.as_deref(), Some("Song"));
        assert_eq!(p.artists, vec!["Alice", "Bob"]);
        assert_eq!(p.album.as_deref(), Some("Album X"));
        assert_eq!(p.length_micros, Some(210_000_000));
        assert_eq!(p.track_id.as_deref(), Some("/org/mpris/MediaPlayer2/Track/1"));
        assert_eq!(p.art_url.as_deref(), Some("https://example.com/cover.jpg"));
    }

    #[test]
    fn parse_artist_single_string() {
        let mut m = HashMap::new();
        m.insert("xesam:artist".into(), s("Solo"));
        let p = parse_metadata(&m);
        assert_eq!(p.artists, vec!["Solo"]);
    }

    #[test]
    fn parse_artist_array() {
        let mut m = HashMap::new();
        m.insert("xesam:artist".into(), arr_str(&["A", "B", "C"]));
        let p = parse_metadata(&m);
        assert_eq!(p.artists, vec!["A", "B", "C"]);
    }

    #[test]
    fn parse_length_u64() {
        let mut m = HashMap::new();
        m.insert("mpris:length".into(), u64v(123_456_789));
        let p = parse_metadata(&m);
        assert_eq!(p.length_micros, Some(123_456_789));
    }

    #[test]
    fn parse_length_u32() {
        let mut m = HashMap::new();
        m.insert("mpris:length".into(), u32v(42_000_000));
        let p = parse_metadata(&m);
        assert_eq!(p.length_micros, Some(42_000_000));
    }

    #[test]
    fn parse_missing_length() {
        let mut m = HashMap::new();
        m.insert("xesam:title".into(), s("No length"));
        let p = parse_metadata(&m);
        assert!(p.length_micros.is_none());
    }

    struct MockBackend {
        names: Vec<String>,
        position: i64,
        status: String,
        track_id: Option<String>,
        set_position_ok: bool,
        rate: f64,
        shuffle: bool,
        loop_status: String,
    }
    #[async_trait]
    impl MprisBackend for MockBackend {
        async fn list_names(&self) -> Result<Vec<String>, String> {
            Ok(self.names.clone())
        }
        async fn call_player_method(&self, _player: &str, _method: &str, _arg: Option<&str>) -> Result<(), String> {
            Ok(())
        }
        async fn seek_offset(&self, _player: &str, _offset: i64) -> Result<(), String> {
            Ok(())
        }
        async fn set_position(&self, _player: &str, _track_id: &str, _pos: i64) -> Result<(), String> {
            if self.set_position_ok { Ok(()) } else { Err("ERR_MEDIA_NOT_SUPPORTED: mock".into()) }
        }
        async fn get_position(&self, _player: &str) -> Result<i64, String> { Ok(self.position) }
        async fn get_volume(&self, _player: &str) -> Result<f64, String> { Ok(0.5) }
        async fn set_volume(&self, _player: &str, _v: f64) -> Result<(), String> { Ok(()) }
        async fn get_playback_status(&self, _player: &str) -> Result<String, String> { Ok(self.status.clone()) }
        async fn get_metadata(&self, _player: &str) -> Result<HashMap<String, OwnedValue>, String> {
            let mut m = HashMap::new();
            if let Some(tid) = &self.track_id {
                m.insert("mpris:trackid".into(), s(tid));
            }
            Ok(m)
        }
        async fn get_rate(&self, _player: &str) -> Result<f64, String> { Ok(self.rate) }
        async fn get_shuffle(&self, _player: &str) -> Result<bool, String> { Ok(self.shuffle) }
        async fn set_shuffle(&self, _player: &str, _v: bool) -> Result<(), String> { Ok(()) }
        async fn get_loop_status(&self, _player: &str) -> Result<String, String> { Ok(self.loop_status.clone()) }
        async fn set_loop_status(&self, _player: &str, _s: &str) -> Result<(), String> { Ok(()) }
    }

    fn mock(names: Vec<&str>, position: i64, status: &str, track_id: Option<&str>) -> MockBackend {
        MockBackend {
            names: names.into_iter().map(|s| s.to_string()).collect(),
            position,
            status: status.into(),
            track_id: track_id.map(|s| s.to_string()),
            set_position_ok: true,
            rate: 1.0,
            shuffle: false,
            loop_status: "None".into(),
        }
    }

    #[tokio::test]
    async fn resolve_first_available() {
        let b = mock(vec!["org.mpris.MediaPlayer2.spotify", "org.mpris.MediaPlayer2.vlc"], 0, "Playing", None);
        let players = list_players_with(&b).await.unwrap();
        assert!(players.contains(&"org.mpris.MediaPlayer2.spotify".to_string()));
        let resolved = resolve_player(None, &players).unwrap();
        assert_eq!(resolved, "org.mpris.MediaPlayer2.spotify");
    }

    #[tokio::test]
    async fn list_players_filters_prefix() {
        let b = mock(vec!["org.mpris.MediaPlayer2.spotify", "org.freedesktop.DBus", ":1.42"], 0, "Playing", None);
        let players = list_players_with(&b).await.unwrap();
        assert_eq!(players, vec!["org.mpris.MediaPlayer2.spotify"]);
    }

    #[tokio::test]
    async fn seek_no_track_rejects() {
        let b = mock(vec!["org.mpris.MediaPlayer2.mpd"], 0, "Stopped", Some("/TrackList/NoTrack"));
        let err = seek_with(&b, None, 5000).await.unwrap_err();
        assert!(err.contains("ERR_MEDIA_NO_TRACK"));
    }

    #[tokio::test]
    async fn seek_uses_set_position_primary() {
        let b = mock(vec!["org.mpris.MediaPlayer2.spotify"], 0, "Playing", Some("/org/mpris/MediaPlayer2/Track/1"));
        let res = seek_with(&b, None, 5000).await.unwrap();
        assert_eq!(res["position_ms"], 5000);
    }

    #[tokio::test]
    async fn seek_fallback_to_seek_when_set_position_unsupported() {
        let mut b = mock(vec!["org.mpris.MediaPlayer2.spotify"], 1000, "Playing", Some("/org/mpris/MediaPlayer2/Track/1"));
        b.set_position_ok = false;
        let res = seek_with(&b, None, 5000).await.unwrap();
        assert_eq!(res["position_ms"], 5000);
    }

    #[tokio::test]
    async fn status_includes_shuffle_loop_rate() {
        let b = mock(vec!["org.mpris.MediaPlayer2.spotify"], 5_000_000, "Playing", Some("/Track/1"));
        let v = status_with(&b, None).await.unwrap();
        assert_eq!(v["shuffle"], false);
        assert_eq!(v["loop_status"], "None");
        assert_eq!(v["rate"], 1.0);
        assert_eq!(v["position_ms"], 5000);
    }

    #[tokio::test]
    async fn metadata_cache_keeps_length() {
        let player = "org.mpris.MediaPlayer2.test_cache";
        let m1 = MediaMetadata { title: Some("Song".into()), length_micros: Some(210_000_000), track_id: Some("/Track/1".into()), ..Default::default() };
        merge_metadata_cached(player, m1);
        let m2 = MediaMetadata { title: Some("Song".into()), length_micros: None, track_id: None, ..Default::default() };
        let merged = merge_metadata_cached(player, m2);
        assert_eq!(merged.length_micros, Some(210_000_000));
        assert_eq!(merged.track_id.as_deref(), Some("/Track/1"));
    }
}
