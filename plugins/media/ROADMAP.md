# media plugin roadmap

Local MPRIS playback control — one blessed path for `play/pause/seek/volume/status/list` over the session D-Bus.

## v1 — shipped (0.1.0, local-only, no permissions)

10 actions behind `org.mpris.MediaPlayer2.*`:

- `media_list_players` — `ListNames` filtered `org.mpris.MediaPlayer2.*`, sorted, allowlist `MEDIA_PLUGIN_PLAYERS`
- `media_status {player?}` — `PlaybackStatus + Metadata + Volume + Position` via `Properties.Get`
- `media_play {player?, uri?}` — `OpenUri?` + `Play`
- `media_pause / media_play_pause / media_next / media_prev / media_stop` — direct `Player.*`
- `media_seek {position_ms, player?}` — `Seek(offset)` where `offset = target - Position`
- `media_volume {level, player?}` — `Properties.Set Volume` (0.0-1.0, accepts 0-100 int% via `main.rs:parse_volume`)

`zbus 4` async, `RealBackend` + `MockBackend`, 8 unit tests (parsing + mock), `permissions: []` (pure local IPC).

## Known bugs — fix next (see BUGS.md)

- Firefox YouTube `Position` always 0 after seek — `Seek` with large offset clamps to 0, `SetPosition(o x)` fails `Invalid argument` on Firefox's synthetic trackId. Need fallback: try `SetPosition` with `TrackId` from Metadata, else `Seek`.
- `media_status` position stale when `Position` property is 0 — should extrapolate `Position + Rate*(now - lastUpdate)` or poll `GetPosition` via `PropertiesChanged` subscription.
- `media_play_pause` reports `playing` from immediate `PlaybackStatus` read — race (status updates async via `PropertiesChanged`). Should await signal or retry after 100ms.
- Allowlist `MEDIA_PLUGIN_PLAYERS` parsed on every call via `std::env::var` — cache or reload via SIGHUP would be cleaner.
- No event publishing (`media.state_changed` via `PERMISSION_EVENT_PUBLISH`) — `agent` has to poll.

## v1.1 — polish (no kernel change)

- Fix `seek`: `SetPosition(trackId, pos_us)` primary, `Seek(delta)` fallback. TrackId normalization: `mpris:trackid` is `ObjectPath`, not plain string.
- Add `PropertiesChanged` listener (zbus `Stream` for `org.freedesktop.DBus.Properties.PropertiesChanged` on `Player`), publish `media.state_changed` when declared `PERMISSION_EVENT_PUBLISH` (opt-in).
- Handle `CanSeek/CanControl/CanPause` guards before calling — return `ERR_MEDIA_NOT_SUPPORTED` instead of forwarding raw D-Bus error.
- Cover `media_*` with `bytes_written` style negative tests: empty `x:artist`, `x:artist` as single string vs array, `mpris:length` as `i64` vs `u64` vs `Variant`.

## v1.2 — MPD + mpv hardening

- MPD advertises `NoTrack` (`/TrackList/NoTrack`) when stopped — `seek` must reject on that trackId with `ERR_MEDIA_NO_TRACK`.
- Add `media_queue`/`media_playlist` (TrackList `GetTracksMetadata` + `AddTrack`/`RemoveTrack`) for MPD, gated behind `MEDIA_PLUGIN_ENABLE_TRACKLIST=false` default (browsers don't implement TrackList).

## v2 — remote providers (requires `network` + `secrets`)

- `media_search {query, limit}` + `media_play {uri}` for Spotify/YouTube Music via `network.http_request`, keys from `secrets` vault (`SPOTIFY_ACCESS_TOKEN`, `YOUTUBE_API_KEY` via `SECRETS_PLUGIN_MASTER_KEY`). Adds `PERMISSION_NETWORK` + `PERMISSION_SECRETS` (caller of gated actions).
- System volume (`pactl`/`wpctl`) via `PERMISSION_SYSTEM` optional mode — currently `media_volume` only touches MPRIS player volume, not host mixer.
- `config_schema` additions: `MEDIA_PLUGIN_SECRETS_ALLOWLIST`, `MEDIA_PLUGIN_SPOTIFY_MARKET` etc.

## Non-goals

- No audio streaming — `tts_speak` already streams Opus `AudioStreamChunk`s; `media` is control-plane only.
- No window focus/raise — `window` plugin will handle `Raise`.
- No new `PermissionType` enum value — local MPRIS needs none; remote mode reuses `network`/`secrets`.

## References

- MPRIS 2.2: https://specifications.freedesktop.org/mpris-spec/latest/
- zbus 4.4: `Connection::session`, `DBusProxy::list_names`, `PropertiesProxy::get/set`
