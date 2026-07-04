# network plugin

Outbound HTTP for Veyron plugins/kernel. Exposes one action, `http_request`,
guarded by `PERMISSION_NETWORK`. See
`docs/superpowers/specs/2026-07-05-network-plugin-design.md` for the full
design (request/response shape, guardrails, error mapping).

v1 is HTTP only — no WebSocket.

## Operator note

This plugin needs real network egress. In the kernel's `config.yaml`
`plugins:` entry for `network`, set `sandbox: false`. `sandbox: true` puts
the plugin in an isolated PID+net namespace with no route out
(`src/plugins/runner.rs`), which makes every `http_request` fail.

## Extra SSRF blocklist (operator-configurable)

Besides the built-in blocklist (loopback, RFC1918 private ranges,
link-local, multicast, broadcast, cloud metadata `169.254.169.254`), an
operator can block additional IPs and/or hostnames via the
`NETWORK_PLUGIN_EXTRA_BLOCKED_HOSTS` environment variable: a comma-separated
list, each entry either a literal IP address or a bare hostname (matched
case-insensitively against the request's host before DNS resolution).

Set it via the plugin's `env:` list in the kernel's `config.yaml` — see
`config.example.yaml` in this directory for a full example entry. Example:

```yaml
env:
  - NETWORK_PLUGIN_EXTRA_BLOCKED_HOSTS=10.99.0.5,internal-admin.corp,203.0.113.7
```

Both forms are enforced at the DNS resolver used for every connection
(initial request and any redirect hop) — see `src/handler.rs`'s
`SsrfSafeResolver` and `src/ssrf.rs`'s `Blocklist`.
