# Changelog — clipboard plugin

All notable changes to `clipboard` follow Keep a Changelog + SemVer. `plugin.json` `version` mirrors `Cargo.toml`.

## [0.1.0] — 2026-08-21 — initial release

### Added
- Actions `clipboard_read` / `clipboard_write` / `clipboard_providers`.
- Wayland (`wl-paste --no-newline` / `wl-copy`) and X11 (`xclip` → `xsel`) provider chains; session detection from `WAYLAND_DISPLAY`/`XDG_SESSION_TYPE`/`DISPLAY`, override via `CLIPBOARD_PLUGIN_PROVIDER`.
- Security model: argv-only spawn (never a shell), size cap `CLIPBOARD_PLUGIN_MAX_BYTES` (default 1 MiB) enforced on both directions, per-spawn timeout `CLIPBOARD_PLUGIN_TIMEOUT_MS` (default 5s), empty writes rejected.
- Error taxonomy `ERR_CLIPBOARD_NO_SESSION / PROVIDER_MISSING / TIMEOUT / TOO_LARGE / BAD_PARAMS / READ_FAILED / WRITE_FAILED`.
- Manifest v2 with per-action `permission: "clipboard"` + input/output schemas + `config_schema`.
- 21 tests over the `Runner` trait boundary (`RealRunner` / `FakeRunner`) — no compositor required in CI.
