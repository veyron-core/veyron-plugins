//! Audio player backends: `pw-cat --playback` (PipeWire), `paplay`
//! (PulseAudio), `aplay` (ALSA, wav) and `ffplay` (ffmpeg, any format).
//! Every playback spawns the binary directly with argv — never a shell —
//! so a crafted path or format string cannot inject commands.
//!
//! The [`Spawner`] trait is the process-execution boundary: tests inject
//! a fake so CI never touches real audio hardware or host binaries.

use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::Stdio;
#[cfg(test)]
use std::sync::atomic::Ordering;
#[cfg(test)]
use std::sync::{Arc, Mutex as StdMutex};
#[cfg(test)]
use std::time::Duration;

use async_trait::async_trait;
use tokio::process::{Child, Command};

/// Operator env var: pin one backend binary instead of falling through the chain.
pub const PLAYER_ENV: &str = "SOUND_PLUGIN_PLAYER";
/// Operator env var: default output device; a per-call `device` param wins.
pub const DEVICE_ENV: &str = "SOUND_PLUGIN_DEVICE";
/// Operator env var: hard cap on source size in bytes.
pub const MAX_BYTES_ENV: &str = "SOUND_PLUGIN_MAX_BYTES";

pub const DEFAULT_MAX_BYTES: usize = 33_554_432; // 32 MiB

/// Operator policy resolved once per call from env (constructed directly
/// in tests).
#[derive(Debug, Clone)]
pub struct Config {
    /// Cap on source size in bytes — file stat or decoded inline audio.
    pub max_bytes: usize,
    /// `SOUND_PLUGIN_PLAYER`: pin one backend binary.
    pub player_override: Option<String>,
    /// `SOUND_PLUGIN_DEVICE`: default output device; per-call param wins.
    pub default_device: Option<String>,
    /// Where inline audio temp files are written (`std::env::temp_dir()`;
    /// injectable so tests can assert creation/cleanup inside their own dir).
    pub temp_dir: PathBuf,
}

impl Config {
    pub fn from_env() -> Self {
        let env = |k: &str| std::env::var(k).ok();
        let player_override = env(PLAYER_ENV)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let default_device = env(DEVICE_ENV)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let max_bytes = env(MAX_BYTES_ENV)
            .and_then(|v| v.trim().parse().ok())
            .filter(|&n| n > 0)
            .unwrap_or(DEFAULT_MAX_BYTES);
        Self {
            max_bytes,
            player_override,
            default_device,
            temp_dir: std::env::temp_dir(),
        }
    }
}

// ---------------------------------------------------------------------------
// Provider chains and argv construction
// ---------------------------------------------------------------------------

/// Backend profiles. `volume`/`device` record whether the backend accepts
/// native volume/device flags — used to filter the auto chain when the
/// caller requests those capabilities.
#[derive(Clone, Copy)]
struct Profile {
    bin: &'static str,
    volume: bool,
    device: bool,
}

const PW_CAT: Profile = Profile {
    bin: "pw-cat",
    volume: true,
    device: true,
};
const PAPLAY: Profile = Profile {
    bin: "paplay",
    volume: true,
    device: true,
};
const APLAY: Profile = Profile {
    bin: "aplay",
    volume: false,
    device: true,
};
const FFPLAY: Profile = Profile {
    bin: "ffplay",
    volume: true,
    device: false,
};

const KNOWN_PROFILES: [Profile; 4] = [PW_CAT, PAPLAY, APLAY, FFPLAY];

