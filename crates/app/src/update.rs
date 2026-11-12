//! Self-update from GitHub Releases.
//!
//! CI publishes a release for every push to main (see
//! `.github/workflows/macos-dmg.yml`) with `Nota-macos-arm64.zip` and its
//! SHA-256. Nota checks for a newer release a few seconds after launch and
//! every few hours, downloads and verifies it in the background, and offers
//! "Restart to update", which swaps the app bundle and relaunches.
//! Only an installed `.app` updates itself; `cargo run` builds never do.

use gpui::{Context, Task};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub const REPO: &str = "steven-lijw/nota";
const ASSET: &str = "Nota-macos-arm64.zip";

/// The version this build was made as (CI sets `NOTA_VERSION`).
pub fn current_version() -> &'static str {
    option_env!("NOTA_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

#[derive(Clone, Debug, PartialEq)]
pub enum UpdateStatus {
    Idle,
    Checking,
    UpToDate,
    Downloading(String),
    Ready { version: String, app: PathBuf },
    Failed(String),
}

pub struct Updater {
    pub status: UpdateStatus,
    /// Whether the last check was asked for (then "up to date" is worth showing).
    pub manual: bool,
    _check: Option<Task<()>>,
    _timer: Option<Task<()>>,
}

/// The `.app` bundle this process runs from, if any.
pub fn bundle_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    // …/Nota.app/Contents/MacOS/Nota
    let app = exe.parent()?.parent()?.parent()?;
    (app.extension().is_some_and(|e| e == "app")).then(|| app.to_path_buf())
}

pub fn can_self_update() -> bool {
    cfg!(target_os = "macos") && bundle_path().is_some()
}

