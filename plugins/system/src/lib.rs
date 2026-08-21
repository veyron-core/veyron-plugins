//! `system` plugin — local host queries and simple, reversible controls.
//!
//! One domain: the state of the machine this plugin runs on. P1 ships
//! read-only actions (`sys_info`, `sys_battery`, `sys_procs`, `sys_volume`);
//! setters (volume/mute/brightness/lock/power-profile) follow in P2 behind
//! the same dispatch. Deliberately NOT here: destructive or
//! differently-scoped actions (kill process, network config) — those would
//! turn this into shell-lite (see root `ROADMAP.md`).

pub mod backends;
pub mod detect;
pub mod error;
pub mod info;
pub mod runner;
#[cfg(target_os = "linux")]
pub mod upower;
pub mod volume;

use serde_json::json;

use crate::backends::SystemBackends;
use crate::error::SystemError;

pub const PLUGIN_ID: &str = "system";
pub const PLUGIN_VERSION: &str = "0.1.0";

/// Actions this plugin serves; must stay in sync with `plugin.json` and the
/// kernel refuses ambiguous manifest declarations anyway.
pub const ACTIONS: &[&str] = &["sys_info", "sys_battery", "sys_procs", "sys_volume"];

/// Dispatch one action to its backend.
///
/// Every P1 action takes no parameters: an absent or empty JSON object is
/// accepted, anything else is `ERR_SYS_BAD_PARAMS` — loud at the boundary,
/// per the authoring notes ("a manifest minimum is documentation, not a
/// check").
pub async fn handle_action(
    action: &str,
    params_json: &[u8],
    be: &SystemBackends,
) -> Result<serde_json::Value, SystemError> {
    if !ACTIONS.contains(&action) {
        return Err(SystemError::UnknownAction(action.to_string()));
    }
    expect_no_params(params_json)?;

    match action {
        "sys_info" => encode(info::sys_info()),
        "sys_procs" => encode(info::sys_procs()),
        "sys_battery" => match &be.battery {
            Some(b) => {
                let s = b.status().await?;
                encode(json!({
                    "percent": s.percent,
                    "state": s.state.as_str(),
                    "time_to_empty_s": s.time_to_empty_s,
                    "time_to_full_s": s.time_to_full_s,
                }))
            }
            None => Err(SystemError::NotSupported("battery")),
        },
        "sys_volume" => match &be.volume {
            Some(v) => {
                let s = v.get().await?;
                encode(json!({ "percent": s.percent, "muted": s.muted }))
            }
            None => Err(SystemError::NotSupported("volume")),
        },
        // The ACTIONS.contains guard above keeps this arm unreachable.
        other => Err(SystemError::UnknownAction(other.to_string())),
    }
}

/// Parameterless-action boundary check: empty buffer or `{}` only.
fn expect_no_params(params_json: &[u8]) -> Result<(), SystemError> {
    let trimmed = trim_ascii(params_json);
    if trimmed.is_empty() {
        return Ok(());
    }
    let v: serde_json::Value = serde_json::from_slice(trimmed)
        .map_err(|e| SystemError::BadParams(format!("params must be a JSON object: {e}")))?;
    match v {
        serde_json::Value::Object(ref map) if map.is_empty() => Ok(()),
        _ => Err(SystemError::BadParams(
            "this action takes no parameters".to_string(),
        )),
    }
}

/// Serialize a response value; encoding our own plain structs cannot fail,
/// but keep the error path typed instead of panicking.
fn encode(v: impl serde::Serialize) -> Result<serde_json::Value, SystemError> {
    serde_json::to_value(v).map_err(|e| SystemError::Backend(format!("encode response: {e}")))
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let not_ws = |b: &u8| !b.is_ascii_whitespace();
    let Some(start) = bytes.iter().position(not_ws) else {
        return &[];
    };
    let end = bytes.iter().rposition(not_ws).expect("start implies a non-ws byte exists");
    &bytes[start..=end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::{Battery, BatteryState, BatteryStatus, Volume, VolumeStatus};
    use std::sync::Arc;

    struct FakeBattery;
    #[async_trait::async_trait]
    impl Battery for FakeBattery {
        async fn status(&self) -> Result<BatteryStatus, SystemError> {
            Ok(BatteryStatus {
                percent: 87.5,
                state: BatteryState::Discharging,
                time_to_empty_s: Some(4210),
                time_to_full_s: None,
            })
        }
    }

    struct FakeVolume;
    #[async_trait::async_trait]
    impl Volume for FakeVolume {
        async fn get(&self) -> Result<VolumeStatus, SystemError> {
            Ok(VolumeStatus { percent: 42, muted: true })
        }
    }

    fn full_backends() -> SystemBackends {
        SystemBackends {
            battery: Some(Arc::new(FakeBattery)),
            volume: Some(Arc::new(FakeVolume)),
        }
    }

    #[tokio::test]
    async fn sys_battery_maps_backend_status() {
        let v = handle_action("sys_battery", b"{}", &full_backends()).await.expect("ok");
        assert_eq!(v["percent"], 87.5);
        assert_eq!(v["state"], "discharging");
        assert_eq!(v["time_to_empty_s"], 4210);
        assert_eq!(v["time_to_full_s"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn missing_backend_is_not_supported_naming_the_capability() {
        let be = SystemBackends::default();
        let e = handle_action("sys_battery", b"{}", &be).await.unwrap_err();
        assert_eq!(e.code().as_str(), "ERR_SYS_NOT_SUPPORTED");
        assert!(e.to_string().contains("battery"));

        let e = handle_action("sys_volume", b"{}", &be).await.unwrap_err();
        assert_eq!(e.code().as_str(), "ERR_SYS_NOT_SUPPORTED");
        assert!(e.to_string().contains("volume"));
    }

    #[tokio::test]
    async fn sys_volume_maps_backend_status() {
        let v = handle_action("sys_volume", b"", &full_backends()).await.expect("ok");
        assert_eq!(v["percent"], 42);
        assert_eq!(v["muted"], true);
    }

    #[tokio::test]
    async fn unknown_action_is_not_found() {
        let e = handle_action("sys_frobnicate", b"{}", &full_backends()).await.unwrap_err();
        assert_eq!(e.code().as_str(), "ERR_SYS_NOT_FOUND");
    }

    #[tokio::test]
    async fn rejects_nonempty_params_loudly() {
        let be = full_backends();
        let e = handle_action("sys_info", br#"{"foo":1}"#, &be).await.unwrap_err();
        assert_eq!(e.code().as_str(), "ERR_SYS_BAD_PARAMS");

        // Malformed JSON also lands on the same code, not a crash.
        let e = handle_action("sys_info", b"{broken", &be).await.unwrap_err();
        assert_eq!(e.code().as_str(), "ERR_SYS_BAD_PARAMS");

        // Whitespace-padded empty object is fine.
        assert!(handle_action("sys_info", b"  {} \n", &be).await.is_ok());
    }

    #[test]
    fn trim_ascii_handles_all_whitespace_buffers() {
        assert_eq!(trim_ascii(b""), b"");
        assert_eq!(trim_ascii(b"   "), b"");
        assert_eq!(trim_ascii(b"\n {} \t"), b"{}");
    }
}