/// Ordered candidate binaries for one play request.
///
/// Auto mode: wav tries `pw-cat` → `paplay` → `aplay`; every other format
/// goes straight to `ffplay` (ffmpeg decodes anything). Backends that can't
/// honor requested capabilities are dropped from the auto chain — `aplay`
/// has no volume flag, `ffplay` cannot target an output device. If nothing
/// supports the combination, the error names the conflict instead of
/// silently ignoring it.
///
/// Override mode (`SOUND_PLUGIN_PLAYER`): exactly that binary is used and
/// capability filtering is skipped — unsupported flags are omitted from the
/// argv, the operator pinned it knowing their setup.
///
/// Installed-ness is NOT checked here: spawn attempts fall through on
/// `ERR_SOUND_PROVIDER_MISSING`, same two-layer behavior as `clipboard`.
pub fn player_chain(
    format_is_wav: bool,
    volume: f64,
    device: Option<&str>,
    override_bin: Option<&str>,
) -> Result<Vec<String>, String> {
    if let Some(bin) = override_bin {
        let pinned = KNOWN_PROFILES
            .iter()
            .find(|p| p.bin == bin)
            .map(|p| p.bin.to_string())
            .unwrap_or_else(|| bin.to_string());
        return Ok(vec![pinned]);
    }
    let mut chain: Vec<Profile> = if format_is_wav {
        vec![PW_CAT, PAPLAY, APLAY]
    } else {
        vec![FFPLAY]
    };
    chain.retain(|p| (volume == 1.0 || p.volume) && (device.is_none() || p.device));
    if chain.is_empty() {
        let why = match (volume != 1.0, device.is_some()) {
            (true, true) => "no backend supports both volume and device for this format",
            (true, false) => "no backend supports volume for this format",
            (false, true) => "no backend supports device selection for this format",
            (false, false) => unreachable!("unfiltered auto chain is non-empty"),
        };
        return Err(format!(
            "ERR_SOUND_PROVIDER_MISSING: {why} \
             (format {}, volume {volume}, device '{}'); \
             pin SOUND_PLUGIN_PLAYER to force one",
            if format_is_wav { "wav" } else { "non-wav" },
            device.unwrap_or_default(),
        ));
    }
    Ok(chain.into_iter().map(|p| p.bin.to_string()).collect())
}

/// Format a linear volume multiplier compactly: 1.0 → "1", 0.5 → "0.5".
fn fmt_volume(volume: f64) -> String {
    format!("{volume}")
}

/// Build argv for one backend. The file path is ALWAYS the last argument.
/// Volume is passed natively where supported (linear multiplier for
/// pw-cat/paplay, integer percent for ffplay) and omitted otherwise.
pub fn build_args(player: &str, file: &str, volume: f64, device: Option<&str>) -> Vec<String> {
    match player {
        "pw-cat" => {
            let mut args = vec![
                "--playback".to_string(),
                format!("--volume={}", fmt_volume(volume)),
            ];
            if let Some(d) = device {
                args.push(format!("--target={d}"));
            }
            args.push(file.to_string());
            args
        }
        "paplay" => {
            let mut args = vec![format!("--volume={}", fmt_volume(volume))];
            if let Some(d) = device {
                args.push(format!("--device={d}"));
            }
            args.push(file.to_string());
            args
        }
        "aplay" => {
            // `-q`: no progress noise; aplay has no native volume flag.
            let mut args = vec!["-q".to_string()];
            if let Some(d) = device {
                args.extend(["-D".to_string(), d.to_string()]);
            }
            args.push(file.to_string());
            args
        }
        "ffplay" => {
            // `-nodisp` keeps SDL windows closed, `-autoexit` makes ffplay
            // quit at end of input so status converges to idle.
            let pct = ((volume * 100.0).round() as i64).clamp(0, 1000);
            vec![
                "-nodisp".to_string(),
                "-autoexit".to_string(),
                "-loglevel".to_string(),
                "error".to_string(),
                "-volume".to_string(),
                pct.to_string(),
                file.to_string(),
            ]
        }
        // Unknown operator override: pass the file through untouched.
        _ => vec![file.to_string()],
    }
}

// ---------------------------------------------------------------------------
// Process execution boundary
// ---------------------------------------------------------------------------

/// Handle to one spawned playback process.
#[async_trait]
pub trait Process: Send {
    /// Terminate the process (best-effort; safe to call more than once).
    fn start_kill(&mut self);
    /// Reap without blocking: Some once the process has exited.
    fn try_wait(&mut self) -> Option<i32>;
}

pub type BoxedProcess = Box<dyn Process>;

/// Process execution boundary — mocked in tests so CI never touches a real
/// audio stack.
#[async_trait]
pub trait Spawner: Send + Sync {
    /// Spawn `bin args` detached. Errors carry the ERR_SOUND_* taxonomy:
    /// PROVIDER_MISSING when the binary isn't installed (the caller falls
    /// through to the next candidate), SPAWN_FAILED otherwise.
    async fn spawn(&self, bin: &str, args: &[String]) -> Result<BoxedProcess, String>;
}

pub struct RealSpawner;

