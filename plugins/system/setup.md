# system plugin — operator setup

Optional host dependencies and permissions. The plugin degrades
gracefully: anything absent answers `ERR_SYS_NOT_SUPPORTED`, nothing here
is required for the rest to work.

## Optional host tools

| Tool | Package (Debian/Ubuntu) | Package (Arch) | Enables |
|---|---|---|---|
| `wpctl` | `wireplumber` | `wireplumber` | volume get/set/mute (PipeWire, preferred) |
| `pactl` | `pulseaudio-utils` | `libpulse` | volume fallback (PulseAudio / pipewire-pulse) |
| `brightnessctl` | `brightnessctl` | `brightnessctl` | brightness when sysfs write is denied |
| `power-profiles-daemon` | `power-profiles-daemon` | `power-profiles-daemon` | power profile get/set |
| `upower` | `upower` | `upower` | battery |
| `loginctl` | systemd (always present) | systemd | lock fallback |

macOS needs nothing extra — `pmset`, `osascript` and CGSession ship with
the OS.

## Backlight write permission

Direct `/sys/class/backlight/<device>/brightness` writes require the
plugin process to own the node. Without it the plugin falls back to
spawning `brightnessctl`, which itself needs the same permission — so on
a stock install both paths fail with `ERR_SYS_BACKEND`.

Grant write access to the `video` group with a udev rule:

```udev
# /etc/udev/rules.d/90-backlight.rules
ACTION=="add", SUBSYSTEM=="backlight", \
  RUN+="/bin/chgrp video /sys/class/backlight/%k/brightness", \
  RUN+="/bin/chmod g+w /sys/class/backlight/%k/brightness"
```

Then add the user running the kernel to `video` and reload:

```sh
sudo usermod -aG video <user>
sudo udevadm control --reload && sudo udevadm trigger
```

## Sandbox note

The plugin must run with `sandbox: false`: it talks to the **system**
D-Bus (UPower, power-profiles-daemon) and spawns host binaries argv-only.
It opens no network sockets.
