# network plugin roadmap

Goal: make `network` the one blessed way any plugin does outbound network
I/O — nobody else opens sockets, everybody routes through here so SSRF
policy, egress control, and observability live in one place.

## Done

- **Response body as bytes, not lossy UTF-8** — `body` is UTF-8 text as-is
  when valid, else base64 with `body_encoding: "base64"`.
- **Header/URL size caps** — `MAX_URL_LEN` (8 KiB), `MAX_HEADER_COUNT`
  (100), `MAX_HEADERS_TOTAL_BYTES` (32 KiB), all rejected outright.
- **IPv6 SSRF test coverage** — loopback/unique-local/link-local/multicast
  and a public-IP allow case, mirroring the v4 tests.
- **Allowlist mode** — `NETWORK_PLUGIN_ALLOWED_HOSTS`, default-deny except
  listed hosts/IPs; `NETWORK_PLUGIN_EXTRA_BLOCKED_HOSTS` still overrides on
  top.
- **Redirect-follow, opt-in** — `follow_redirects: true` follows up to
  `MAX_REDIRECTS` (10, fixed) hops; every hop still resolves through
  `SsrfSafeResolver` via a second pre-built client
  (`NetworkPlugin::redirect_client`), sharing the same TLS/proxy config as
  the default client.
- **TLS client cert (mTLS) + custom CA bundle** —
  `NETWORK_PLUGIN_CA_BUNDLE_PATH` / `NETWORK_PLUGIN_CLIENT_IDENTITY_PATH`.
- **Structured JSON logging** — one JSON line per attempt to stdout.
- **Retry with backoff** — `max_retries`/`retry_backoff_ms`, retries only
  on 429/5xx/transport errors.
- **Opt-in proxy** — `NETWORK_PLUGIN_PROXY_URL`; ambient `HTTP_PROXY` env
  is now ignored (was a silent SSRF bypass before this was closed).

## Near-term (buildable now, no kernel changes)

- **Per-caller concurrency cap** — track in-flight requests per calling
  plugin id and reject over some configurable limit, so one noisy plugin
  can't monopolize `network`'s outbound connections. Currently blocked:
  `ActionRequest` has no caller-id field to key on (see
  `KERNEL_PROTOCOL_TODO.md`) — would need that added first, or the kernel
  to pass caller identity some other way.
- **Configurable `max_redirects` per request** — today it's a fixed 10 via
  a single pre-built client; a genuinely per-request cap would need either
  a client built per request (loses connection pooling) or a custom
  `redirect::Policy` closure driven by request-scoped state.

## Requires kernel/protocol changes (see `KERNEL_PROTOCOL_TODO.md`, gitignored)

- **`network.request_completed` events** — publish status/host/latency/retry
  count to the event bus instead of (or in addition to) stdout, so other
  plugins/observability tooling can subscribe. Blocked on a plugin → event
  publish wire path that doesn't exist yet.
- **`http_request_stream` action** — avoid full-body buffering for large
  downloads/uploads. Blocked on a chunked-action wire primitive.
- **WebSocket support** — persistent bidirectional connections. Biggest
  lift; needs its own design pass in `veyron-core` before any code here.
- **Kernel-enforced per-caller rate limits** — belt-and-suspenders on top of
  the near-term self-tracked concurrency cap above.

## "Standard network path" checklist

For `network` to be the thing every plugin reaches for instead of rolling
its own HTTP client, it needs, roughly in priority order:

1. Binary-safe responses (near-term item above) — text-only responses are
   a hard blocker for e.g. a plugin fetching an image.
2. Documented, stable JSON schema for `http_request`/response, versioned
   so other plugins don't need to track this plugin's internals directly.
3. Redirect support — a lot of real-world APIs redirect; disabling it
   entirely pushes callers back toward writing their own client.
4. Observability (events or at least parseable logs) so a caller plugin's
   failures are debuggable without SSHing in to read `network`'s stdout.
5. WebSocket, last — most plugins doing simple API calls only need HTTP;
   don't let this block the above.
