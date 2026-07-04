# veyron-plugins

Plugins for the [Veyron](https://github.com/veyron-core/veyron) plugin
kernel.

## Plugins

| Plugin | Path | Permissions | Description |
|---|---|---|---|
| `ping-pong` | `plugins/ping-pong-rs/` | none | Minimal example plugin that responds to ping actions. |
| `network` | `plugins/network/` | `PERMISSION_NETWORK` | Outbound HTTP for plugins/kernel via one `http_request` action. HTTP-only v1 (no WebSocket). See `plugins/network/README.md`. |

## Registry

`registry.json` lists released/published plugin archives (marketplace
metadata: version, archive URL, sha256). A plugin only gets an entry once
it's packaged and released — see each plugin's own README for its current
status.
