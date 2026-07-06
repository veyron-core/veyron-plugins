//! `network` plugin — outbound HTTP for other plugins/kernel, gated by
//! `PERMISSION_NETWORK`. See
//! docs/superpowers/specs/2026-07-05-network-plugin-design.md for the design.
//!
//! v1 is HTTP only. Needs real network egress: run with `sandbox: false`
//! in the kernel's `config.yaml` (see README.md).

use std::sync::Arc;

use network_plugin::{handler, request};
use veyron_sdk::proto::{envelope, ActionResponse, ActionStatus, Envelope, Event, PluginManifest};
use veyron_sdk::{Plugin, VeyronClient, VeyronError};

struct NetworkPlugin {
    client: reqwest::Client,
    redirect_client: reqwest::Client,
}

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

impl NetworkPlugin {
    fn new() -> Self {
        Self {
            client: Self::build_client(reqwest::redirect::Policy::none()),
            redirect_client: Self::build_client(reqwest::redirect::Policy::limited(
                request::MAX_REDIRECTS,
            )),
        }
    }

    /// Builds one `reqwest::Client` with every operator-configured option
    /// (SSRF resolver, proxy, CA bundle, client identity) applied — only
    /// `redirect` differs between `client` and `redirect_client`, so both
    /// share the same TLS/proxy/SSRF posture instead of drifting apart.
    fn build_client(redirect_policy: reqwest::redirect::Policy) -> reqwest::Client {
        // SSRF gating lives in `SsrfSafeResolver` (used for every connect,
        // including redirects) rather than a one-time pre-flight check —
        // see module docs on `SsrfSafeResolver`.
        let resolver = handler::SsrfSafeResolver {
            extra_blocklist: network_plugin::ssrf::Blocklist::from_env(),
            allowlist: network_plugin::ssrf::Allowlist::from_env(),
        };
        let mut builder = reqwest::Client::builder()
            .redirect(redirect_policy)
            .dns_resolver(Arc::new(resolver))
            // reqwest honors HTTP_PROXY/HTTPS_PROXY from the environment by
            // default; that would silently route requests around
            // SsrfSafeResolver. Turn it off — proxying is opt-in only via
            // `NETWORK_PLUGIN_PROXY_URL` below.
            .no_proxy();
        if let Ok(proxy_url) = std::env::var(PROXY_URL_ENV) {
            let proxy = reqwest::Proxy::all(&proxy_url)
                .unwrap_or_else(|e| panic!("invalid {PROXY_URL_ENV}: {e}"));
            builder = builder.proxy(proxy);
        }
        if let Ok(ca_path) = std::env::var(CA_BUNDLE_PATH_ENV) {
            let pem = std::fs::read(&ca_path)
                .unwrap_or_else(|e| panic!("failed to read {CA_BUNDLE_PATH_ENV} ({ca_path}): {e}"));
            let certs = reqwest::Certificate::from_pem_bundle(&pem)
                .unwrap_or_else(|e| panic!("invalid CA bundle at {ca_path}: {e}"));
            for cert in certs {
                builder = builder.add_root_certificate(cert);
            }
        }
        if let Ok(identity_path) = std::env::var(CLIENT_IDENTITY_PATH_ENV) {
            let pem = std::fs::read(&identity_path).unwrap_or_else(|e| {
                panic!("failed to read {CLIENT_IDENTITY_PATH_ENV} ({identity_path}): {e}")
            });
            let identity = reqwest::Identity::from_pem(&pem)
                .unwrap_or_else(|e| panic!("invalid client identity at {identity_path}: {e}"));
            builder = builder.identity(identity);
        }
        builder.build().expect("failed to build reqwest client")
    }

    async fn handle_http_request(&self, params_json: &[u8]) -> Result<Vec<u8>, String> {
        let params = request::parse_request(params_json)?;
        let client = if params.follow_redirects {
            &self.redirect_client
        } else {
            &self.client
        };
        let resp = handler::fetch(client, &params).await?;
        serde_json::to_vec(&serde_json::json!({
            "status": resp.status,
            "headers": resp.headers,
            "body": resp.body,
            "body_encoding": resp.body_encoding,
        }))
        .map_err(|e| format!("failed to encode response: {e}"))
    }
}

impl Plugin for NetworkPlugin {
    fn id(&self) -> &str {
        "network"
    }

    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            permissions: vec!["PERMISSION_NETWORK".into()],
            actions: vec!["http_request".into()],
            ..Default::default()
        }
    }

    async fn on_init(&mut self, _client: &mut VeyronClient) -> Result<(), VeyronError> {
        println!("[{}] registered with kernel", self.id());
        Ok(())
    }

    async fn on_message(&mut self, envelope: Envelope) -> Result<Option<Envelope>, VeyronError> {
        match envelope.payload {
            Some(envelope::Payload::ActionRequest(req)) if req.action == "http_request" => {
                let reply = match self.handle_http_request(&req.params_json).await {
                    Ok(data_json) => ActionResponse {
                        action_id: req.action_id,
                        status: ActionStatus::ActionOk as i32,
                        data_json,
                        error: String::new(),
                    },
                    Err(error) => ActionResponse {
                        action_id: req.action_id,
                        status: ActionStatus::ActionError as i32,
                        data_json: Vec::new(),
                        error,
                    },
                };
                Ok(Some(Envelope {
                    payload: Some(envelope::Payload::ActionResponse(reply)),
                    ..Default::default()
                }))
            }
            Some(envelope::Payload::ActionRequest(req)) => Ok(Some(Envelope {
                payload: Some(envelope::Payload::ActionResponse(ActionResponse {
                    action_id: req.action_id,
                    status: ActionStatus::ActionNotFound as i32,
                    data_json: Vec::new(),
                    error: format!("unknown action: {}", req.action),
                })),
                ..Default::default()
            })),
            other => {
                println!("[{}] unhandled message: {other:?}", self.id());
                Ok(None)
            }
        }
    }

    async fn on_event(&mut self, _event: Event) -> Result<Option<Envelope>, VeyronError> {
        Ok(None)
    }

    async fn on_shutdown(&mut self) -> Result<(), VeyronError> {
        println!("[{}] shutting down", self.id());
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), VeyronError> {
    NetworkPlugin::new().run().await
}
