//! `system` plugin binary: detect backends, hand them to the lib's
//! [`system_plugin::SystemPlugin`], run the stock SDK serve loop.

use std::sync::Arc;

use veyron_sdk::{Plugin, VeyronError};

use system_plugin::{detect, PLUGIN_ID};

#[tokio::main]
async fn main() -> Result<(), VeyronError> {
    let backends = Arc::new(detect::detect().await);
    println!(
        "[{PLUGIN_ID}] backends detected: battery={} volume={} brightness={} lock={} power={}",
        backends.battery.is_some(),
        backends.volume.is_some(),
        backends.brightness.is_some(),
        backends.lock.is_some(),
        backends.power.is_some()
    );
    system_plugin::SystemPlugin::new(backends).run().await
}
