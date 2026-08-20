//! Local MPRIS implementation for the `media` plugin.
//!
//! Talks to `org.mpris.MediaPlayer2.*` players on the session D-Bus via
//! `zbus` (session bus, object `/org/mpris/MediaPlayer2`,
//! interface `org.mpris.MediaPlayer2.Player`). All public functions are
//! testable: the D-Bus is accessed only through `MprisBackend` so tests use
//! `MockBackend`.

use std::collections::HashMap;

use async_trait::async_trait;
use serde::Serialize;
use zbus::Connection;
use zbus::fdo::{DBusProxy, PropertiesProxy};
use zbus::names::InterfaceName;
use zvariant::{OwnedValue, Value};

const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";

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
    async fn get_position(&self, player: &str) -> Result<i64, String>;
    async fn get_volume(&self, player: &str) -> Result<f64, String>;
    async fn set_volume(&self, player: &str, volume: f64) -> Result<(), String>;
    async fn get_playback_status(&self, player: &str) -> Result<String, String>;
    async fn get_metadata(&self, player: &str) -> Result<HashMap<String, OwnedValue>, String>;
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
            // OpenUri(uri)
            conn.call_method(
                Some(player),
                MPRIS_PATH,
                Some(PLAYER_IFACE),
                method,
                &(uri,),
            )
            .await
            .map_err(|e| format!("player '{player}' {method} failed: {e}"))?;
        } else {
            conn.call_method(
                Some(player),
                MPRIS_PATH,
                Some(PLAYER_IFACE),
                method,
                &(),
            )
            .await
            .map_err(|e| format!("player '{player}' {method} failed: {e}"))?;
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
        .map_err(|e| format!("player '{player}' Seek failed: {e}"))?;
        Ok(())
    }

    async fn get_position(&self, player: &str) -> Result<i64, String> {
        let conn = Connection::session()
            .await
            .map_err(|e| format!("ERR_MEDIA_BUS_UNAVAILABLE: {e}"))?;
        let iface = InterfaceName::try_from(PLAYER_IFACE).unwrap();
        let props = PropertiesProxy::builder(&conn)
            .destination(player)
            .map_err(|e| format!("player '{player}' get Position failed: {e}"))?
            .path(MPRIS_PATH)
            .map_err(|e| format!("player '{player}' get Position failed: {e}"))?
            .build()
            .await
            .map_err(|e| format!("player '{player}' get Position failed: {e}"))?;
        let v: OwnedValue = props
            .get(iface.clone(), "Position")
            .await
            .map_err(|e| format!("player '{player}' get Position failed: {e}"))?;
        i64::try_from(v).map_err(|e| format!("player '{player}' bad Position type: {e}"))
    }

    async fn get_volume(&self, player: &str) -> Result<f64, String> {
        let conn = Connection::session()
            .await
            .map_err(|e| format!("ERR_MEDIA_BUS_UNAVAILABLE: {e}"))?;
        let iface = InterfaceName::try_from(PLAYER_IFACE).unwrap();
        let props = PropertiesProxy::builder(&conn)
            .destination(player)
            .map_err(|e| format!("player '{player}' get Volume failed: {e}"))?
            .path(MPRIS_PATH)
            .map_err(|e| format!("player '{player}' get Volume failed: {e}"))?
            .build()
            .await
            .map_err(|e| format!("player '{player}' get Volume failed: {e}"))?;
        let v: OwnedValue = props
            .get(iface.clone(), "Volume")
            .await
            .map_err(|e| format!("player '{player}' get Volume failed: {e}"))?;
        f64::try_from(v).map_err(|e| format!("player '{player}' bad Volume type: {e}"))
    }

    async fn set_volume(&self, player: &str, volume: f64) -> Result<(), String> {
        let conn = Connection::session()
            .await
            .map_err(|e| format!("ERR_MEDIA_BUS_UNAVAILABLE: {e}"))?;
        let iface = InterfaceName::try_from(PLAYER_IFACE).unwrap();
        let props = PropertiesProxy::builder(&conn)
            .destination(player)
            .map_err(|e| format!("player '{player}' set Volume failed: {e}"))?
            .path(MPRIS_PATH)
            .map_err(|e| format!("player '{player}' set Volume failed: {e}"))?
            .build()
            .await
            .map_err(|e| format!("player '{player}' set Volume failed: {e}"))?;
        let val = Value::new(volume);
        props
            .set(iface, "Volume", &val)
            .await
            .map_err(|e| format!("player '{player}' set Volume failed: {e}"))?;
        Ok(())
    }

    async fn get_playback_status(&self, player: &str) -> Result<String, String> {
        let conn = Connection::session()
            .await
            .map_err(|e| format!("ERR_MEDIA_BUS_UNAVAILABLE: {e}"))?;
        let iface = InterfaceName::try_from(PLAYER_IFACE).unwrap();
        let props = PropertiesProxy::builder(&conn)
            .destination(player)
            .map_err(|e| format!("player '{player}' get PlaybackStatus failed: {e}"))?
            .path(MPRIS_PATH)
            .map_err(|e| format!("player '{player}' get PlaybackStatus failed: {e}"))?
            .build()
            .await
            .map_err(|e| format!("player '{player}' get PlaybackStatus failed: {e}"))?;
        let v: OwnedValue = props
            .get(iface.clone(), "PlaybackStatus")
            .await
            .map_err(|e| format!("player '{player}' get PlaybackStatus failed: {e}"))?;
        String::try_from(v).map_err(|e| format!("player '{player}' bad PlaybackStatus: {e}"))
    }

    async fn get_metadata(&self, player: &str) -> Result<HashMap<String, OwnedValue>, String> {
        let conn = Connection::session()
            .await
            .map_err(|e| format!("ERR_MEDIA_BUS_UNAVAILABLE: {e}"))?;
        let iface = InterfaceName::try_from(PLAYER_IFACE).unwrap();
        let props = PropertiesProxy::builder(&conn)
            .destination(player)
            .map_err(|e| format!("player '{player}' get Metadata failed: {e}"))?
            .path(MPRIS_PATH)
            .map_err(|e| format!("player '{player}' get Metadata failed: {e}"))?
            .build()
            .await
            .map_err(|e| format!("player '{player}' get Metadata failed: {e}"))?;
        let v: OwnedValue = props
            .get(iface.clone(), "Metadata")
            .await
            .map_err(|e| format!("player '{player}' get Metadata failed: {e}"))?;
        HashMap::<String, OwnedValue>::try_from(v)
            .map_err(|e| format!("player '{player}' bad Metadata type: {e}"))
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

fn is_allowed(name: &str) -> bool {
    match allowlist() {
        None => true,
        Some(list) => list.iter().any(|a| a == name),
    }
}

fn resolve_player(requested: Option<&str>, available: &[String]) -> Result<String, String> {
    if let Some(r) = requested {
        if !is_allowed(r) {
            return Err(format!("ERR_MEDIA_PLAYER_NOT_ALLOWED: '{r}' not in MEDIA_PLUGIN_PLAYERS"));
        }
        if !available.contains(&r.to_string()) {
            return Err(format!("ERR_MEDIA_PLAYER_NOT_FOUND: '{r}' not available (have: {available:?})"));
        }
        return Ok(r.to_string());
    }
    if let Some(def) = default_player() {
        if !is_allowed(&def) {
            return Err(format!("ERR_MEDIA_PLAYER_NOT_ALLOWED: default '{def}' not in allowlist"));
        }
        if available.contains(&def) {
            return Ok(def);
        }
        // default not running — fall through to first available
    }
    available
        .iter()
        .find(|n| is_allowed(n))
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
    let mut players: Vec<String> = names
        .into_iter()
        .filter(|n| n.starts_with(MPRIS_PREFIX) && is_allowed(n))
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
    // read back status to report playing bool (best-effort)
    let status = backend.get_playback_status(&target).await.unwrap_or_else(|_| "Paused".into());
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
    let target_us = (position_ms as i64) * 1000;
    let current = backend.get_position(&target).await.unwrap_or(0);
    let offset = target_us - current;
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
    let playback = backend.get_playback_status(&target).await.unwrap_or_else(|_| "Stopped".into());
    let volume = backend.get_volume(&target).await.unwrap_or(0.0);
    let position_us = backend.get_position(&target).await.unwrap_or(0);
    let metadata_raw = backend.get_metadata(&target).await.unwrap_or_default();
    let meta = parse_metadata(&metadata_raw);
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
        "position_ms": position_us / 1000
    }))
}

