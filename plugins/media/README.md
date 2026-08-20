# media plugin

Local MPRIS media playback control for vynkor plugins. v1 is offline: it
drives `org.mpris.MediaPlayer2.*` players on the session D-Bus (Spotify,
mpv, VLC, browsers, …) and never opens a network connection, so it
declares no permissions.

## Status

v1 — local MPRIS via `zbus` session bus. All 10 actions are wired through
`MprisBackend` (real `RealBackend` + `MockBackend` for tests). Parsing covers
`xesam:title/artist/album`, `mpris:length/trackid/artUrl`, `PlaybackStatus`,
`Volume`, `Position` with `ERR_MEDIA_*` error taxonomy.

## D-Bus interfaces used

| Interface | Purpose |
|---|---|
| `org.freedesktop.DBus` | enumerate `org.mpris.MediaPlayer2.*` names on the session bus |
| `org.mpris.MediaPlayer2` | player identity (`Identity`), `Raise`/`Quit` |
| `org.mpris.MediaPlayer2.Player` | playback control (`Play`/`Pause`/`PlayPause`/`Next`/`Previous`/`Stop`/`Seek`/`SetPosition`) + `Metadata`/`Volume`/`Position`/`PlaybackStatus` properties |

## Actions

| Action | Params | Result |
|---|---|---|
| `media_play` | `player?`, `uri?` | `{ ok }` |
| `media_pause` | `player?` | `{ ok }` |
| `media_play_pause` | `player?` | `{ ok, playing }` |
| `media_next` | `player?` | `{ ok }` |
| `media_prev` | `player?` | `{ ok }` |
| `media_stop` | `player?` | `{ ok }` |
| `media_seek` | `position_ms` (≥0), `player?` | `{ position_ms }` |
| `media_volume` | `level` (0.0–1.0 or 0–100), `player?` | `{ volume }` |
| `media_status` | `player?` | `{ player, status, metadata, volume, position_ms }` |
| `media_list_players` | — | `{ players: [..] }` |

## Config

Config is read from the environment (the kernel's plugin supervisor
translates the `config.yaml` `env:` entry into these before spawning the
plugin).

| Env var | Default | Meaning |
|---|---|---|
| `MEDIA_PLUGIN_PLAYERS` | — (allow all) | comma-separated allowlist of MPRIS player names (`org.mpris.MediaPlayer2.<name>`) this plugin may control |
| `MEDIA_PLUGIN_DEFAULT_PLAYER` | — (first player) | player used when a request omits `player` |
