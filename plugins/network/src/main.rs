//! `network` plugin — outbound HTTP for other plugins/kernel, gated by
//! `PERMISSION_NETWORK`. See
//! docs/superpowers/specs/2026-07-05-network-plugin-design.md for the design.
//!
//! v1 is HTTP only. Needs real network egress: run with `sandbox: false`
//! in the kernel's `config.yaml` (see README.md).
//!
//! Concurrency: like `database`, this hand-rolls a concurrent loop instead
//! of the SDK's sequential `serve()` (root ROADMAP.md, "hot-path plugins").
//! One task owns the `VeyronClient` exclusively and `tokio::select!`s
//! between `client.recv()` (inbound frames) and an `mpsc::Receiver` that
//! spawned handler tasks push completed responses into — see `run_loop` for
//! why this can't deadlock. Each `http_request` runs in its own task, so
//! multiple requests from one caller (and many callers) make progress in
//! parallel, and a per-caller in-flight cap
//! (`NETWORK_PLUGIN_MAX_INFLIGHT_PER_CALLER`) keeps one noisy caller from
//! monopolizing `network`'s outbound connections. Out-of-order replies are
//! fine — the kernel matches on `action_id`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use network_plugin::{handler, request};
use tokio::sync::mpsc;
use veyron_sdk::proto::{
    envelope, ActionResponse, ActionStatus, Envelope, EventPublish, EventPublishStatus,
    PluginManifest, Pong,
};
use veyron_sdk::{VeyronClient, VeyronError};

/// Operator-only opt-in proxy for all outbound requests. Deliberately not a
/// per-request param: a caller-controlled proxy would let any action bypass
/// `SsrfSafeResolver` entirely (the target host is resolved by the proxy,
/// not by us), so only an operator setting the plugin's own environment can
/// enable it.
const PROXY_URL_ENV: &str = "NETWORK_PLUGIN_PROXY_URL";

/// Operator-supplied extra CA cert(s) (PEM, one or more concatenated) to
/// trust in addition to the built-in root store — for internal APIs signed
/// by a private CA.
const CA_BUNDLE_PATH_ENV: &str = "NETWORK_PLUGIN_CA_BUNDLE_PATH";

/// Operator-supplied client identity (a single PEM file containing both the
/// client certificate and its private key, concatenated) for mTLS.
const CLIENT_IDENTITY_PATH_ENV: &str = "NETWORK_PLUGIN_CLIENT_IDENTITY_PATH";

/// Per-caller ceiling on how many `http_request`s may be in flight at once.
/// One noisy plugin can't monopolize `network`'s outbound connections while
/// others starve. `0` disables the cap (unlimited).
const MAX_INFLIGHT_PER_CALLER_ENV: &str = "NETWORK_PLUGIN_MAX_INFLIGHT_PER_CALLER";
const DEFAULT_MAX_INFLIGHT_PER_CALLER: usize = 8;

/// Everything operator-configurable that shapes a `reqwest::Client`, read
/// once at startup so per-cap redirect clients don't re-read env/files.
struct ClientConfig {
    proxy: Option<reqwest::Proxy>,
    ca_certs: Vec<reqwest::Certificate>,
    identity: Option<reqwest::Identity>,
}

impl ClientConfig {
    fn from_env() -> Self {
        let proxy = match std::env::var(PROXY_URL_ENV) {
            Ok(proxy_url) => Some(
                reqwest::Proxy::all(&proxy_url)
                    .unwrap_or_else(|e| panic!("invalid {PROXY_URL_ENV}: {e}")),
            ),
            Err(_) => None,
        };
        let mut ca_certs = Vec::new();
        if let Ok(ca_path) = std::env::var(CA_BUNDLE_PATH_ENV) {
            let pem = std::fs::read(&ca_path)
                .unwrap_or_else(|e| panic!("failed to read {CA_BUNDLE_PATH_ENV} ({ca_path}): {e}"));
            ca_certs = reqwest::Certificate::from_pem_bundle(&pem)
                .unwrap_or_else(|e| panic!("invalid CA bundle at {ca_path}: {e}"));
        }
        let identity = match std::env::var(CLIENT_IDENTITY_PATH_ENV) {
            Ok(identity_path) => {
                let pem = std::fs::read(&identity_path).unwrap_or_else(|e| {
                    panic!("failed to read {CLIENT_IDENTITY_PATH_ENV} ({identity_path}): {e}")
                });
                Some(
                    reqwest::Identity::from_pem(&pem).unwrap_or_else(|e| {
                        panic!("invalid client identity at {identity_path}: {e}")
                    }),
                )
            }
            Err(_) => None,
        };
        Self {
            proxy,
            ca_certs,
            identity,
        }
    }
}