fn parse_version(v: &str) -> Vec<u64> {
    v.trim().trim_start_matches('v').split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

fn newer(candidate: &str, current: &str) -> bool {
    parse_version(candidate) > parse_version(current)
}

fn curl(args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time", "300", "-H", "User-Agent: Nota-updater"])
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("could not run curl: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(out.stdout)
}

struct Release {
    version: String,
    zip_url: String,
    sha_url: Option<String>,
}

fn latest_release() -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = curl(&["-H", "Accept: application/vnd.github+json", &url])?;
    let v: serde_json::Value = serde_json::from_slice(&body).map_err(|e| format!("bad reply from GitHub: {e}"))?;
    let Some(tag) = v["tag_name"].as_str() else { return Ok(None) };
    let assets = v["assets"].as_array().cloned().unwrap_or_default();
    let find = |name: &str| {
        assets
            .iter()
            .find(|a| a["name"].as_str() == Some(name))
            .and_then(|a| a["browser_download_url"].as_str().map(String::from))
    };
    let Some(zip_url) = find(ASSET) else { return Ok(None) };
    Ok(Some(Release {
        version: tag.trim_start_matches('v').to_string(),
        zip_url,
        sha_url: find(&format!("{ASSET}.sha256")),
    }))
}

fn sha256(path: &Path) -> Result<String, String> {
    let out = Command::new("shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()
        .map_err(|e| format!("could not run shasum: {e}"))?;
    let s = String::from_utf8_lossy(&out.stdout);
    s.split_whitespace().next().map(|h| h.to_lowercase()).ok_or_else(|| "no checksum".into())
}

/// Download, verify and unpack a release; returns the new `Nota.app`.
fn download(r: &Release) -> Result<PathBuf, String> {
    let dir = dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("nota")
        .join("updates")
        .join(&r.version);
    let app = dir.join("Nota.app");
    if app.join("Contents/MacOS/Nota").is_file() && dir.join(".verified").is_file() {
        return Ok(app); // downloaded earlier
    }
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let zip = dir.join(ASSET);
    curl(&["-o", &zip.to_string_lossy(), &r.zip_url])?;
    let sha_url = r.sha_url.as_ref().ok_or("the release has no checksum")?;
    let expected = String::from_utf8_lossy(&curl(&[sha_url])?).split_whitespace().next().unwrap_or("").to_lowercase();
    if expected.is_empty() || sha256(&zip)? != expected {
        let _ = std::fs::remove_dir_all(&dir);
        return Err("the download did not match its checksum".into());
    }
    let ok = Command::new("ditto")
        .args(["-x", "-k"])
        .arg(&zip)
        .arg(&dir)
        .status()
        .map_err(|e| format!("could not unpack: {e}"))?;
    if !ok.success() || !app.join("Contents/MacOS/Nota").is_file() {
        return Err("the downloaded archive has no Nota.app".into());
    }
    let _ = std::fs::remove_file(&zip);
    let _ = std::fs::write(dir.join(".verified"), &r.version);
    Ok(app)
}

impl Updater {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut u = Updater { status: UpdateStatus::Idle, manual: false, _check: None, _timer: None };
        if can_self_update() {
            u._timer = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_secs(6)).await;
                loop {
                    if this.update(cx, |this, cx| this.check(false, cx)).is_err() {
                        break;
                    }
                    cx.background_executor().timer(Duration::from_secs(4 * 3600)).await;
                }
            }));
        }
        u
    }

    pub fn busy(&self) -> bool {
        matches!(self.status, UpdateStatus::Checking | UpdateStatus::Downloading(_))
    }

    /// Look for a newer release and download it.
    pub fn check(&mut self, manual: bool, cx: &mut Context<Self>) {
        if self.busy() || matches!(self.status, UpdateStatus::Ready { .. }) {
            return;
        }
        self.manual = manual;
        if !can_self_update() {
            if manual {
                self.status = UpdateStatus::Failed(
                    "Updates work in the installed app (Nota.app), not in development builds.".into(),
                );
                cx.notify();
            }
            return;
        }
        self.status = UpdateStatus::Checking;
        cx.notify();
        self._check = Some(cx.spawn(async move |this, cx| {
            let found = cx.background_executor().spawn(async { latest_release() }).await;
            let release = match found {
                Ok(Some(r)) if newer(&r.version, current_version()) => r,
                Ok(_) => {
                    this.update(cx, |this, cx| {
                        this.status = UpdateStatus::UpToDate;
                        cx.notify();
                    })
                    .ok();
                    return;
                }
                Err(e) => {
                    this.update(cx, |this, cx| {
                        this.status = UpdateStatus::Failed(format!("Could not check for updates: {e}"));
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let version = release.version.clone();
            this.update(cx, |this, cx| {
                this.status = UpdateStatus::Downloading(version.clone());
                cx.notify();
            })
            .ok();
            let result = cx.background_executor().spawn(async move { download(&release) }).await;
            this.update(cx, |this, cx| {
                this.status = match result {
                    Ok(app) => UpdateStatus::Ready { version, app },
                    Err(e) => UpdateStatus::Failed(format!("Could not download the update: {e}")),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    /// Replace the running app with the downloaded one and relaunch it.
    /// Returns an error message if that is not possible.
    pub fn install(&mut self, cx: &mut gpui::App) -> Result<(), String> {
        let UpdateStatus::Ready { app, .. } = &self.status else {
            return Err("No update is ready.".into());
        };
        let target = bundle_path().ok_or("Nota is not running from an app bundle.")?;
        let parent = target.parent().ok_or("Unknown install location.")?;
        let probe = parent.join(format!(".nota-update-{}", std::process::id()));
        if std::fs::write(&probe, b"").is_err() {
            return Err(format!(
                "Nota can't replace itself in {} (no permission). Drag the new version there yourself — it is in {}.",
                parent.display(),
                app.parent().map(|p| p.display().to_string()).unwrap_or_default()
            ));
        }
        let _ = std::fs::remove_file(&probe);
        // Wait for this process to exit, swap the bundles (rolling back if the
        // move fails), then start the new version.
        let script = r#"
while kill -0 "$NOTA_PID" 2>/dev/null; do sleep 0.2; done
rm -rf "$NOTA_TARGET.old"
if mv "$NOTA_TARGET" "$NOTA_TARGET.old"; then
  if mv "$NOTA_NEW" "$NOTA_TARGET"; then rm -rf "$NOTA_TARGET.old"; else mv "$NOTA_TARGET.old" "$NOTA_TARGET"; fi
fi
xattr -dr com.apple.quarantine "$NOTA_TARGET" 2>/dev/null
open "$NOTA_TARGET"
"#;
        Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .env("NOTA_PID", std::process::id().to_string())
            .env("NOTA_TARGET", &target)
            .env("NOTA_NEW", app)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Could not start the installer: {e}"))?;
        cx.quit();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(newer("0.1.12", "0.1.9"));
        assert!(newer("v0.2.0", "0.1.99"));
        assert!(!newer("0.1.9", "0.1.9"));
        assert!(!newer("0.1.8", "0.1.9"));
    }
}
