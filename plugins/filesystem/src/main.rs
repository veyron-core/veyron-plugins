//! `filesystem` plugin binary — sandboxed local file browse/read/write via
//! `fs_list` / `fs_read` / `fs_write`, gated by `PERMISSION_FILES_READ` and
//! `PERMISSION_FILES_WRITE`. No outbound RPC, no network, no secrets.
//!
//! Sandbox: only absolute paths inside `FILES_PLUGIN_ALLOWED_ROOTS` are
//! reachable. Unset/empty → every action is rejected (default-deny). See
//! ROADMAP.md for scope and non-goals.

use std::sync::Arc;

use filesystem_plugin::config::Config;
use filesystem_plugin::handler::Handler;
use filesystem_plugin::sandbox::Sandbox;
use vynkor_sdk::concurrent::serve_concurrent;
use vynkor_sdk::{VynkorClient, VynkorError};

#[tokio::main]
async fn main() -> Result<(), VynkorError> {
    let sandbox = Sandbox::from_env();
    let handler = Arc::new(Handler::new(sandbox, Config::from_env()));

    let client = VynkorClient::connect_from_env().await?;
    let token = std::env::var("VYN_JWT_TOKEN").unwrap_or_default();
    serve_concurrent(client, &token, handler).await?;

    eprintln!("[filesystem] shutting down");
    Ok(())
}
