//! Stateless file download with a built-in antivirus gate.
//!
//! The file is fetched through stealth Chrome into a **quarantine** directory,
//! scanned ([`void_crawl_core::scanner`]), and only then — if clean — moved
//! into the caller's output directory. A flagged file never leaves quarantine
//! and is deleted; the tool reports `ok=false` with a `reason`.

use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use void_crawl_core::{DownloadOutcome, ScanConfig, Verdict, VoidCrawlError, scanner};

use crate::{server::VoidCrawlServer, sessions::PendingDownload};

pub const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Environment variable that opts the `download` tool in. Unset (or
/// `0`/`false`/ empty) → the tool refuses. Downloads are off by default because
/// they pull untrusted bytes to disk over the session's live authenticated
/// cookies.
pub const ENABLE_ENV: &str = "VOIDCRAWL_ALLOW_DOWNLOADS";

/// Whether file downloads are enabled via [`ENABLE_ENV`].
fn downloads_enabled() -> bool {
    enabled_from(env::var(ENABLE_ENV).ok().as_deref())
}

/// Pure gate logic: enabled iff the value is present and not a falsey token.
fn enabled_from(value: Option<&str>) -> bool {
    match value {
        Some(v) => {
            let v = v.trim();
            !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
        }
        None => false,
    }
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct DownloadArgs {
    /// Absolute URL of the file to download.
    pub url:          String,
    /// Directory the scanned-clean file is placed in (created if missing).
    /// Defaults to a `voidcrawl-downloads` folder under the system temp dir.
    #[serde(default)]
    pub output_dir:   Option<String>,
    /// Reject downloads larger than this many bytes (default 100 MiB).
    #[serde(default)]
    pub max_bytes:    Option<u64>,
    /// Download + scan timeout in seconds (default 120).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DownloadResult {
    pub url:           String,
    /// `true` only when the file passed every scan and is on disk at `path`.
    pub ok:            bool,
    /// "clean" or "flagged".
    pub verdict:       String,
    /// Present when flagged: why the file was rejected.
    pub reason:        Option<String>,
    /// Absolute path to the clean file. `None` when flagged (file deleted).
    pub path:          Option<String>,
    /// MIME type inferred from the file's magic bytes, if recognized.
    pub detected_mime: Option<String>,
    /// Size of the downloaded file in bytes.
    pub size:          u64,
    /// Milliseconds queued for a free pool tab before work began.
    pub waited_ms:     u64,
}

pub async fn run(
    server: &VoidCrawlServer,
    args: DownloadArgs,
) -> Result<DownloadResult, VoidCrawlError> {
    // Opt-in capability: downloads pull untrusted bytes to disk over live auth
    // sessions, so the tool is inert unless the operator explicitly enables it.
    if !downloads_enabled() {
        return Err(VoidCrawlError::Other(format!(
            "file downloads are disabled; set {ENABLE_ENV}=1 to enable the `download` tool"
        )));
    }

    let output_dir = match &args.output_dir {
        Some(d) => PathBuf::from(d),
        None => env::temp_dir().join("voidcrawl-downloads"),
    };
    fs::create_dir_all(&output_dir)
        .map_err(|e| VoidCrawlError::Other(format!("create {}: {e}", output_dir.display())))?;

    // Quarantine lives *inside* the output dir so the post-scan move stays on
    // one filesystem (a cross-device rename would fail).
    let quarantine = tempfile::tempdir_in(&output_dir)
        .map_err(|e| VoidCrawlError::Other(format!("create quarantine dir: {e}")))?;

    let timeout = Duration::from_secs(args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
    let max_bytes = args.max_bytes.unwrap_or(scanner::DEFAULT_MAX_BYTES);

    // Download into quarantine over a pooled stealth tab.
    let pool = server.state().pool().await?;
    let (tab, waited_ms) = pool.acquire_timed().await?;
    let downloaded =
        tab.page.download_to_dir(&args.url, quarantine.path(), timeout, max_bytes).await;
    pool.release(tab).await;
    let downloaded = downloaded?;

    // Quarantine stays alive until the scan+move in `finalize` completes; on a
    // clean verdict the file is renamed out before `quarantine` drops here.
    let result = finalize(args.url, &downloaded, &output_dir, max_bytes, waited_ms);
    drop(quarantine);
    result
}

/// Scan a quarantined download and, if clean, move it into `output_dir`.
/// Builds the [`DownloadResult`] for both the URL and action download tools.
fn finalize(
    url: String,
    downloaded: &DownloadOutcome,
    output_dir: &Path,
    max_bytes: u64,
    waited_ms: u64,
) -> Result<DownloadResult, VoidCrawlError> {
    // Feed the server's claimed Content-Type (if the download captured one) so
    // the scanner can flag an executable disguised as a document.
    let cfg = ScanConfig { max_bytes, claimed_mime: downloaded.content_type.clone() };
    let report = scanner::scan_path(&downloaded.path, &cfg)?;

    match report.verdict {
        Verdict::Clean => {
            // Move the clean file out of quarantine into the output dir.
            let name = downloaded
                .path
                .file_name()
                .map_or_else(|| OsString::from("download"), OsString::from);
            let dest = unique_dest(output_dir, &name)?;
            fs::rename(&downloaded.path, &dest)
                .map_err(|e| VoidCrawlError::Other(format!("move clean file: {e}")))?;
            Ok(DownloadResult {
                url,
                ok: true,
                verdict: "clean".into(),
                reason: None,
                path: Some(dest.to_string_lossy().into_owned()),
                detected_mime: report.detected_mime,
                size: report.size,
                waited_ms,
            })
        }
        Verdict::Flagged { reason } => {
            // Leave the file in quarantine; the caller drops the TempDir, which
            // deletes it. Nothing clean-looking is left behind.
            Ok(DownloadResult {
                url,
                ok: false,
                verdict: "flagged".into(),
                reason: Some(reason),
                path: None,
                detected_mime: report.detected_mime,
                size: report.size,
                waited_ms,
            })
        }
    }
}

// ── Action-triggered downloads (arm → click → wait) on a session ─────────

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct DownloadArmArgs {
    /// Session whose next download (triggered by a click) to capture.
    pub session_id: String,
    /// Directory the scanned-clean file is placed in (created if missing).
    /// Defaults to a `voidcrawl-downloads` folder under the system temp dir.
    #[serde(default)]
    pub output_dir: Option<String>,
    /// Reject downloads larger than this many bytes (default 100 MiB).
    #[serde(default)]
    pub max_bytes:  Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DownloadArmResult {
    pub armed:   bool,
    pub message: String,
}

#[derive(Debug, Deserialize, JsonSchema, Default)]
pub struct DownloadWaitArgs {
    /// Session that was armed with `download_arm`.
    pub session_id:   String,
    /// Seconds to wait for the download to land (default 120).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

fn disabled_err() -> VoidCrawlError {
    VoidCrawlError::Other(format!("file downloads are disabled; set {ENABLE_ENV}=1 to enable"))
}

/// Arm a session to capture the file produced by the *next* download-triggering
/// action (e.g. a `click_by_role` on a "Download" button). Pair with
/// `download_wait` after performing that action.
pub async fn arm(
    server: &VoidCrawlServer,
    args: DownloadArmArgs,
) -> Result<DownloadArmResult, VoidCrawlError> {
    if !downloads_enabled() {
        return Err(disabled_err());
    }
    let session =
        server.state().sessions.get(&args.session_id).await.ok_or_else(|| {
            VoidCrawlError::Other(format!("no such session: {}", args.session_id))
        })?;

    // Reject a second arm while one is pending, rather than silently clobbering
    // the first capture (and discarding any download already in flight).
    if session.pending_download.lock().await.is_some() {
        return Err(VoidCrawlError::Other(
            "a download is already armed on this session; call download_wait first".into(),
        ));
    }

    let output_dir = match &args.output_dir {
        Some(d) => PathBuf::from(d),
        None => env::temp_dir().join("voidcrawl-downloads"),
    };
    fs::create_dir_all(&output_dir)
        .map_err(|e| VoidCrawlError::Other(format!("create {}: {e}", output_dir.display())))?;
    let quarantine = tempfile::tempdir_in(&output_dir)
        .map_err(|e| VoidCrawlError::Other(format!("create quarantine dir: {e}")))?;
    let max_bytes = args.max_bytes.unwrap_or(scanner::DEFAULT_MAX_BYTES);

    let capture = {
        let page = session.page.lock().await;
        page.arm_download(quarantine.path(), max_bytes).await?
    };
    *session.pending_download.lock().await =
        Some(PendingDownload { capture, quarantine, output_dir, max_bytes });

    Ok(DownloadArmResult {
        armed:   true,
        message: "download armed — perform the click that triggers it, then call download_wait"
            .into(),
    })
}

/// Wait for the download armed by `download_arm` to land, scan it, and (if
/// clean) move it into the output dir. Returns the same shape as `download`.
pub async fn wait(
    server: &VoidCrawlServer,
    args: DownloadWaitArgs,
) -> Result<DownloadResult, VoidCrawlError> {
    if !downloads_enabled() {
        return Err(disabled_err());
    }
    let session =
        server.state().sessions.get(&args.session_id).await.ok_or_else(|| {
            VoidCrawlError::Other(format!("no such session: {}", args.session_id))
        })?;

    let pending = session.pending_download.lock().await.take().ok_or_else(|| {
        VoidCrawlError::Other("no armed download for this session; call download_arm first".into())
    })?;
    let PendingDownload { capture, quarantine, output_dir, max_bytes } = pending;
    let timeout = Duration::from_secs(args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));

    // Poll for the file WITHOUT holding the session page lock, so other tools on
    // this session (clicks, content, close) aren't blocked for the whole wait.
    let downloaded = capture.poll(timeout).await;
    // Reset download behavior with a brief lock, regardless of outcome.
    {
        let page = session.page.lock().await;
        page.reset_download_behavior().await;
    }
    let downloaded = downloaded?;

    let result =
        finalize(format!("session:{}", args.session_id), &downloaded, &output_dir, max_bytes, 0);
    drop(quarantine);
    result
}

/// A destination path in `dir` for `name` that does not clobber an existing
/// file: `name`, then `name.1`, `name.2`, … . Errors rather than overwrite if
/// the namespace is exhausted.
fn unique_dest(dir: &Path, name: &OsString) -> Result<PathBuf, VoidCrawlError> {
    let base = dir.join(name);
    if !base.exists() {
        return Ok(base);
    }
    let stem = name.to_string_lossy();
    (1..10_000).map(|i| dir.join(format!("{stem}.{i}"))).find(|p| !p.exists()).ok_or_else(|| {
        VoidCrawlError::Other(format!("no free filename for {stem:?} in {}", dir.display()))
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test harness")]

    use super::enabled_from;

    #[test]
    fn gate_is_off_by_default() {
        assert!(!enabled_from(None));
    }

    #[test]
    fn gate_rejects_falsey_tokens() {
        for v in ["", "  ", "0", "false", "FALSE", "False"] {
            assert!(!enabled_from(Some(v)), "{v:?} should be disabled");
        }
    }

    #[test]
    fn gate_accepts_truthy_tokens() {
        for v in ["1", "true", "yes", "on"] {
            assert!(enabled_from(Some(v)), "{v:?} should be enabled");
        }
    }
}
