//! Native Chrome profile support.
//!
//! Discovers installed Chrome profiles on the host OS, leases one
//! exclusively for a `BrowserSession`, and releases it on drop. Uses a
//! voidcrawl-owned advisory lock file (`.voidcrawl.lock` inside the
//! profile directory) for cross-process exclusion. Chrome's own
//! `SingletonLock` still applies — this module only arbitrates between
//! voidcrawl consumers.

use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::ErrorKind,
    path::{Path, PathBuf},
    time::Duration,
};

use fs2::FileExt;
use tokio::time::{Instant, sleep};

use crate::{
    error::{Result, VoidCrawlError},
    session::{BrowserSession, BrowserSessionBuilder},
};

/// Descriptor returned by [`list_profiles`].
#[derive(Debug, Clone)]
pub struct ProfileInfo {
    /// Profile directory name as Chrome knows it (e.g. "Default", "Profile 1").
    pub name: String,
    /// Absolute path to the profile directory.
    pub path: PathBuf,
}

/// Live lease on a Chrome profile. Dropping the handle releases the
/// lock file; call [`ProfileHandle::close`] first for a graceful Chrome
/// shutdown.
pub struct ProfileHandle {
    name:    String,
    path:    PathBuf,
    session: Option<BrowserSession>,
    // Holding the File keeps the fs2 advisory lock; dropping it
    // releases via the OS.
    _lock:   File,
}

impl fmt::Debug for ProfileHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProfileHandle")
            .field("name", &self.name)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl ProfileHandle {
    /// Profile name (Chrome-facing, e.g. "Default").
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Absolute path to the profile directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Borrow the underlying `BrowserSession`. Use this to open pages.
    pub fn session(&self) -> Result<&BrowserSession> {
        self.session.as_ref().ok_or(VoidCrawlError::BrowserClosed)
    }

    /// Close the Chrome process gracefully. The lock file is released
    /// when the handle is dropped.
    pub async fn close(&mut self) -> Result<()> {
        if let Some(session) = self.session.take() {
            session.close().await?;
        }
        Ok(())
    }
}

/// Enumerate Chrome profiles discovered in the platform's default
/// user data directory. Returns an empty vec if the base directory
/// does not exist.
pub fn list_profiles() -> Result<Vec<ProfileInfo>> {
    let bases = chrome_user_data_dirs();
    let mut out = Vec::new();
    for base in &bases {
        if !base.is_dir() {
            continue;
        }
        let entries = fs::read_dir(base)
            .map_err(|e| VoidCrawlError::Other(format!("read_dir {}: {e}", base.display())))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            // Chrome profile dirs are "Default" or "Profile N" or
            // "Guest Profile". Filter to those that look like a real
            // profile (contain a "Preferences" file).
            if !is_profile_dir(&path) {
                continue;
            }
            out.push(ProfileInfo { name: name.to_string(), path });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Resolve a profile by name to its on-disk path.
pub fn resolve_profile(name: &str) -> Result<PathBuf> {
    let bases = chrome_user_data_dirs();
    let mut searched = Vec::new();
    for base in &bases {
        searched.push(base.display().to_string());
        let candidate = base.join(name);
        if is_profile_dir(&candidate) {
            return Ok(candidate);
        }
    }
    Err(VoidCrawlError::ProfileNotFound { name: name.to_string(), searched })
}

/// Acquire exclusive lease on a Chrome profile and launch Chrome
/// pointing at it. The caller is responsible for calling
/// [`ProfileHandle::close`] (or dropping the handle) when done.
///
/// `lease_timeout` is how long to poll for the lock before giving up
/// with [`VoidCrawlError::ProfileLeaseExpired`]. If another voidcrawl
/// process holds the lock for the entire duration, this returns
/// [`VoidCrawlError::ProfileBusy`].
pub async fn acquire_profile(name: &str, lease_timeout: Duration) -> Result<ProfileHandle> {
    acquire_profile_in(name, &chrome_user_data_dirs(), lease_timeout).await
}

/// Same as [`acquire_profile`] but searches the caller-supplied base
/// directories instead of the platform defaults. Useful for tests.
pub async fn acquire_profile_in(
    name: &str,
    bases: &[PathBuf],
    lease_timeout: Duration,
) -> Result<ProfileHandle> {
    let mut searched = Vec::new();
    let mut path = None;
    for base in bases {
        searched.push(base.display().to_string());
        let candidate = base.join(name);
        if is_profile_dir(&candidate) {
            path = Some(candidate);
            break;
        }
    }
    let path =
        path.ok_or_else(|| VoidCrawlError::ProfileNotFound { name: name.to_string(), searched })?;

    let lock_path = path.join(".voidcrawl.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| VoidCrawlError::Other(format!("open {}: {e}", lock_path.display())))?;

    // Poll for an exclusive advisory lock. fs2::try_lock_exclusive
    // returns immediately; we sleep between attempts so we don't
    // starve the tokio runtime.
    let deadline = Instant::now() + lease_timeout;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => break,
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(if lease_timeout.is_zero() {
                        VoidCrawlError::ProfileBusy { name: name.to_string() }
                    } else {
                        VoidCrawlError::ProfileLeaseExpired {
                            name:         name.to_string(),
                            timeout_secs: lease_timeout.as_secs(),
                        }
                    });
                }
                sleep(Duration::from_millis(100)).await;
            }
            Err(e) => {
                return Err(VoidCrawlError::Other(format!("lock {}: {e}", lock_path.display())));
            }
        }
    }

    let session = BrowserSessionBuilder::new().headful().user_data_dir(&path).launch().await?;

    Ok(ProfileHandle { name: name.to_string(), path, session: Some(session), _lock: file })
}

/// Release a profile lease explicitly. Equivalent to dropping the
/// handle, but awaits graceful Chrome shutdown first.
pub async fn release_profile(mut handle: ProfileHandle) -> Result<()> {
    handle.close().await
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn is_profile_dir(path: &Path) -> bool {
    path.is_dir() && path.join("Preferences").is_file()
}

/// Platform-specific Chrome user-data directory roots. Returns every
/// plausible base so users with Chrome + Chromium + Chrome Canary
/// installed all work.
pub fn chrome_user_data_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    #[cfg(target_os = "linux")]
    {
        if let Some(config) = dirs::config_dir() {
            out.push(config.join("google-chrome"));
            out.push(config.join("chromium"));
            out.push(config.join("google-chrome-beta"));
            out.push(config.join("google-chrome-unstable"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = dirs::home_dir() {
            let app_sup = home.join("Library").join("Application Support");
            out.push(app_sup.join("Google").join("Chrome"));
            out.push(app_sup.join("Chromium"));
            out.push(app_sup.join("Google").join("Chrome Canary"));
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(local) = dirs::data_local_dir() {
            out.push(local.join("Google").join("Chrome").join("User Data"));
            out.push(local.join("Chromium").join("User Data"));
        }
    }
    out
}