#[async_trait]
impl Spawner for RealSpawner {
    async fn spawn(&self, bin: &str, args: &[String]) -> Result<BoxedProcess, String> {
        let mut cmd = Command::new(bin);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // Dropping the handle after a kill reaps the child instead of
            // leaving a zombie until plugin exit.
            .kill_on_drop(true);

        match cmd.spawn() {
            Ok(child) => Ok(Box::new(RealProcess { child })),
            Err(e) if e.kind() == ErrorKind::NotFound => Err(format!(
                "ERR_SOUND_PROVIDER_MISSING: binary '{bin}' not found on PATH"
            )),
            Err(e) => Err(format!("ERR_SOUND_SPAWN_FAILED: spawn '{bin}' failed: {e}")),
        }
    }
}

struct RealProcess {
    child: Child,
}

#[async_trait]
impl Process for RealProcess {
    fn start_kill(&mut self) {
        let _ = self.child.start_kill();
    }

    fn try_wait(&mut self) -> Option<i32> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(status.code().unwrap_or(-1)),
            Ok(None) => None,
            // Polling error: report finished-with-failure rather than leak.
            Err(_) => Some(-1),
        }
    }
}

// ---------------------------------------------------------------------------
// Test doubles
// ---------------------------------------------------------------------------

/// Test double: records every invocation, returns canned results keyed by
/// binary name (unlisted binaries succeed). Spawned processes exit naturally
/// after `auto_exit_ms` (or run forever when None); after `start_kill` they
/// report exit code 137 (SIGKILL).
#[cfg(test)]
pub struct FakeSpawner {
    results: StdMutex<std::collections::HashMap<String, Result<(), String>>>,
    calls: StdMutex<Vec<(String, Vec<String>)>>,
    kill_log: Arc<std::sync::Mutex<Vec<String>>>,
    pub auto_exit_ms: Option<u64>,
}

#[cfg(test)]
impl FakeSpawner {
    /// Every spawn succeeds.
    pub fn ok(auto_exit_ms: Option<u64>) -> Self {
        Self::new(Vec::new(), auto_exit_ms)
    }

    /// Per-binary outcomes; unlisted binaries succeed.
    pub fn new(results: Vec<(&str, Result<(), String>)>, auto_exit_ms: Option<u64>) -> Self {
        Self {
            results: StdMutex::new(
                results
                    .into_iter()
                    .map(|(b, r)| (b.to_string(), r))
                    .collect(),
            ),
            calls: StdMutex::new(Vec::new()),
            kill_log: Arc::new(StdMutex::new(Vec::new())),
            auto_exit_ms,
        }
    }

    pub fn calls(&self) -> Vec<(String, Vec<String>)> {
        self.calls.lock().unwrap().clone()
    }

    pub fn killed_bins(&self) -> Vec<String> {
        self.kill_log.lock().unwrap().clone()
    }
}

