//! Host info and process/memory stats via the `sysinfo` crate — one
//! cross-platform dependency covering Linux and macOS, so P3's macOS
//! subset gets these two actions for free.
//!
//! The reads are thin (call sysinfo, serialize); response *shapes* are
//! locked by tests on the serde structs, not on live host values.

use serde::Serialize;
use sysinfo::System;

/// `sys_info` response: identity of the host.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SysInfo {
    pub hostname: Option<String>,
    /// Long OS name, e.g. "Arch Linux" / "Debian GNU/Linux 12 (bookworm)".
    pub os: Option<String>,
    pub os_version: Option<String>,
    pub kernel: Option<String>,
    pub arch: Option<String>,
}

/// `sys_procs` response: load, process count, memory pressure.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SysProcs {
    pub process_count: usize,
    /// 1/5/15-minute load averages.
    pub load_avg: [f64; 3],
    pub memory_total_mb: u64,
    pub memory_used_mb: u64,
}

pub fn sys_info() -> SysInfo {
    SysInfo {
        hostname: System::host_name(),
        os: System::long_os_version(),
        os_version: System::os_version(),
        kernel: System::kernel_version(),
        arch: Some(normalize_arch(std::env::consts::ARCH).to_string()),
    }
}

pub fn sys_procs() -> SysProcs {
    // Load average and memory need a refreshed System; processes want the
    // full list. One new_all() covers all three for this call size.
    let sys = System::new_all();
    let load = System::load_average();
    SysProcs {
        process_count: sys.processes().len(),
        load_avg: [load.one, load.five, load.fifteen],
        memory_total_mb: sys.total_memory() / (1024 * 1024),
        memory_used_mb: sys.used_memory() / (1024 * 1024),
    }
}

/// Map Rust's arch triple onto the conventional short forms callers
/// expect (`x86_64`, `aarch64`, ...).
fn normalize_arch(arch: &str) -> &str {
    match arch {
        "x86_64" | "amd64" => "x86_64",
        "aarch64" | "arm64" => "aarch64",
        "x86" | "i686" => "x86",
        "arm" => "arm",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arch_normalizes_known_triples() {
        assert_eq!(normalize_arch("x86_64"), "x86_64");
        assert_eq!(normalize_arch("amd64"), "x86_64");
        assert_eq!(normalize_arch("aarch64"), "aarch64");
        assert_eq!(normalize_arch("arm64"), "aarch64");
        assert_eq!(normalize_arch("riscv64"), "riscv64");
    }

    #[test]
    fn sys_info_serializes_with_expected_fields() {
        let info = sys_info();
        let v = serde_json::to_value(&info).expect("serialize");
        for key in ["hostname", "os", "os_version", "kernel", "arch"] {
            assert!(v.get(key).is_some(), "missing field {key}");
        }
    }

    #[test]
    fn sys_procs_shape_is_sane_on_this_host() {
        let procs = sys_procs();
        // Live-host smoke: shape and basic sanity, not exact values.
        assert!(procs.load_avg[0] >= 0.0);
        assert!(procs.memory_total_mb > 0);
        assert!(procs.memory_used_mb <= procs.memory_total_mb);
    }
}
