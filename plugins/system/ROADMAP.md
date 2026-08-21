# system plugin roadmap

Local host state: queries (P1, shipped) then simple reversible setters
(P2+). One permission (`PERMISSION_SYSTEM`), one domain — the machine this
plugin runs on. The root `ROADMAP.md` Planned table carries the cross-plugin
picture; this file is the per-plugin detail.

## v1 scope — P1 shipped (0.1.0)

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

## P2 — reversible setters

- `sys_volume_set { percent }`, `sys_volume_mute { enabled | toggle }`
  through the same wpctl/pactl chain.
- `sys_lock` — logind `LockSession` → org.freedesktop.ScreenSaver.Lock
  fallback.
- `sys_brightness_set { percent }` — direct `/sys/class/backlight` write,
  falling back to spawn `brightnessctl` on EACCES; ship the udev rule in
  `setup.md` and optional packages in `assets/dependencies.json`.
- `sys_power_profile_get/set` — power-profiles-daemon D-Bus
  (performance/balanced/power-saver); TLP-only hosts report NOT_SUPPORTED.
- Fake-kernel end-to-end harness (UnixStream::pair shim) for the
  parameterized actions.

## P3 — macOS subset

- Battery via `pmset -g batt` parse (IOKit later if needed).
- Volume via `osascript` spawn (CoreAudio native later).
- Lock via CGSession suspend.
- `sys_info`/`sys_procs` already work — `sysinfo` is cross-platform.
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
