# Changelog — media plugin

All notable changes to `media` follow Keep a Changelog + SemVer. `plugin.json` `version` mirrors `Cargo.toml`.

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
