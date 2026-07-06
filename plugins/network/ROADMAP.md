# network plugin roadmap

Goal: make `network` the one blessed way any plugin does outbound network
I/O — nobody else opens sockets, everybody routes through here so SSRF
policy, egress control, and observability live in one place.

## Near-term (buildable now, no kernel changes)

- **Response body as bytes, not lossy UTF-8** — `resp.body` is currently
  `String::from_utf8_lossy`, which corrupts binary responses (images,
  protobuf, gzip). Switch to base64-encoded bytes, or branch on
  `content-type` and only lossy-decode text.
- **Header/URL size caps** — no limit today on header count/size or URL
  length; a caller (or compromised plugin) can hand us gigantic headers.
  Cap similar to `MAX_BODY_BYTES`.
- **IPv6 SSRF test coverage** — `is_blocked_ip`'s V6 branch
  (`unique_local`/`unicast_link_local`/multicast) has no unit tests,
  unlike the V4 branch. Close the gap.
- **Allowlist mode** — operator-configurable switch from default-block-list
  to default-deny-except-allowlist, for locked-down deployments
  (`NETWORK_PLUGIN_ALLOWED_HOSTS`, mirroring the existing
  `NETWORK_PLUGIN_EXTRA_BLOCKED_HOSTS` shape).
- **Per-caller concurrency cap** — track in-flight requests per calling
  plugin id (if `ActionRequest` carries a caller id) and reject over some
  configurable limit, so one noisy plugin can't monopolize `network`'s
  outbound connections even without kernel-level quotas.
- **Redirect-follow, opt-in** — v1 disables redirects outright. Add an
  opt-in `follow_redirects: bool` + `max_redirects` param, re-checking the
  SSRF resolver on every hop (it already covers this — just re-enable
  `redirect::Policy` with a policy closure instead of `none()`).
- **TLS client cert (mTLS) + custom CA bundle** — operator-supplied
  `NETWORK_PLUGIN_CLIENT_CERT_PATH`/`NETWORK_PLUGIN_CLIENT_KEY_PATH` and
  `NETWORK_PLUGIN_CA_BUNDLE_PATH`, for talking to internal APIs that need
  mutual TLS or a private CA. Operator-only (env), same trust model as
  `NETWORK_PLUGIN_PROXY_URL`.
- **Structured JSON logging** — current stdout logging is
  `println!`-formatted text; switch to one-line JSON per attempt so
  operators can pipe it into normal log aggregation without a custom
  parser.

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
