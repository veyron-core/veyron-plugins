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

## Retries

`http_request` accepts optional `max_retries` (default `0`, capped at `5`)
and `retry_backoff_ms` (default `200`, capped at `5000`, doubling each
attempt). A response is retried only on `429` or `5xx`; any other status
(including other `4xx`) is returned on the first attempt. Transport-level
failures (connection refused, timeout) are always retried up to
`max_retries`. Retries are opt-in — callers get none unless they ask.

Retries aren't restricted to idempotent methods; a caller requesting
retries on e.g. `POST` is responsible for that being safe for its endpoint.

## Proxy (operator-only)

By default no HTTP proxy is used, and ambient `HTTP_PROXY`/`HTTPS_PROXY`
environment variables are ignored — `reqwest` honors them by default, which
would otherwise let a request bypass `SsrfSafeResolver` entirely (the
target host gets resolved by the proxy, not by this plugin).

An operator can opt in via `NETWORK_PLUGIN_PROXY_URL` in the plugin's `env:`
list, e.g. `NETWORK_PLUGIN_PROXY_URL=http://proxy.internal:8080`. This is
deliberately not a per-request param: once set, the SSRF blocklist no
longer covers hosts reached through that proxy — only enable it pointed at
a proxy you trust to do its own filtering.

## Logging

Every attempt (including retries) logs one JSON line to stdout: `plugin`,
`method`, `host`, `attempt`, `status`, `error`, `duration_ms`. There's no
kernel-event-bus metrics path yet — the wire protocol has no plugin → event
publish path (only kernel-internal code calls `EventBus::publish`), so that
would need changes in `veyron-core` itself, not this plugin.

## Response body encoding

`body` is the response text as-is when it's valid UTF-8. When it isn't
(binary responses — images, protobuf, etc.), `body` is base64 and
`body_encoding` is `"base64"` instead of `"utf8"`. Always check
`body_encoding` before treating `body` as text.

## Redirects

Disabled by default (`ACTION_OK` with the redirect's own 3xx status
returned as-is). Set `follow_redirects: true` to follow up to
`MAX_REDIRECTS` (10, fixed — not caller-configurable) hops; every hop still
resolves through `SsrfSafeResolver`, so a redirect to a blocked host still
fails the whole request.

## Allowlist mode (operator-configurable)

`NETWORK_PLUGIN_ALLOWED_HOSTS` (same comma-separated IP/hostname shape as
`NETWORK_PLUGIN_EXTRA_BLOCKED_HOSTS`) switches from default-block to
default-deny: when set, only listed hosts/IPs are reachable at all, and the
built-in RFC1918/loopback/etc. ranges stop being consulted for them (an
allowlist is an explicit statement that reaching that address, even a
private one, is intended). `NETWORK_PLUGIN_EXTRA_BLOCKED_HOSTS` still
applies on top as an override.

## TLS: custom CA bundle and client identity (mTLS)

- `NETWORK_PLUGIN_CA_BUNDLE_PATH` — path to one or more PEM-encoded CA
  certificates (concatenated), trusted in addition to the built-in root
  store. For internal APIs signed by a private CA.
- `NETWORK_PLUGIN_CLIENT_IDENTITY_PATH` — path to a single PEM file
  containing both the client certificate and its private key
  (concatenated), presented for mutual TLS.

Both are read once at startup; an invalid path or malformed PEM aborts
plugin startup rather than silently skipping TLS config.
