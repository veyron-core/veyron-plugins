# clipboard plugin roadmap

Text-only system clipboard access via host binaries — one blessed path for
`read`/`write` over the detected graphical session.

## v1 — shipped (0.1.0, local-only)

- `clipboard_read` / `clipboard_write` / `clipboard_providers`.
- Wayland: `wl-paste --no-newline` / `wl-copy`; X11: `xclip` → `xsel`
  fallback chains. Session detection: `WAYLAND_DISPLAY` →
  `XDG_SESSION_TYPE=x11` → `DISPLAY`; operator override via
  `CLIPBOARD_PLUGIN_PROVIDER`.
- argv-only spawn, never a shell (notify precedent). Size cap
  (`CLIPBOARD_PLUGIN_MAX_BYTES`, default 1 MiB) and per-spawn timeout
  (`CLIPBOARD_PLUGIN_TIMEOUT_MS`, default 5s).
- `Runner` trait boundary (`RealRunner` / `FakeRunner`) — 21 tests, no real
  compositor needed in CI.
- Declares `PERMISSION_CLIPBOARD` (proto v1.4, value 16).

## Later (unscheduled)

- `clipboard_clear` — only once a reliable cross-backend story exists
  (Wayland has no standard clear; xclip clears only the selection it owns).
  Empty writes are rejected today to keep "clear" an explicit future action.
- Non-text MIME (images/HTML) behind a feature flag — needs a different
  transport than argv/stdin and a size policy of its own.
- Clipboard history / multi-slot — out of scope; the kernel has no storage
  surface for it and `database` covers persistence if a caller wants it.
- Primary-selection support (`--primary` / `-selection primary`) if a caller
  actually needs it.

## Non-goals

- No network sync between machines.
- No daemon/watch mode — reads are on-demand spawns; a watch loop would need
  the calendar-style select loop first (see `plugins/media/ROADMAP.md` v1.2).
- No new `PermissionType` enum value — `PERMISSION_CLIPBOARD` already exists.

## References

- wl-clipboard: https://github.com/bugaevc/wl-clipboard
- xclip: https://github.com/astrand/xclip
