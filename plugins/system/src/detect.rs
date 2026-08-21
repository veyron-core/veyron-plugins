//! Runtime backend detection: probe the host once at startup and fill
//! [`SystemBackends`] with what actually exists. Everything undetected
//! stays `None` → `ERR_SYS_NOT_SUPPORTED` at call time.
//!
//! Probes are cheap one-shot calls (`wpctl --version`, one UPower property
//! read) made before registration; the serve loop never pays for them.

use std::sync::Arc;

use crate::backends::{SystemBackends, Volume};
use crate::runner::{CommandRunner, RunnerError, SharedRunner};
use crate::volume::{PactlVolume, WpctlVolume};

/// Detect every capability. Never fails: missing pieces become `None`.
pub async fn detect() -> SystemBackends {
    let runner: SharedRunner = Arc::new(crate::runner::RealRunner);
    SystemBackends {
        battery: detect_battery().await,
        volume: detect_volume(Arc::clone(&runner)).await,
    }
}

/// Battery backend, when a usable UPower DisplayDevice answers.
#[cfg(target_os = "linux")]
async fn detect_battery() -> Option<Arc<dyn crate::backends::Battery>> {
    match zbus::Connection::system().await {
        Ok(conn) => match crate::upower::UpowerBattery::connect(conn).await {
            Ok(b) => Some(Arc::new(b)),
            // No UPower on this host (headless server, container) — fine.
            Err(_) => None,
        },
        Err(_) => None,
    }
}

#[cfg(not(target_os = "linux"))]
async fn detect_battery() -> Option<Arc<dyn crate::backends::Battery>> {
    // P3: pmset -g batt parse (macOS). Until then the action reports
    // ERR_SYS_NOT_SUPPORTED.
    None
}

/// Volume provider selection: `wpctl` (PipeWire) preferred, `pactl`
/// (PulseAudio/pipewire-pulse) fallback. A provider is "present" when its
/// binary exists and answers `--version`; a present-but-broken tool is
/// still selected so callers see its real error instead of silent absence.
async fn detect_volume(runner: SharedRunner) -> Option<Arc<dyn Volume>> {
    if probe(&*runner, "wpctl").await {
        return Some(Arc::new(WpctlVolume::new(runner)));
    }
    if probe(&*runner, "pactl").await {
        return Some(Arc::new(PactlVolume::new(runner)));
    }
    None
}

/// True when `program --version` spawns successfully (exit status is not
/// required — some tools version to stderr; only spawn failure counts).
async fn probe(runner: &dyn CommandRunner, program: &str) -> bool {
    !matches!(runner.run(program, &["--version"]).await, Err(RunnerError::NotFound(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{RunOutcome, RunResult};
    use std::collections::HashSet;
    use std::sync::Mutex;

    struct FakeRunner {
        /// Programs that "exist" on this fake PATH.
        present: HashSet<&'static str>,
        log: Mutex<Vec<String>>,
    }

    impl FakeRunner {
        fn new(present: &[&'static str]) -> Self {
            Self { present: present.iter().copied().collect(), log: Mutex::new(Vec::new()) }
        }

        fn ran(&self) -> Vec<String> {
            self.log.lock().expect("log lock").clone()
        }
    }

    #[async_trait::async_trait]
    impl CommandRunner for FakeRunner {
        async fn run(&self, program: &str, _args: &[&str]) -> RunResult {
            self.log.lock().expect("log lock").push(program.to_string());
            if self.present.contains(program) {
                Ok(RunOutcome { ok: true, stdout: "version 1\n".into(), stderr: String::new() })
            } else {
                Err(RunnerError::NotFound(program.to_string()))
            }
        }
    }

    #[tokio::test]
    async fn prefers_wpctl_when_present() {
        let runner = Arc::new(FakeRunner::new(&["wpctl", "pactl"]));
        let vol = detect_volume(Arc::clone(&runner) as SharedRunner).await;
        assert!(vol.is_some());
        // Only wpctl was probed; pactl never needed.
        assert_eq!(runner.ran(), vec!["wpctl".to_string()]);
    }

    #[tokio::test]
    async fn falls_back_to_pactl_without_wpctl() {
        let runner = Arc::new(FakeRunner::new(&["pactl"]));
        let vol = detect_volume(Arc::clone(&runner) as SharedRunner).await;
        assert!(vol.is_some());
        assert_eq!(runner.ran(), vec!["wpctl".to_string(), "pactl".to_string()]);
    }

    #[tokio::test]
    async fn no_provider_without_any_tool() {
        let runner = Arc::new(FakeRunner::new(&[]));
        let vol = detect_volume(Arc::clone(&runner) as SharedRunner).await;
        assert!(vol.is_none());
        assert_eq!(runner.ran(), vec!["wpctl".to_string(), "pactl".to_string()]);
    }
}
