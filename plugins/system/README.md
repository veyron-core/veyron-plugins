# system plugin

Local host queries and simple reversible controls for vynkor plugins —
battery, processes, memory, output volume, backlight, session lock, power
profile, OS identity. One domain: the state of the machine this plugin runs
on. v0.2.0 ships the P2 setter wave behind the P1 read-only dispatch.
Declares `PERMISSION_SYSTEM` (broad access by design — keep strict, see
root `ROADMAP.md`).

Deliberately out of scope forever: destructive or differently-scoped
actions (kill process, network config, device management). Those would turn
this into shell-lite, which was rejected at the ecosystem level.

## Operator note

`system` needs `sandbox: false`: it talks to the **system** D-Bus (UPower)
and spawns host binaries (`wpctl`/`pactl`) argv-only — never a shell, same
delivery model as `clipboard`/`notify`. Every spawn is bounded by a 5 s
timeout.

Backends are probed once at startup; anything undetected answers
`ERR_SYS_NOT_SUPPORTED` naming the capability instead of failing obscurely.
No configuration required.

## Backends

| Capability | Linux | macOS |
|---|---|---|
| `sys_info`, `sys_procs` | `sysinfo` crate (cross-platform) | same |
| `sys_battery` | UPower `DisplayDevice` on the system bus (`zbus`) | P3: `pmset -g batt` parse |
| `sys_volume[_set/_mute]` | `wpctl` (PipeWire) → `pactl` (PulseAudio/pipewire-pulse) fallback chain | P3 |
| `sys_brightness[_set]` | `/sys/class/backlight` write → `brightnessctl` fallback on EACCES | — |
| `sys_lock` | `org.freedesktop.ScreenSaver.Lock` → `loginctl lock-session` chain | P3 |
| `sys_power_profile[_set]` | power-profiles-daemon D-Bus (both name/path generations) | — |

Safety contract: `sys_brightness_set {percent: 0}` clamps to the device's
minimum non-blanking step — the plugin can darken but never blank your
screen.

Distro note: Arch/Debian/Fedora need nothing special — UPower,
power-profiles-daemon and PipeWire/PulseAudio are freedesktop standards.
What matters is the session stack, not the distro.

## Actions

Getters are parameterless; non-empty params on them (or malformed params
anywhere) are rejected with `ERR_SYS_BAD_PARAMS`. Setters return the
resulting reading, not an echo of the request.

| Action | Params | Result |
|---|---|---|
| `sys_info` | — | `{ hostname, os, os_version, kernel, arch }` |
| `sys_battery` | — | `{ percent, state, time_to_empty_s, time_to_full_s }` — `state ∈ unknown/charging/discharging/empty/full`; times are seconds or `null` when unknown |
| `sys_procs` | — | `{ process_count, load_avg: [1m,5m,15m], memory_total_mb, memory_used_mb }` |
| `sys_volume` | — | `{ percent, muted }` — default sink volume 0–100 |
| `sys_volume_set` | `percent` (0–100) | `{ percent, muted }` after the change |
| `sys_volume_mute` | `mode`: `on`\|`off`\|`toggle` | `{ percent, muted }` after the change |
| `sys_brightness` | — | `{ percent }` |
| `sys_brightness_set` | `percent` (0–100, 0 = min non-blanking step) | `{ percent }` after the change |
| `sys_lock` | — | `{ ok }` — ScreenSaver first, logind broadcast fallback |
| `sys_power_profile` | — | `{ profile, available: [...] }` |
| `sys_power_profile_set` | `profile`: `performance`\|`balanced`\|`power-saver` | `{ profile, available }` after the switch |

## Error taxonomy

| Code | Meaning |
|---|---|
| `ERR_SYS_BAD_PARAMS` | Params present but this action takes none / malformed JSON |
| `ERR_SYS_NOT_FOUND` | Unknown action name (also surfaced as `ACTION_NOT_FOUND` status) |
| `ERR_SYS_NOT_SUPPORTED` | No backend detected for the capability on this host |
| `ERR_SYS_BACKEND` | A detected backend failed at call time (D-Bus error, spawn failure, unparseable tool output) |

Nothing sensitive is ever embedded in error text — only interface names and
tool output.

## Testing

`cargo test` — 37 unit + 4 end-to-end tests, no real desktop needed:
pure parsers against fixture outputs (wpctl/pactl variants), UPower
state/time mapping, sysfs brightness against tmpdir fixtures (including the
EACCES → brightnessctl fallback), dispatch with fake backends,
provider-selection chains via a fake runner — plus a fake-kernel harness
(`UnixStream::pair`) driving the real serve loop through registration and
wire-level error statuses.