#[cfg(test)]
#[async_trait]
impl Spawner for FakeSpawner {
    async fn spawn(&self, bin: &str, args: &[String]) -> Result<BoxedProcess, String> {
        self.calls
            .lock()
            .unwrap()
            .push((bin.to_string(), args.to_vec()));
        let outcome = self
            .results
            .lock()
            .unwrap()
            .get(bin)
            .cloned()
            .unwrap_or(Ok(()));
        match outcome {
            Ok(()) => Ok(Box::new(FakeProcess {
                bin: bin.to_string(),
                started_ms: unix_millis(),
                auto_exit_after_ms: self.auto_exit_ms,
                killed: std::sync::atomic::AtomicBool::new(false),
                kill_log: Arc::clone(&self.kill_log),
            })),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before epoch")
        .as_millis() as u64
}

#[cfg(test)]
struct FakeProcess {
    bin: String,
    started_ms: u64,
    auto_exit_after_ms: Option<u64>,
    killed: std::sync::atomic::AtomicBool,
    kill_log: Arc<StdMutex<Vec<String>>>,
}

#[cfg(test)]
impl FakeProcess {
    fn finished(&self) -> bool {
        self.killed.load(Ordering::SeqCst)
            || self
                .auto_exit_after_ms
                .map(|ms| unix_millis() >= self.started_ms + ms)
                .unwrap_or(false)
    }

    fn code(&self) -> i32 {
        if self.killed.load(Ordering::SeqCst) {
            137 // 128 + SIGKILL, mirrors what the kernel reports for a kill
        } else {
            0
        }
    }
}

#[cfg(test)]
#[async_trait]
impl Process for FakeProcess {
    fn start_kill(&mut self) {
        if !self.killed.swap(true, Ordering::SeqCst) {
            self.kill_log.lock().unwrap().push(self.bin.clone());
        }
    }

    fn try_wait(&mut self) -> Option<i32> {
        if self.finished() {
            Some(self.code())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_chain_tries_pwcat_paplay_aplay() {
        assert_eq!(
            player_chain(true, 1.0, None, None).unwrap(),
            vec!["pw-cat", "paplay", "aplay"]
        );
    }

    #[test]
    fn non_wav_goes_straight_to_ffplay() {
        assert_eq!(
            player_chain(false, 1.0, None, None).unwrap(),
            vec!["ffplay"]
        );
    }

    #[test]
    fn volume_request_drops_aplay_from_wav_chain() {
        assert_eq!(
            player_chain(true, 1.5, None, None).unwrap(),
            vec!["pw-cat", "paplay"]
        );
    }

    #[test]
    fn device_request_on_non_wav_names_the_conflict() {
        let err = player_chain(false, 1.0, Some("bluez_sink"), None).unwrap_err();
        assert!(err.contains("ERR_SOUND_PROVIDER_MISSING"), "{err}");
        assert!(err.contains("device"), "{err}");

        // wav chain keeps all three — all support device targeting.
        assert_eq!(
            player_chain(true, 1.0, Some("bluez_sink"), None).unwrap(),
            vec!["pw-cat", "paplay", "aplay"]
        );
    }

    #[test]
    fn volume_plus_device_conflict_names_both_constraints() {
        let err = player_chain(false, 2.0, Some("usb"), None).unwrap_err();
        assert!(err.contains("both volume and device"), "{err}");
    }

    #[test]
    fn override_pins_single_backend_and_skips_filtering() {
        // Even though aplay has no volume flag, the operator's pin wins.
        assert_eq!(
            player_chain(true, 3.0, None, Some("aplay")).unwrap(),
            vec!["aplay"]
        );
        // Unknown override passes through verbatim.
        assert_eq!(
            player_chain(false, 1.0, None, Some("my-player")).unwrap(),
            vec!["my-player"]
        );
    }

    #[test]
    fn args_pwcat_playback_volume_target_then_file_last() {
        let args = build_args("pw-cat", "/tmp/a.wav", 1.5, Some("sink0"));
        assert_eq!(
            args,
            vec!["--playback", "--volume=1.5", "--target=sink0", "/tmp/a.wav"]
        );
    }

    #[test]
    fn args_paplay_omits_device_when_unset() {
        let args = build_args("paplay", "/tmp/a.wav", 1.0, None);
        assert_eq!(args, vec!["--volume=1", "/tmp/a.wav"]);
    }

    #[test]
    fn args_aplay_uses_dash_d_for_device() {
        let args = build_args("aplay", "/tmp/a.wav", 1.0, Some("hw:0"));
        assert_eq!(args, vec!["-q", "-D", "hw:0", "/tmp/a.wav"]);
    }

    #[test]
    fn args_ffplay_percent_volume_and_quiet_flags() {
        let args = build_args("ffplay", "/tmp/a.mp3", 0.5, None);
        assert_eq!(
            args,
            vec![
                "-nodisp",
                "-autoexit",
                "-loglevel",
                "error",
                "-volume",
                "50",
                "/tmp/a.mp3"
            ]
        );
    }

    #[test]
    fn args_unknown_player_passes_file_through() {
        let args = build_args("my-player", "/tmp/a.flac", 2.0, Some("x"));
        assert_eq!(args, vec!["/tmp/a.flac"]);
    }

    #[tokio::test]
    async fn real_spawner_missing_binary_maps_to_provider_missing() {
        let err = match RealSpawner
            .spawn(
                "definitely-not-a-real-audio-bin-xyz",
                &["/dev/null".to_string()],
            )
            .await
        {
            Ok(_) => panic!("spawn of a nonexistent binary must fail"),
            Err(e) => e,
        };
        assert!(err.contains("ERR_SOUND_PROVIDER_MISSING"), "{err}");
    }

    #[tokio::test]
    async fn fake_process_reports_kill_and_natural_exit() {
        let sp = FakeSpawner::ok(Some(20));
        let mut proc = sp.spawn("pw-cat", &[]).await.unwrap();
        assert_eq!(proc.try_wait(), None, "must still be running right away");
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(proc.try_wait(), Some(0), "natural exit after auto_exit_ms");

        let mut proc2 = sp.spawn("paplay", &[]).await.unwrap();
        proc2.start_kill();
        assert_eq!(proc2.try_wait(), Some(137));
    }
}
