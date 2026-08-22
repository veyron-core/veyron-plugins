# sound plugin roadmap

Audio output primitive — the single owner of the speakers. `tts`
synthesizes but doesn't play host-side; `notify`'s `speak:true` built-in
player migrates here eventually.

## v1 — shipped (0.1.0, local-only)

- `sound_play` / `sound_stop` / `sound_status`; background playback,
  replace-on-play, idempotent stop, lazy reap (no watcher task).
- Chains: wav → `pw-cat --playback` → `paplay` → `aplay`; non-wav →
  `ffplay`. Capability-aware auto filtering (volume/device), operator pin
  via `SOUND_PLUGIN_PLAYER`.
- argv-only spawn (clipboard/notify precedent); inline audio via capped,
  format-validated temp files. No new permission — `PERMISSION_AUDIO`
  exists.
- `Spawner`/`Process` trait boundary (`RealSpawner` / `FakeSpawner`) —
  unit tests plus fake-kernel wire tests over `UnixStream::pair`; no real
  audio stack in CI.

## Next

- **Migrate `notify`'s `speak:true` player here** — notify currently
  resolves a player and manages temp files itself (`providers.rs::
  pick_player` + `speak_via_tts`). Once it calls `sound_play`, the plugin
  becomes the only process that touches the speakers.
- Ducking hook: while a clip plays, lower `media` MPRIS volume (needs a
  small outbound-call story or an event subscription on `media`).
- Exit-code surfacing in `sound_status` (player failed vs finished) once a
  consumer needs to distinguish them.

## Later (unscheduled)

- Queue/mix modes — deliberately absent in v1; single-owner keeps agent
  semantics predictable (last call wins). Revisit only with a real
  multi-clip consumer.
- Recording/capture — belongs to `capture` (PipeWire stack), not here.
- Streaming input (`FLAG_RAW_BINARY`) — the kernel supports audio frames
  (D-12) for `tts_speak`/`stt_listen`; feeding them straight into a player
  is a natural v2 if latency demands it.

## Non-goals

- No synthesis — that's `tts`; this plugin only outputs bytes/files.
- No mixer/volume-of-the-system control — `system` owns `sys_volume`.
- No network streaming URLs — local files and inline bytes only; fetching
  is `network`'s job and would need its gated `http_request` (T-19).
- No new `PermissionType` enum value — `PERMISSION_AUDIO` already exists.

## References

- pw-cat: https://docs.pipewire.org/page_man_pw-cat_1.html
- paplay: https://manpages.debian.org/paplay(1)
- ffplay: https://ffmpeg.org/ffplay.html
