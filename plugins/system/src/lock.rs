//! Session locking: try the desktop's native lock interface first
//! (`org.freedesktop.ScreenSaver` on the session bus — GNOME/KDE/XFCE all
//! implement it), fall back to spawning `loginctl lock-session` which asks
//! logind to broadcast the Lock signal for the caller's session.
//!
//! There is no cheap "is a locker present?" probe for either path, so this
//! capability is always detected as present on Linux and call-time failures
//! surface as `ERR_SYS_BACKEND` naming both attempts.

use zbus::Connection;

use crate::error::SystemError;
use crate::runner::CommandRunner;

#[zbus::proxy(
    interface = "org.freedesktop.ScreenSaver",
    default_service = "org.freedesktop.ScreenSaver",
    default_path = "/org/freedesktop/ScreenSaver"
)]
trait ScreenSaver {
    fn lock(&self) -> zbus::Result<()>;
}

pub struct SessionBusLock {
    conn: Connection,
    runner: Arc<dyn CommandRunner>,
}

use std::sync::Arc;

use crate::runner::SharedRunner;

impl SessionBusLock {
    pub fn new(conn: Connection, runner: SharedRunner) -> Self {
        Self { conn, runner }
    }
}

#[async_trait::async_trait]
impl super::backends::SessionLock for SessionBusLock {
    async fn lock(&self) -> Result<(), SystemError> {
        match self.lock_via_screensaver().await {
            Ok(()) => Ok(()),
            Err(saver_err) => self.lock_via_logind(saver_err).await,
        }
    }
}

impl SessionBusLock {
    async fn lock_via_screensaver(&self) -> Result<(), SystemError> {
        let proxy = ScreenSaverProxy::new(&self.conn)
            .await
            .map_err(|e| SystemError::Backend(format!("ScreenSaver proxy: {e}")))?;
        proxy
            .lock()
            .await
            .map_err(|e| SystemError::Backend(format!("ScreenSaver.Lock: {e}")))
    }

    async fn lock_via_logind(&self, screensaver_err: SystemError) -> Result<(), SystemError> {
        let out = self
            .runner
            .run("loginctl", &["lock-session"])
            .await
            .map_err(|e| {
                SystemError::Backend(format!(
                    "lock failed via both paths — {screensaver_err}; loginctl: {e}"
                ))
            })?;
        if !out.ok {
            return Err(SystemError::Backend(format!(
                "lock failed via both paths — {screensaver_err}; loginctl exited nonzero: {}",
                out.stderr.trim()
            )));
        }
        Ok(())
    }
}