struct NetworkPlugin {
    /// Shared no-redirect client — the default path, so connection pooling
    /// is preserved for callers that don't follow redirects.
    client: reqwest::Client,
    /// One redirect-enabled client per distinct `max_redirects` value
    /// (`0..=request::MAX_REDIRECTS`, so `handle_http_request` can index it
    /// directly). Building per cap instead of per request keeps connection
    /// pooling for the common values while still honoring a caller's
    /// request-scoped limit.
    redirect_clients: Vec<reqwest::Client>,
    /// Same instances handed to `SsrfSafeResolver` for every client — kept
    /// here too so `handle_http_request` can run the literal-IP gate
    /// (`ssrf::check_literal_ip_host`) that the resolver can't cover, since
    /// it's only ever invoked for hostnames needing DNS resolution.
    extra_blocklist: network_plugin::ssrf::Blocklist,
    allowlist: network_plugin::ssrf::Allowlist,
}

impl NetworkPlugin {
    fn new() -> Self {
        let extra_blocklist = network_plugin::ssrf::Blocklist::from_env();
        let allowlist = network_plugin::ssrf::Allowlist::from_env();
        let config = ClientConfig::from_env();
        Self {
            client: Self::build_client(
                reqwest::redirect::Policy::none(),
                extra_blocklist.clone(),
                allowlist.clone(),
                &config,
            ),
            redirect_clients: (0..=request::MAX_REDIRECTS)
                .map(|cap| {
                    Self::build_client(
                        Self::redirect_policy(cap, extra_blocklist.clone(), allowlist.clone()),
                        extra_blocklist.clone(),
                        allowlist.clone(),
                        &config,
                    )
                })
                .collect(),
            extra_blocklist,
            allowlist,
        }
    }

    /// Redirect policy for `follow_redirects: true`, capped at
    /// `max_redirects` hops. Can't rely on `SsrfSafeResolver` alone here:
    /// it's a DNS resolver, and `reqwest` skips DNS resolution (and so the
    /// resolver) whenever a hop's target host is already a literal IP
    /// (`hyper-util`'s `HttpConnector::call_async`) — a redirect to
    /// `http://169.254.169.254/...` would otherwise sail through unguarded.
    /// Every hop's URL is checked here in addition to the resolver, which
    /// still runs for hostname hops.
    fn redirect_policy(
        max_redirects: usize,
        extra_blocklist: network_plugin::ssrf::Blocklist,
        allowlist: network_plugin::ssrf::Allowlist,
    ) -> reqwest::redirect::Policy {
        reqwest::redirect::Policy::custom(move |attempt| {
            // `previous()` includes the original URL, so `> max_redirects`
            // is what allows exactly `max_redirects` hops (reqwest's own
            // docs use `previous().len() > n` for "allow n redirects").
            if attempt.previous().len() > max_redirects {
                return attempt.stop();
            }
            let host = attempt.url().host_str().unwrap_or_default();
            match network_plugin::ssrf::check_literal_ip_host(host, &extra_blocklist, &allowlist)
            {
                Ok(()) => attempt.follow(),
                Err(e) => attempt.error(e),
            }
        })
    }

    /// Builds one `reqwest::Client` with every operator-configured option
    /// (SSRF resolver, proxy, CA bundle, client identity) applied — only
    /// `redirect` differs between the no-redirect client and the
    /// per-cap redirect clients, so all of them share the same
    /// TLS/proxy/SSRF posture instead of drifting apart.
    fn build_client(
        redirect_policy: reqwest::redirect::Policy,
        extra_blocklist: network_plugin::ssrf::Blocklist,
        allowlist: network_plugin::ssrf::Allowlist,
        config: &ClientConfig,
    ) -> reqwest::Client {
        // SSRF gating lives in `SsrfSafeResolver` (used for every connect,
        // including redirects) rather than a one-time pre-flight check —
        // see module docs on `SsrfSafeResolver`. That covers hostnames;
        // literal-IP hosts bypass it entirely (see `redirect_policy` and
        // `handle_http_request`), so this is deliberately not the only gate.
        let resolver = handler::SsrfSafeResolver {
            extra_blocklist,
            allowlist,
        };
        let mut builder = reqwest::Client::builder()
            .redirect(redirect_policy)
            .dns_resolver(Arc::new(resolver))
            // reqwest honors HTTP_PROXY/HTTPS_PROXY from the environment by
            // default; that would silently route requests around
            // SsrfSafeResolver. Turn it off — proxying is opt-in only via
            // `NETWORK_PLUGIN_PROXY_URL` below.
            .no_proxy();
        if let Some(proxy) = &config.proxy {
            builder = builder.proxy(proxy.clone());
        }
        for cert in &config.ca_certs {
            builder = builder.add_root_certificate(cert.clone());
        }
        if let Some(identity) = &config.identity {
            builder = builder.identity(identity.clone());
        }
        builder.build().expect("failed to build reqwest client")
    }