// ---------------------------------------------------------------------------
// Metadata parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Serialize)]
pub struct MediaMetadata {
    pub title: Option<String>,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub length_micros: Option<i64>,
    pub track_id: Option<String>,
    pub art_url: Option<String>,
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

pub fn parse_metadata(metadata: &HashMap<String, OwnedValue>) -> MediaMetadata {
    MediaMetadata {
        title: meta_string(metadata, "xesam:title"),
        artists: meta_string_array(metadata, "xesam:artist"),
        album: meta_string(metadata, "xesam:album"),
        length_micros: metadata
            .get("mpris:length")
            .and_then(|v| v.try_clone().ok())
            .and_then(|v| {
                if let Ok(Value::I64(n)) = Value::try_from(v.try_clone().ok()?) {
                    return Some(n);
                }
                if let Ok(Value::U64(n)) = Value::try_from(v) {
                    return Some(n as i64);
                }
                None
            }),
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
        async fn get_position(&self, _player: &str) -> Result<i64, String> { Ok(self.position) }
        async fn get_volume(&self, _player: &str) -> Result<f64, String> { Ok(0.5) }
        async fn set_volume(&self, _player: &str, _v: f64) -> Result<(), String> { Ok(()) }
        async fn get_playback_status(&self, _player: &str) -> Result<String, String> { Ok(self.status.clone()) }
        async fn get_metadata(&self, _player: &str) -> Result<HashMap<String, OwnedValue>, String> { Ok(HashMap::new()) }
    }

    #[tokio::test]
    async fn resolve_first_available() {
        let b = MockBackend { names: vec!["org.mpris.MediaPlayer2.spotify".into(), "org.mpris.MediaPlayer2.vlc".into()], position: 0, status: "Playing".into() };
        let players = list_players_with(&b).await.unwrap();
        assert!(players.contains(&"org.mpris.MediaPlayer2.spotify".to_string()));
        let resolved = resolve_player(None, &players).unwrap();
        assert_eq!(resolved, "org.mpris.MediaPlayer2.spotify");
    }

    #[tokio::test]
    async fn list_players_filters_prefix() {
        let b = MockBackend { names: vec!["org.mpris.MediaPlayer2.spotify".into(), "org.freedesktop.DBus".into(), ":1.42".into()], position: 0, status: "Playing".into() };
        let players = list_players_with(&b).await.unwrap();
        assert_eq!(players, vec!["org.mpris.MediaPlayer2.spotify"]);
    }
}
