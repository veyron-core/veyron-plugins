# system plugin roadmap

Local host state: queries (P1, shipped) then simple reversible setters
(P2+). One permission (`PERMISSION_SYSTEM`), one domain — the machine this
plugin runs on. The root `ROADMAP.md` Planned table carries the cross-plugin
picture; this file is the per-plugin detail.

## P1 scope — shipped (0.1.0)

Read-only actions, all parameterless:

- `sys_info` — hostname/os/os_version/kernel/arch via `sysinfo`.
- `sys_battery` — UPower `DisplayDevice` on the system bus (`zbus`),
  aggregated reading like desktop applets show; UPower pending-charge
  states fold into their direction; `-1` times map to `null`.
- `sys_procs` — process count, load averages, memory via `sysinfo`.
- `sys_volume` — default sink volume/mute: `wpctl` → `pactl` fallback
  chain over argv-only spawns with a 5 s timeout; provider probed once at
  startup (`--version`), missing tools degrade to
  `ERR_SYS_NOT_SUPPORTED`.

Architecture notes:

- Domain traits (`Battery`, `Volume`) + runtime detection fill a
  `SystemBackends`; every absent capability is an explicit `None` →
  `ERR_SYS_NOT_SUPPORTED naming the capability` — same graceful shape as
  media's capability guards.
- `CommandRunner` trait (real/fake) is the test seam for spawn-based
  backends, copied from clipboard's `Runner`.
- Stock SDK serve loop — no outbound RPC, so the single-reader rule never
  applies.

## P2 — reversible setters, shipped (0.2.0)

- `sys_volume_set { percent }`, `sys_volume_mute { mode }` through the same
  wpctl/pactl chain; both return the resulting reading.
- `sys_lock` — `org.freedesktop.ScreenSaver.Lock` (session bus) first,
  `loginctl lock-session` broadcast as fallback; always detected on Linux
  since neither path has a cheap presence probe.
- `sys_brightness` / `sys_brightness_set { percent }` — direct
  `/sys/class/backlight` write with `brightnessctl` fallback on EACCES;
  **set(0) clamps to the minimum non-blanking step** so the plugin can
  never strand the operator on a black screen. udev rule / optional
  packages doc still pending (see P4).
- `sys_power_profile` / `sys_power_profile_set` — power-profiles-daemon
  D-Bus, probing both the renamed
  (`org.freedesktop.UPower.PowerProfiles`) and legacy (`net.hadess`)
  name/path pairs; TLP-only hosts report NOT_SUPPORTED.
- Fake-kernel end-to-end harness (UnixStream::pair shim): registration
  handshake, action roundtrip, and wire-level error statuses
  (`ACTION_ERROR` vs `ACTION_NOT_FOUND`).

Remaining from the original P2 list: none.

## P3 — macOS subset, shipped (0.3.0)

- Battery via `pmset -g batt` parse; desktop Macs without a battery fail
  the startup probe → NOT_SUPPORTED, same semantics as missing UPower.
- Volume via `osascript` volume settings; mute `toggle` is a
  read-then-write inverse (AppleScript has no toggle primitive).
- Lock via CGSession `-suspend`.
- `sys_info`/`sys_procs` already worked — `sysinfo` is cross-platform.
- Structure: pure output parsers live in non-gated `macos_parse.rs`
  (CI-tested on Linux against fixtures); only spawn wiring sits behind
  `cfg(target_os = "macos")`. The system-bus handle is an opaque struct so
  zbus never leaks into cross-platform signatures (fixes a latent P1
  compile-on-macOS hazard).
- Brightness/night light stay Linux-only.

## Non-goals

- **No destructive actions** — kill process, delete files, network config:
  different trust level, would make this shell-lite (rejected ecosystem-
  wide). If a need appears, it becomes its own plugin + permission.
- **No night light before v1.1** — every DE does it differently
  (gsettings / KDE / gammastep); defer until there's a consumer.
- **No watching/polling loops** — metrics history belongs to the planned
  `metrics` plugin; this plugin answers one-shot queries.
- **No per-action permission split** — everything here is one trust
  domain; splitting into micro-permissions adds enum values without
  adding safety.