    fn client_for(&self, params: &request::HttpRequestParams) -> &reqwest::Client {
        if params.follow_redirects {
            &self.redirect_clients[params.max_redirects]
        } else {
            &self.client
        }
    }

    async fn handle_http_request(&self, params_json: &[u8]) -> HttpOutcome {
        let started = std::time::Instant::now();
        let params = match request::parse_request(params_json) {
            Ok(params) => params,
            Err(e) => {
                return HttpOutcome {
                    response: Err(e),
                    status: 0,
                    host: String::new(),
                    latency_ms: started.elapsed().as_millis() as u64,
                    retry_count: 0,
                }
            }
        };
        let host = url::Url::parse(&params.url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_default();

        // `SsrfSafeResolver` never runs for a literal-IP host (see its gate
        // in `redirect_policy`'s doc comment) — this is the only check for
        // the initial URL in that case. Rejecting here also avoids wasting
        // `network`'s retry/backoff budget on a request that was never
        // going anywhere.
        if let Ok(url) = url::Url::parse(&params.url) {
            let host_str = url.host_str().unwrap_or_default();
            if let Err(e) = network_plugin::ssrf::check_literal_ip_host(
                host_str,
                &self.extra_blocklist,
                &self.allowlist,
            ) {
                return HttpOutcome {
                    response: Err(e),
                    status: 0,
                    host,
                    latency_ms: started.elapsed().as_millis() as u64,
                    retry_count: 0,
                };
            }
            // Hostname hosts: same fail-fast intent as the literal-IP gate.
            // The resolver is still authoritative at connect time (a name
            // can re-resolve between here and there — rebinding TOCTOU),
            // but a host that's blocked today fails here deterministically,
            // before the retry loop can burn attempts on it.
            if host_str.parse::<std::net::IpAddr>().is_err() && !host_str.is_empty() {
                if let Err(e) = handler::check_host_reachable(
                    host_str,
                    &self.extra_blocklist,
                    &self.allowlist,
                )
                .await
                {
                    return HttpOutcome {
                        response: Err(e),
                        status: 0,
                        host,
                        latency_ms: started.elapsed().as_millis() as u64,
                        retry_count: 0,
                    };
                }
            }
        }

        let outcome = handler::fetch_with_stats(self.client_for(&params), &params).await;
        let (response, status) = match outcome.response {
            Ok(resp) => (
                serde_json::to_vec(&serde_json::json!({
                    "status": resp.status,
                    "headers": resp.headers,
                    "body": resp.body,
                    "body_encoding": resp.body_encoding,
                }))
                .map_err(|e| format!("failed to encode response: {e}")),
                resp.status,
            ),
            Err(e) => (Err(e), 0),
        };
        HttpOutcome {
            response,
            status,
            host,
            latency_ms: started.elapsed().as_millis() as u64,
            retry_count: outcome.stats.attempts.saturating_sub(1),
        }
    }
}

/// Terminal outcome of one `http_request`: the response bytes (or error) the
/// caller gets in its `ActionResponse`, plus the data the best-effort
/// `network.request_completed` event carries. `spawn_handler` builds both
/// envelopes from this single value.
struct HttpOutcome {
    response: Result<Vec<u8>, String>,
    /// HTTP status of the final attempt; `0` when no HTTP response was
    /// obtained (SSRF-policy rejection, transport failure).
    status: u16,
    host: String,
    latency_ms: u64,
    /// `attempts - 1` — how many times the fetch was retried.
    retry_count: u32,
}

/// Best-effort `network.request_completed` event (kernel-namespaced as
/// `plugin.network.request_completed`). Fire-and-forget: it must never block
/// or alter the caller's response, so `spawn_handler` sends the
/// `ActionResponse` envelope first.
fn event_envelope(outcome: &HttpOutcome) -> Envelope {
    let payload_json = serde_json::to_vec(&serde_json::json!({
        "status": outcome.status,
        "host": outcome.host,
        "latency_ms": outcome.latency_ms,
        "retry_count": outcome.retry_count,
        "error": outcome.response.as_ref().err().cloned().unwrap_or_default(),
    }))
    .unwrap_or_else(|e| format!("{{\"error\":\"failed to encode event: {e}\"}}").into_bytes());
    Envelope {
        payload: Some(envelope::Payload::EventPublish(EventPublish {
            event_type: "request_completed".into(),
            payload_json,
        })),
        ..Default::default()
    }
}

fn manifest() -> PluginManifest {
    PluginManifest {
        permissions: vec![
            "PERMISSION_NETWORK".into(),
            "PERMISSION_EVENT_PUBLISH".into(),
        ],
        actions: vec!["http_request".into()],
        ..Default::default()
    }
}

/// Per-caller in-flight request tracker for the concurrency cap. The lock
/// is `std::sync::Mutex` on purpose: it is only ever held for the brief
/// increment/decrement, never across an `.await`, so a parking-lot/tokio
/// mutex's async semantics buy nothing here.
struct Inflight {
    limit: usize,
    active: Mutex<HashMap<String, usize>>,
}

impl Inflight {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            active: Mutex::new(HashMap::new()),
        }
    }

    /// Reserve a slot for `caller`, rejecting the request when it already
    /// has `limit` requests in flight.
    fn try_acquire(&self, caller: &str) -> Result<(), String> {
        if self.limit == 0 {
            return Ok(());
        }
        let mut active = self.active.lock().unwrap();
        let in_flight = active.entry(caller.to_string()).or_insert(0);
        if *in_flight >= self.limit {
            return Err(format!(
                "caller {caller} already has {in_flight} requests in flight (limit {})",
                self.limit
            ));
        }
        *in_flight += 1;
        Ok(())
    }

    /// Free the slot a completed (or failed) request held for `caller`.
    fn release(&self, caller: &str) {
        if self.limit == 0 {
            return;
        }
        let mut active = self.active.lock().unwrap();
        if let Some(in_flight) = active.get_mut(caller) {
            *in_flight -= 1;
            if *in_flight == 0 {
                active.remove(caller);
            }
        }
    }
}

