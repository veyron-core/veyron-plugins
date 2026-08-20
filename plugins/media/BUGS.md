# media plugin — known bugs / tech debt

> File intentionally kept inside the plugin tree so future fixes can link commits to entries here. Each entry has `scope`, `repro`, `expected`, `actual`, `root cause` (if known) and `fix idea`. Triaged by severity.

## BUG-1 — Seek on Firefox/YouTube resets to 0 (high)

- **Scope:** `src/mpris.rs:seek_with` + `RealBackend::seek_offset`
- **Repro:** `media_status` @ `pos 383000` → `media_seek {position_ms:388000}` (target +5s) → `media_status` shows `pos 0`, not `388000`. `busctl ... Seek x 5000000` → `Position x 0`. `SetPosition o x "/org/mpris/MediaPlayer2/firefox" 380000000` → `Invalid argument`.
- **Expected:** absolute seek to `target_ms`. Firefox should end at ~`target`.
- **Actual:** `Position` becomes 0; `Length` flips `423000000 → null`.
- **Root cause:** current impl uses `Seek(offset)` only (`offset = target - current`). Firefox/Chromium MPRIS for HTML5 `<video>` treats `Seek` as relative but clamps large jumps to 0; `SetPosition` expects a *real* `TrackId` object path that Firefox synthesizes (`/org/mpris/MediaPlayer2/firefox`) but validates differently per video element — our `Value::Str` → `ObjectPath` conversion may be wrong, and we pass the path as plain string not `o`.
- **Fix:** Try `SetPosition(trackId, pos_us)` first where `trackId` is parsed `mpris:trackid` as `OwnedObjectPath` (validate via `ObjectPath::try_from`), fallback to `Seek(delta)` only when `trackId == "/TrackList/NoTrack"` or `SetPosition` returns `UnknownMethod`/`NotSupported`. Add regression test with mock `SetPosition` vs `Seek`.
- **Files:** `src/mpris.rs:106-150` (add `set_position` to `MprisBackend`), `src/mpris.rs:365-380`.

## BUG-2 — `media_status` shows stale Position 0 when Position property is 0 (medium)

- **Scope:** `status_with` reads `Position` synchronously via `Properties.Get`.
- **Repro:** Firefox after `media_seek` → `media_status` `position_ms 0` even though YouTube UI shows 6:23.
- **Expected:** `position_ms` ≈ `elapsed since Play` (MPRIS spec says `Position` is in microseconds, monotonic while Playing; many browsers update it via `PropertiesChanged` only).
- **Actual:** `Get(Position)` returns 0 until next `PropertiesChanged` event; we never subscribe.
- **Fix:** Subscribe to `PropertiesChanged` on `org.mpris.MediaPlayer2.Player` (zbus `PropertyStream`), cache `(position, rate, updated_at)` and extrapolate `position + rate*(now-updated_at)` in `status_with`. Or poll `Position` twice with 200ms delay. Needs `PERMISSION_EVENT_PUBLISH` if we publish `media.state_changed`.

## BUG-3 — `media_play_pause` race reports wrong `playing` (low)

- **Scope:** `play_pause_with` calls `PlayPause` then immediate `get_playback_status`.
- **Repro:** Firefox Playing → `media_play_pause` → returns `{"ok":true,"playing":true}` but `playerctl status` flips to Paused 80ms later.
- **Expected:** `playing` matches post-toggle state.
- **Actual:** reads old `PlaybackStatus` before the player emits `PropertiesChanged`.
- **Fix:** Await `PropertiesChanged` for `PlaybackStatus` up to 300ms or retry `get_playback_status` with exponential backoff 2×50ms.

## BUG-4 — `Metadata` length missing on Firefox after Seek (low)

- **Scope:** `parse_metadata` — `mpris:length` absent after seek error.
- **Repro:** `media_status` before seek `length_micros 423000000` → after failed `Seek` `length_micros null`.
- **Expected:** length stable per track.
- **Actual:** Firefox clears `Metadata` on error Seek? Or our `get_metadata` failed and we `unwrap_or_default`.
- **Fix:** Keep last-known `Metadata` per player in an `Arc<Mutex<LruCache>>` and merge missing keys.

## TECH-1 — Allowlist parsed per call via `env::var` (debt)

- Env `MEDIA_PLUGIN_PLAYERS` read on every `resolve_player` via `std::env::var`. No caching, no hot-reload. OK for 10 RPS but breaks `SIGHUP` reload expectation.
- Fix: parse once in `on_init`, store in `Arc<Vec<String>>`, expose `media_reload_config` action.

## TECH-2 — Error taxonomy inconsistent

- Some paths return `ERR_MEDIA_BUS_UNAVAILABLE`, others plain `player 'x' failed: ...`. Kernel `ActionStatus::ACTION_ERROR` expects free-form string but callers grep for `ERR_MEDIA_*` prefix.
- Fix: wrap every D-Bus error as `ERR_MEDIA_PLAYER_VANISHED` / `ERR_MEDIA_NOT_SUPPORTED` / `ERR_MEDIA_BAD_PARAMS` consistently; add helper `err(prefix, context, source)`.

## TECH-3 — MPD `NoTrack` handling

- MPD when stopped returns `track_id "/TrackList/NoTrack"` and `length 155520000` even with nothing playing. `seek` should reject on `NoTrack` with `ERR_MEDIA_NO_TRACK` instead of computing offset against stale `Position 0`.

## IDEA — `media_events` subscription

- Subscribe to `Seeked(int64)` signal (`org.mpris.MediaPlayer2.Player.Seeked`) and `PropertiesChanged` to publish `media.seeked` / `media.state_changed` events. Requires declaring `PERMISSION_EVENT_PUBLISH` in manifest.

## IDEA — Extrapolate Rate

- MPRIS `Rate` is `double` (1.0 normal, 0.0 paused). Should multiply `Position` delta by `Rate` when extrapolating.

## How to reproduce locally

```bash
vyn start --config ~/projects/veyron-core/veyron/config.yaml
playerctl -l  # shows firefox.instance_1_424 + mpd
python3 - <<'PY'
import subprocess, json, time
from veyron_sdk import VeyronClient # or use /tmp/media_test
PY
busctl --user call org.mpris.MediaPlayer2.firefox.instance_1_424 /org/mpris/MediaPlayer2 org.mpris.MediaPlayer2.Player Pause
```
