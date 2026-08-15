//! `sync-client` plugin binary. Loads config, builds the handler, then loops
//! reconnecting to the kernel (backoff 1s → 30s) so an offline device catches
//! up: each `serve_cycle` re-registers, re-subscribes and re-pulls the
//! snapshot, replacing the mirror.

use std::sync::Arc;
use std::time::Duration;

use sync_client_plugin::{serve_cycle, SyncClientHandler};
use veyron_sdk::{VeyronClient, VeyronError};

struct Config {
    heartbeat_secs: u64,
    device_id: String,
    snapshot_timeout_ms: u32,
}

fn load_config() -> Config {
    let heartbeat_secs = std::env::var("SYNC_CLIENT_HEARTBEAT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);
    let device_id = std::env::var("SYNC_CLIENT_DEVICE_ID")
        .unwrap_or_else(|_| std::env::var("HOSTNAME").unwrap_or_else(|_| "local".to_string()));
    let snapshot_timeout_ms = std::env::var("SYNC_CLIENT_SNAPSHOT_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5000);
    Config {
        heartbeat_secs,
        device_id,
        snapshot_timeout_ms,
    }
}

#[tokio::main]
async fn main() -> Result<(), VeyronError> {
    let config = load_config();
    let handler = Arc::new(SyncClientHandler::new(
        config.device_id,
        config.snapshot_timeout_ms,
    ));

    let mut backoff = Duration::from_secs(1);
    loop {
        match VeyronClient::connect_from_env().await {
            Ok(client) => {
                let token = std::env::var("VEYRON_JWT_TOKEN").unwrap_or_default();
                if let Err(e) =
                    serve_cycle(client, &token, handler.clone(), config.heartbeat_secs).await
                {
                    eprintln!("[sync-client] serve cycle ended: {e}");
                }
                backoff = Duration::from_secs(1);
            }
            Err(e) => eprintln!("[sync-client] connect failed: {e}"),
        }
        tokio::time::sleep(backoff).await;
        backoff = std::cmp::min(backoff * 2, Duration::from_secs(30));
    }
}