/// Build the response envelope for a completed (or failed) action.
fn response_envelope(action_id: String, result: Result<Vec<u8>, String>) -> Envelope {
    let response = match result {
        Ok(data_json) => ActionResponse {
            action_id,
            status: ActionStatus::ActionOk as i32,
            data_json,
            error: String::new(),
        },
        Err(error) => ActionResponse {
            action_id,
            status: ActionStatus::ActionError as i32,
            data_json: Vec::new(),
            error,
        },
    };
    Envelope {
        payload: Some(envelope::Payload::ActionResponse(response)),
        ..Default::default()
    }
}

/// Spawn a handler task for `req` that always produces exactly one response
/// envelope on `tx`, even if `handle_http_request` panics.
///
/// This double-spawns: the inner `tokio::spawn` runs the actual handler and
/// its `JoinHandle` is awaited by the outer task. A panic inside the inner
/// task is caught by Tokio and surfaced as `Err(JoinError)` to the outer
/// task rather than unwinding it, so the outer task can always reach the
/// `inflight.release(...)` and `tx.send(...)` at the end — a panicking
/// handler becomes an `ACTION_ERROR` response (and a freed cap slot)
/// instead of a silently dropped reply.
fn spawn_handler(
    plugin: Arc<NetworkPlugin>,
    inflight: Arc<Inflight>,
    tx: mpsc::Sender<Envelope>,
    action_id: String,
    caller_plugin_id: String,
    params_json: Vec<u8>,
) {
    tokio::spawn(async move {
        let inner_plugin = plugin.clone();
        let join = tokio::spawn(async move { inner_plugin.handle_http_request(&params_json).await });
        let outcome = match join.await {
            Ok(outcome) => outcome,
            Err(join_err) => HttpOutcome {
                response: Err(format!("handler panicked: {join_err}")),
                status: 0,
                host: String::new(),
                latency_ms: 0,
                retry_count: 0,
            },
        };
        inflight.release(&caller_plugin_id);
        let event = event_envelope(&outcome);
        let envelope = response_envelope(action_id, outcome.response);
        // Receiver side only goes away when the main loop exits, at which
        // point dropping the reply is the correct behavior anyway.
        let _ = tx.send(envelope).await;
        // Best-effort, sent only after the response so the reply never waits on observability.
        let _ = tx.send(event).await;
    });
}

fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Drive the plugin's message loop to completion (disconnect, EOF, or an
/// explicit `PluginShutdown`).
///
/// `client` is owned exclusively by this function — never shared behind a
/// lock. Each loop iteration is a single `tokio::select!` between two
/// futures: `client.recv()` (the next inbound frame) and `rx.recv()` (the
/// next completed response envelope from a spawned handler). Because
/// `client` is never wrapped in a `Mutex`, a handler that finishes while
/// this function is parked inside `client.recv().await` does not need to
/// acquire anything the loop task holds: it just calls `tx.send(...)`,
/// which wakes the `select!` to pick the envelope up and `client.send(...)`
/// it on the next iteration — no task ever waits on a resource held by a
/// task waiting on it. See the module docs in `database`'s main.rs for the
/// full deadlock analysis this design replaced.
async fn run_loop(
    mut client: VeyronClient,
    plugin: Arc<NetworkPlugin>,
    inflight: Arc<Inflight>,
) -> Result<(), VeyronError> {
    let (tx, mut rx) = mpsc::channel::<Envelope>(256);

    loop {
        tokio::select! {
            envelope = client.recv() => {
                let envelope = match envelope {
                    Ok(env) => env,
                    Err(_) => break, // disconnect / EOF
                };

                match envelope.payload {
                    Some(envelope::Payload::Ping(ping)) => {
                        let pong = Envelope {
                            payload: Some(envelope::Payload::Pong(Pong {
                                original_timestamp: ping.timestamp,
                                server_timestamp: unix_millis(),
                            })),
                            ..Default::default()
                        };
                        let _ = client.send("kernel", pong).await;
                    }
                    Some(envelope::Payload::PluginShutdown(_)) => break,
                    Some(envelope::Payload::EventPublishAck(ack)) => {
                        if ack.status != EventPublishStatus::EventPublishOk as i32 {
                            println!("[network] event publish failed: {:?}", ack.status);
                        }
                    }
                    Some(envelope::Payload::ActionRequest(req)) if req.action == "http_request" => {
                        match inflight.try_acquire(&req.caller_plugin_id) {
                            Ok(()) => {
                                spawn_handler(
                                    plugin.clone(),
                                    inflight.clone(),
                                    tx.clone(),
                                    req.action_id,
                                    req.caller_plugin_id,
                                    req.params_json,
                                );
                            }
                            Err(error) => {
                                println!(
                                    "[network] rejecting http_request from {}: {error}",
                                    req.caller_plugin_id
                                );
                                let envelope = response_envelope(req.action_id, Err(error));
                                let _ = client.send("kernel", envelope).await;
                            }
                        }
                    }
                    Some(envelope::Payload::ActionRequest(req)) => {
                        let envelope = response_envelope(
                            req.action_id,
                            Err(format!("unknown action: {}", req.action)),
                        );
                        let _ = client.send("kernel", envelope).await;
                    }
                    other => {
                        println!("[network] unhandled message: {other:?}");
                    }
                }
            }
            Some(response_envelope) = rx.recv() => {
                let _ = client.send("kernel", response_envelope).await;
            }
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), VeyronError> {
    let plugin = Arc::new(NetworkPlugin::new());
    let max_inflight_per_caller = std::env::var(MAX_INFLIGHT_PER_CALLER_ENV)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_MAX_INFLIGHT_PER_CALLER);
    let inflight = Arc::new(Inflight::new(max_inflight_per_caller));

    let mut client = VeyronClient::connect_from_env().await?;
    let token = std::env::var("VEYRON_JWT_TOKEN").unwrap_or_default();
    let ack = client.register_full("network", "0.3.0", manifest(), &token).await?;
    if !ack.accepted {
        return Err(VeyronError::PermissionDenied(format!(
            "registration rejected: {}",
            ack.reject_reason
        )));
    }
    println!("[network] registered with kernel");

    run_loop(client, plugin, inflight).await?;

    println!("[network] shutting down");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::net::UnixStream;
    use veyron_sdk::proto::{ActionRequest, PluginShutdown};
    use veyron_sdk::VeyronClient;

    /// Plugin whose SSRF policy permits loopback, so tests can hit local
    /// mock servers (the real `NetworkPlugin::new()` reads operator env and
    /// would block 127.0.0.1).
    fn test_plugin() -> Arc<NetworkPlugin> {
        let extra_blocklist = network_plugin::ssrf::Blocklist::default();
        let allowlist = network_plugin::ssrf::Allowlist::parse("127.0.0.1");
        let config = ClientConfig {
            proxy: None,
            ca_certs: Vec::new(),
            identity: None,
        };
        let client = NetworkPlugin::build_client(
            reqwest::redirect::Policy::none(),
            extra_blocklist.clone(),
            allowlist.clone(),
            &config,
        );
        let redirect_clients = (0..=request::MAX_REDIRECTS)
            .map(|cap| {
                NetworkPlugin::build_client(
                    NetworkPlugin::redirect_policy(cap, extra_blocklist.clone(), allowlist.clone()),
                    extra_blocklist.clone(),
                    allowlist.clone(),
                    &config,
                )
            })
            .collect();
        Arc::new(NetworkPlugin {
            client,
            redirect_clients,
            extra_blocklist,
            allowlist,
        })
    }

    async fn mock_server_responding(response: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = match listener.accept().await {
                    Ok(conn) => conn,
                    Err(_) => break,
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buf).await;
                    let _ =
                        tokio::io::AsyncWriteExt::write_all(&mut socket, response.as_bytes()).await;
                });
            }
        });
        format!("http://{addr}/")
    }

    fn http_request(
        action_id: &str,
        caller: &str,
        params_json: Vec<u8>,
    ) -> Envelope {
        Envelope {
            payload: Some(envelope::Payload::ActionRequest(ActionRequest {
                action_id: action_id.to_string(),
                action: "http_request".into(),
                params_json,
                timeout_ms: 0,
                streaming: false,
                caller_plugin_id: caller.to_string(),
            })),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn inflight_cap_tracks_and_releases_per_caller() {
        let inflight = Inflight::new(2);
        assert!(inflight.try_acquire("a").is_ok());
        assert!(inflight.try_acquire("a").is_ok());
        let err = inflight.try_acquire("a").unwrap_err();
        assert!(err.contains("in flight"), "error was: {err}");
        // Different caller is unaffected — the cap is per caller, not global.
        assert!(inflight.try_acquire("b").is_ok());

        inflight.release("a");
        assert!(inflight.try_acquire("a").is_ok());
        assert!(inflight.try_acquire("a").is_err());
        // Releasing below zero / unknown callers is a no-op, not a panic.
        inflight.release("a");
        inflight.release("a");
        inflight.release("never_acquired");
    }

    /// Regression test for the deadlock shape that motivated the concurrent
    /// loop (same pattern as `database`'s): fires a batch of
    /// `http_request`s back-to-back over a real `VeyronClient`
    /// (`UnixStream::pair` + `VeyronClient::from_stream` is the SDK's own
    /// test pattern) and then does *not* send anything until every response
    /// has been read back. The `tokio::time::timeout` wrapper turns "would
    /// have hung forever" into a clean failure.
    #[tokio::test]
    async fn concurrent_requests_get_responses_without_deadlocking() {
        let plugin = test_plugin();
        let inflight = Arc::new(Inflight::new(0)); // unlimited — nothing to reject
        let url = mock_server_responding("HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\nhello").await;

        let (plugin_side, kernel_side) = UnixStream::pair().unwrap();
        let client = VeyronClient::from_stream(plugin_side, None);
        let mut kernel = VeyronClient::from_stream(kernel_side, None);

        let loop_task = tokio::spawn(run_loop(client, plugin, inflight));

        const N: usize = 20;
        for i in 0..N {
            let params = serde_json::json!({"method": "GET", "url": url}).to_string();
            kernel
                .send(
                    "network",
                    http_request(&format!("action-{i}"), "caller_x", params.into_bytes()),
                )
                .await
                .unwrap();
        }

        let mut seen = std::collections::HashSet::new();
        let mut events = 0;
        while seen.len() < N || events < N {
            let env = tokio::time::timeout(Duration::from_secs(5), kernel.recv())
                .await
                .expect("timed out waiting for response — loop likely deadlocked")
                .unwrap();
            match env.payload {
                Some(envelope::Payload::ActionResponse(resp)) => {
                    assert_eq!(resp.status, ActionStatus::ActionOk as i32);
                    let v: serde_json::Value = serde_json::from_slice(&resp.data_json).unwrap();
                    assert_eq!(v["body"], "hello");
                    assert!(seen.insert(resp.action_id), "duplicate response");
                }
                Some(envelope::Payload::EventPublish(ev)) => {
                    assert_eq!(ev.event_type, "request_completed");
                    let v: serde_json::Value = serde_json::from_slice(&ev.payload_json).unwrap();
                    assert_eq!(v["status"], 200);
                    assert_eq!(v["retry_count"], 0, "no retries requested");
                    events += 1;
                }
                other => panic!("unexpected payload: {other:?}"),
            }
        }

        let shutdown = Envelope {
            payload: Some(envelope::Payload::PluginShutdown(PluginShutdown {
                reason: "test done".into(),
                grace_seconds: 0,
            })),
            ..Default::default()
        };
        kernel.send("network", shutdown).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), loop_task)
            .await
            .expect("run_loop did not exit after PluginShutdown")
            .unwrap()
            .unwrap();
    }

    /// The cap is enforced per caller and only while requests are actually
    /// in flight. A slow server holds accepted requests open long enough
    /// for the rejects to be observable; once the accepted ones finish,
    /// the slot frees up again.
    #[tokio::test]
    async fn per_caller_cap_rejects_over_limit_requests() {
        let plugin = test_plugin();
        let inflight = Arc::new(Inflight::new(2));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buf).await;
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    let _ = tokio::io::AsyncWriteExt::write_all(
                        &mut socket,
                        b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok",
                    )
                    .await;
                });
            }
        });
        let url = format!("http://{addr}/");

        let (plugin_side, kernel_side) = UnixStream::pair().unwrap();
        let client = VeyronClient::from_stream(plugin_side, None);
        let mut kernel = VeyronClient::from_stream(kernel_side, None);
        let loop_task = tokio::spawn(run_loop(client, plugin, inflight));

        for i in 0..5 {
            let params = serde_json::json!({"method": "GET", "url": url}).to_string();
            kernel
                .send(
                    "network",
                    http_request(&format!("x-{i}"), "caller_x", params.into_bytes()),
                )
                .await
                .unwrap();
        }
        // A different caller is never affected by caller_x's cap.
        let params = serde_json::json!({"method": "GET", "url": url}).to_string();
        kernel
            .send(
                "network",
                http_request("y-1", "caller_y", params.into_bytes()),
            )
            .await
            .unwrap();

        let mut ok = 0;
        let mut rejected = 0;
        let mut events = 0;
        while ok + rejected < 6 || events < 3 {
            let env = tokio::time::timeout(Duration::from_secs(5), kernel.recv())
                .await
                .expect("timed out waiting for response")
                .unwrap();
            match env.payload {
                Some(envelope::Payload::ActionResponse(resp)) => {
                    if resp.status == ActionStatus::ActionOk as i32 {
                        ok += 1;
                    } else {
                        assert!(
                            resp.error.contains("in flight"),
                            "unexpected rejection: {}",
                            resp.error
                        );
                        rejected += 1;
                    }
                }
                Some(envelope::Payload::EventPublish(ev)) => {
                    assert_eq!(ev.event_type, "request_completed");
                    events += 1;
                }                other => panic!("unexpected payload: {other:?}"),
            }
        }
        assert_eq!(ok, 3, "caller_x's 2 + caller_y's 1 should all succeed");
        assert_eq!(rejected, 3, "caller_x's remaining 3 should be rejected");
        assert_eq!(events, 3, "one event per accepted request, none for over-cap rejections");

        let shutdown = Envelope {
            payload: Some(envelope::Payload::PluginShutdown(PluginShutdown {
                reason: "test done".into(),
                grace_seconds: 0,
            })),
            ..Default::default()
        };
        kernel.send("network", shutdown).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), loop_task)
            .await
            .expect("run_loop did not exit after PluginShutdown")
            .unwrap()
            .unwrap();
    }

    /// `max_redirects` is honored per request: a chain of two redirects
    /// (A → B → C) followed with `max_redirects: 1` stops at B's 3xx;
    /// `max_redirects: 2` follows through to C.
    #[tokio::test]
    async fn redirect_max_caps_hops() {
        let plugin = test_plugin();
        let final_url = mock_server_responding("HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok").await;
        let hop_b_url = {
            let location = format!("location: {final_url}\r\n");
            let response = format!("HTTP/1.1 302 Found\r\n{location}content-length: 0\r\n\r\n");
            mock_server_responding(Box::leak(response.into_boxed_str())).await
        };
        let hop_a_url = {
            let location = format!("location: {hop_b_url}\r\n");
            let response = format!("HTTP/1.1 302 Found\r\n{location}content-length: 0\r\n\r\n");
            mock_server_responding(Box::leak(response.into_boxed_str())).await
        };

        let capped = serde_json::json!({
            "method": "GET",
            "url": hop_a_url,
            "follow_redirects": true,
            "max_redirects": 1,
        })
        .to_string();
        let out = plugin.handle_http_request(capped.as_bytes()).await.response.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["status"], 302, "one hop allowed: stops at B's 3xx");

        let full = serde_json::json!({
            "method": "GET",
            "url": hop_a_url,
            "follow_redirects": true,
            "max_redirects": 2,
        })
        .to_string();
        let out = plugin.handle_http_request(full.as_bytes()).await.response.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["status"], 200, "two hops allowed: reaches C");
        assert_eq!(v["body"], "ok");
    }

    /// A successful request emits exactly one `request_completed` event with
    /// `retry_count = attempts - 1` (0 here: one attempt, no retries), the
    /// HTTP status, the target host, and a latency measurement.
    #[tokio::test]
    async fn request_completed_event_reports_retry_count_on_success() {
        let plugin = test_plugin();
        let inflight = Arc::new(Inflight::new(0));
        let url = mock_server_responding("HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\nhello").await;

        let (plugin_side, kernel_side) = UnixStream::pair().unwrap();
        let client = VeyronClient::from_stream(plugin_side, None);
        let mut kernel = VeyronClient::from_stream(kernel_side, None);
        let loop_task = tokio::spawn(run_loop(client, plugin, inflight));

        let params = serde_json::json!({"method": "GET", "url": url}).to_string();
        kernel
            .send(
                "network",
                http_request("evt-ok", "caller_x", params.into_bytes()),
            )
            .await
            .unwrap();

        let mut response_seen = false;
        let mut event_seen = false;
        for _ in 0..2 {
            let env = tokio::time::timeout(Duration::from_secs(5), kernel.recv())
                .await
                .expect("timed out waiting for response")
                .unwrap();
            match env.payload {
                Some(envelope::Payload::ActionResponse(resp)) => {
                    assert_eq!(resp.status, ActionStatus::ActionOk as i32);
                    response_seen = true;
                }
                Some(envelope::Payload::EventPublish(ev)) => {
                    assert_eq!(ev.event_type, "request_completed");
                    let v: serde_json::Value = serde_json::from_slice(&ev.payload_json).unwrap();
                    assert_eq!(v["status"], 200);
                    assert_eq!(v["host"], "127.0.0.1");
                    assert_eq!(v["retry_count"], 0, "one attempt, no retries");
                    assert_eq!(v["error"], "");
                    assert!(v["latency_ms"].as_u64().is_some());
                    event_seen = true;
                }
                other => panic!("unexpected payload: {other:?}"),
            }
        }
        assert!(response_seen && event_seen, "expected a response and an event");

        let shutdown = Envelope {
            payload: Some(envelope::Payload::PluginShutdown(PluginShutdown {
                reason: "test done".into(),
                grace_seconds: 0,
            })),
            ..Default::default()
        };
        kernel.send("network", shutdown).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), loop_task)
            .await
            .expect("run_loop did not exit after PluginShutdown")
            .unwrap()
            .unwrap();
    }

    /// A request that ultimately fails still emits the event, with
    /// `retry_count = attempts - 1`: a server that drops the connection is a
    /// transport error (always retried), so `max_retries: 2` runs 3 attempts
    /// and the event reports `retry_count: 2`, `status: 0`, and an error
    /// string — the same `status`/`error` a subscriber needs to distinguish
    /// "never got an HTTP response" from a status-code failure.
    #[tokio::test]
    async fn request_completed_event_reports_retry_count_on_failure() {
        let plugin = test_plugin();
        let inflight = Arc::new(Inflight::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = match listener.accept().await {
                    Ok(conn) => conn,
                    Err(_) => break,
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buf).await;
                    drop(socket);
                });
            }
        });
        let url = format!("http://{addr}/");

        let (plugin_side, kernel_side) = UnixStream::pair().unwrap();
        let client = VeyronClient::from_stream(plugin_side, None);
        let mut kernel = VeyronClient::from_stream(kernel_side, None);
        let loop_task = tokio::spawn(run_loop(client, plugin, inflight));

        let params = serde_json::json!({
            "method": "GET",
            "url": url,
            "max_retries": 2,
            "retry_backoff_ms": 1,
        })
        .to_string();
        kernel
            .send(
                "network",
                http_request("evt-fail", "caller_x", params.into_bytes()),
            )
            .await
            .unwrap();

        let mut response_seen = false;
        let mut event_seen = false;
        for _ in 0..2 {
            let env = tokio::time::timeout(Duration::from_secs(5), kernel.recv())
                .await
                .expect("timed out waiting for response")
                .unwrap();
            match env.payload {
                Some(envelope::Payload::ActionResponse(resp)) => {
                    assert_eq!(resp.status, ActionStatus::ActionError as i32);
                    response_seen = true;
                }
                Some(envelope::Payload::EventPublish(ev)) => {
                    assert_eq!(ev.event_type, "request_completed");
                    let v: serde_json::Value = serde_json::from_slice(&ev.payload_json).unwrap();
                    assert_eq!(v["status"], 0, "no HTTP response was ever received");
                    assert_eq!(v["host"], "127.0.0.1");
                    assert_eq!(v["retry_count"], 2, "max_retries 2: 3 attempts, 2 retries");
                    assert!(!v["error"].as_str().unwrap().is_empty());
                    event_seen = true;
                }
                other => panic!("unexpected payload: {other:?}"),
            }
        }
        assert!(response_seen && event_seen, "expected a response and an event");

        let shutdown = Envelope {
            payload: Some(envelope::Payload::PluginShutdown(PluginShutdown {
                reason: "test done".into(),
                grace_seconds: 0,
            })),
            ..Default::default()
        };
        kernel.send("network", shutdown).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), loop_task)
            .await
            .expect("run_loop did not exit after PluginShutdown")
            .unwrap()
            .unwrap();
    }
}
