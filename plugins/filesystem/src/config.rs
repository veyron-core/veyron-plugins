//! Operator configuration for the `filesystem` plugin, read from the
//! environment (the kernel's plugin supervisor translates the `config.yaml`
//! `env:` entry into these before spawning the plugin). Every knob is
//! prefixed `FILES_PLUGIN_*`.

use std::env;

/// Comma-separated absolute directory paths the plugin may touch.
/// Unset/empty = deny-all (every action rejected).
pub const ALLOWED_ROOTS_ENV: &str = "FILES_PLUGIN_ALLOWED_ROOTS";
/// Cap on `fs_list` entries.
pub const MAX_LIST_ENTRIES_ENV: &str = "FILES_PLUGIN_MAX_LIST_ENTRIES";
/// Default `fs_read` window.
pub const MAX_READ_BYTES_ENV: &str = "FILES_PLUGIN_MAX_READ_BYTES";

pub const DEFAULT_MAX_LIST_ENTRIES: usize = 1000;
pub const DEFAULT_MAX_READ_BYTES: u64 = 1024 * 1024;
/// Hard ceiling on any `fs_read` window — neither the operator default nor a
/// per-request `max_bytes` may exceed this.
pub const MAX_READ_BYTES_HARD_CAP: u64 = 8 * 1024 * 1024;

/// Operator policy resolved once at startup.
#[derive(Debug, Clone)]
pub struct Config {
    pub max_list_entries: usize,
    pub max_read_bytes: u64,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            max_list_entries: env::var(MAX_LIST_ENTRIES_ENV)
                .ok()
                .and_then(|v| v.trim().parse::<usize>().ok())
                .filter(|&n| n > 0)
                .unwrap_or(DEFAULT_MAX_LIST_ENTRIES),
            max_read_bytes: env::var(MAX_READ_BYTES_ENV)
                .ok()
                .and_then(|v| v.trim().parse::<u64>().ok())
                .filter(|&n| n > 0)
                .map(clamp_max_read)
                .unwrap_or(DEFAULT_MAX_READ_BYTES),
        }
    }
}

/// Clamp a read-window size to the hard cap.
pub fn clamp_max_read(n: u64) -> u64 {
    n.min(MAX_READ_BYTES_HARD_CAP)
}
