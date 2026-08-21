# Changelog — media plugin

All notable changes to `media` follow Keep a Changelog + SemVer. `plugin.json` `version` mirrors `Cargo.toml`.

## [0.0.3] — 2026-08-21 — v1.1 polish (13 actions, 42 tests)

### Added
- `media_seek_relative {offset_ms}` — signed offset from current position, clamps at 0; shares the absolute-seek core with `media_seek`.
- Capability guards: `CanPlay`/`CanPause`/`CanGoNext`/`CanGoPrevious`/`CanSeek`/`CanControl` checked before calls → `ERR_MEDIA_NOT_SUPPORTED` naming the property; only explicit `false` blocks.
- Background signal watcher (`spawn_watch_task`, one task per player): subscribes `PropertiesChanged` + `Seeked(int64)`, feeds `POS_CACHE` `(pos, rate, updated_at)` via `cache_note` — full BUG-2 fix for compliant players.
- Window-capped extrapolation: `EXTRAPOLATION_MAX_AGE_MS` (120s); MPRIS needs no periodic Position updates, so older samples are not trusted.
- Tests: negative metadata parsing (empty/mixed/wrong-typed artist, small-int/float/negative lengths), guard paths for every capability, seek_relative forward/backward/clamp, error classification unit tests, signal-fed cache + extrapolation window. 42 total (was 14).

### Fixed
- D-Bus errors reclassified (`classify_dbus`): `No such property`/`UnknownProperty`/`UnknownMethod`/`NotSupported` → `ERR_MEDIA_NOT_SUPPORTED` (was misreported as `PLAYER_VANISHED`, e.g. firefox Shuffle).
- `parse_length_value` rejects negative lengths; mixed-type artist arrays unwrap `Value::Value`-wrapped elements.

### Deferred
- `media.state_changed` event publication → v1.2 loop migration (needs `PERMISSION_EVENT_PUBLISH` + outbound channel; single-reader rule on the sequential SDK loop).

## [0.0.2] — 2026-08-20 — fix/media-bugs-v1 (`fix/media-bugs-v1` branch) — originally 0.2.0, re-tagged as 0.0.2

### Added
- `MprisBackend::set_position`, `get_rate`, `get_shuffle/set_shuffle`, `get_loop_status/set_loop_status` (zbus `PropertiesProxy` on `org.mpris.MediaPlayer2.Player`).
- Actions `media_shuffle {enabled}` and `media_loop {mode}` (manifest + `plugin.json`).
- `media_status` extra fields `rate`, `shuffle`, `loop_status` (additive, 0.1.0 clients ignore).
- Metadata cache `META_CACHE` + `merge_metadata_cached` (BUG-4), position cache `POS_CACHE` + `extrapolate_position` (partial BUG-2).
- Tests: `parse_length_u32`, `seek_no_track`, `seek_set_position`, `status_shuffle_loop_rate`, `metadata_cache_keeps_length` (14 total, was 8).

### Fixed
- `parse_volume` int `1` → `0.01` (was `1.0` via `as_f64` shadowing `as_u64`) — critical.
- `seek_with` overflow `checked_mul`, `NoTrack` guard `ERR_MEDIA_NO_TRACK`, `SetPosition` primary (ObjectPath) + `Seek` fallback on `NotSupported`.
- `status_with` no longer swallows `PLAYER_VANISHED`; unified `ERR_MEDIA_*` taxonomy.
- `play_pause` race → 50/100/150ms poll.
- `parse_metadata` length now `i64/u64/i32/u32/...`.
- `player`/`uri` param validation `ERR_MEDIA_BAD_PARAMS` instead of silent default.

### Verified (real D-Bus, 2026-08-20)
- `firefox.instance_1_424` YouTube + `mpd` (`кис-кис - падик.mp3`) + `TelegramDesktop`. `list/status/volume/play_pause/mpd seek` OK. `firefox seek` still upstream broken (BUG-1 high) — documented.

### Remaining
- BUG-1 Firefox seek (high), BUG-2 full `PropertiesChanged` subscription → `media.state_changed` event (needs `PERMISSION_EVENT_PUBLISH`).

## [0.2.0] — 2026-08-20 — same as 0.0.2 (kept for history, not published)

### Added
- `MprisBackend::set_position`, `get_rate`, `get_shuffle/set_shuffle`, `get_loop_status/set_loop_status` (zbus `PropertiesProxy` on `org.mpris.MediaPlayer2.Player`).
- Actions `media_shuffle {enabled}` and `media_loop {mode}` (manifest + `plugin.json`).
- `media_status` extra fields `rate`, `shuffle`, `loop_status` (additive, 0.1.0 clients ignore).
- Metadata cache `META_CACHE` + `merge_metadata_cached` (BUG-4), position cache `POS_CACHE` + `extrapolate_position` (partial BUG-2).
- Tests: `parse_length_u32`, `seek_no_track`, `seek_set_position`, `status_shuffle_loop_rate`, `metadata_cache_keeps_length` (14 total, was 8).

### Fixed
- `parse_volume` int `1` → `0.01` (was `1.0` via `as_f64` shadowing `as_u64`) — critical.
- `seek_with` overflow `checked_mul`, `NoTrack` guard `ERR_MEDIA_NO_TRACK`, `SetPosition` primary (ObjectPath) + `Seek` fallback on `NotSupported`.
- `status_with` no longer swallows `PLAYER_VANISHED`; unified `ERR_MEDIA_*` taxonomy.
- `play_pause` race → 50/100/150ms poll.
- `parse_metadata` length now `i64/u64/i32/u32/...`.
- `player`/`uri` param validation `ERR_MEDIA_BAD_PARAMS` instead of silent default.

### Verified (real D-Bus, 2026-08-20)
- `firefox.instance_1_424` YouTube + `mpd` (`кис-кис - падик.mp3`) + `TelegramDesktop`. `list/status/volume/play_pause/mpd seek` OK. `firefox seek` still upstream broken (BUG-1 high) — documented.

### Remaining
- BUG-1 Firefox seek (high), BUG-2 full `PropertiesChanged` subscription → `media.state_changed` event (needs `PERMISSION_EVENT_PUBLISH`).

## [0.1.0] — 2026-08-19 — local MPRIS v1

- Initial `zbus 4` async, `RealBackend` + `MockBackend`, 10 actions, 8 tests, `permissions: []`.
