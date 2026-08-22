# sound plugin

Audio output primitive for vynkor plugins — the single owner of the
speakers. `sound_play` spawns a well-known host player binary directly with
argv (never a shell) and returns immediately; clips play in the background.
`sound_stop` kills the current clip (or a specific one), `sound_status`
reports what is playing. Declares `PERMISSION_AUDIO` (existing enum value 5).

## Status

v0.1.0 — `sound_play` / `sound_stop` / `sound_status`. Local-only, offline.

## Providers

| Format | Chain (first working wins) | Notes |
|---|---|---|
| wav | `pw-cat --playback` → `paplay` → `aplay -q` | PipeWire, then PulseAudio, then bare ALSA |
| everything else | `ffplay` (with `-nodisp -autoexit`) | ffmpeg decodes anything |

A missing binary falls through to the next in the chain; all missing →
`ERR_SOUND_PROVIDER_MISSING` listing what was tried. Pin one backend with
`SOUND_PLUGIN_PLAYER`.

Capability filtering in auto mode: requesting `volume` drops backends with
no native volume flag (`aplay`); requesting `device` drops `ffplay` (no
sane device-targeting flag). If nothing supports the combination the error
names the conflict instead of silently ignoring it.

## Actions

| Action | Params | Result |
|---|---|---|
| `sound_play` | exactly one of `file` (absolute path) or `data_base64` + `format`; optional `volume` (0–10 linear, default 1), `device` | `{ ok, clip_id, player, replaced }` — returns as soon as the player is spawned |
| `sound_stop` | optional `clip_id` (omit = stop everything) | `{ stopped: [ids] }` — idempotent |
| `sound_status` | — | `{ playing: [{id, source, player}], count }` |

Playback model:

- **Non-blocking**: `sound_play` returns once the player process exists;
  playback continues in the background.
- **Single owner of the speakers**: starting a new clip stops whatever was
  playing first (`replaced: true`). No queuing, no mixing.
- **Inline audio** is base64-decoded, written to a temp file
  (`/tmp/sound-<pid>-<millis>.<format>`), and deleted when the clip is
  reaped after finishing or being stopped.
- **Reaping** happens lazily on every action — a finished clip disappears
  from status on the next interaction; no background watcher task.
- Shutdown stops all clips best-effort; players spawn detached with stdio
  null-routed and `kill_on_drop`, so no zombies outlive the plugin.

Volume mapping: linear multiplier passed natively — pw-cat/paplay
`--volume=<f>` (1 = unchanged), ffplay `-volume <int percent>`; aplay has no
volume support (dropped from the chain when volume ≠ 1). Device mapping:
pw-cat `--target=`, paplay `--device=`, aplay `-D`.

## Error taxonomy

`ERR_SOUND_BAD_PARAMS` / `TOO_LARGE` / `SOURCE_UNREADABLE` /
`PROVIDER_MISSING` / `SPAWN_FAILED` / `INTERNAL`.

## Security model

- argv-only spawn of well-known binaries; no shell, so paths and format
  strings are never interpreted.
- `file` must be an absolute path (the plugin's CWD is kernel-dependent);
  existence and size are checked before spawn. Playing an arbitrary
  readable file is equivalent to reading it via `filesystem` permissions —
  the gate for this plugin is `PERMISSION_AUDIO`.
- Inline audio size cap (`SOUND_PLUGIN_MAX_BYTES`, default 32 MiB) applied
  before writing any temp file; format string validated to short
  alphanumeric before it can touch a filename.
- No timeout by design: clips may legitimately run long; stopping is
  explicit via `sound_stop`.

## Config

Config is read from the environment (the kernel's plugin supervisor
translates the `config.yaml` `env:` entry into these before spawning the
plugin).

| Env var | Default | Meaning |
|---|---|---|
| `SOUND_PLUGIN_PLAYER` | *(unset)* | Pin one backend binary (skips capability filtering) |
| `SOUND_PLUGIN_DEVICE` | *(unset)* | Default output device; per-call `device` param wins |
| `SOUND_PLUGIN_MAX_BYTES` | `33554432` | Cap on source size in bytes (file stat or decoded inline audio) |
