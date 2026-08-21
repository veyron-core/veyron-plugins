# system plugin

Local host queries for vynkor plugins — battery, processes, memory, output
volume, OS identity. One domain: the state of the machine this plugin runs
on. P1 (0.1.0) is **read-only**; simple reversible setters
(volume/mute/brightness/lock/power-profile) follow in P2 behind the same
dispatch. Declares `PERMISSION_SYSTEM` (broad access by design — keep
strict, see root `ROADMAP.md`).

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
| `sys_volume` | `wpctl` (PipeWire) → `pactl` (PulseAudio/pipewire-pulse) fallback chain | P3 |

Distro note: Arch/Debian/Fedora need nothing special — UPower,
power-profiles-daemon and PipeWire/PulseAudio are freedesktop standards.
What matters is the session stack, not the distro.

## Actions

All actions are parameterless (empty JSON object); non-empty params are
rejected with `ERR_SYS_BAD_PARAMS`.

| Action | Params | Result |
|---|---|---|
| `sys_info` | — | `{ hostname, os, os_version, kernel, arch }` |
| `sys_battery` | — | `{ percent, state, time_to_empty_s, time_to_full_s }` — `state ∈ unknown/charging/discharging/empty/full`; times are seconds or `null` when unknown |
| `sys_procs` | — | `{ process_count, load_avg: [1m,5m,15m], memory_total_mb, memory_used_mb }` |
| `sys_volume` | — | `{ percent, muted }` — default sink volume 0–100 |

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

`cargo test` — 24 unit tests, no real desktop needed: pure parsers against
fixture outputs (wpctl/pactl variants), UPower state/time mapping, dispatch
with fake backends, provider-selection chains via a fake runner. The SDK
serve loop is exercised upstream; P2 adds the fake-kernel end-to-end harness
when parameterized setters arrive.
