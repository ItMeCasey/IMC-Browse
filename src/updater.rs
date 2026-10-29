//! Self-update from GitHub releases (stable = the latest release, nightly = the rolling `nightly`
//! pre-release). Shared by the WebView2 and CEF builds.
//!
//! Safety: every release carries `update.json` plus `update.json.sig`, an Ed25519 signature made in
//! CI with a private key that only exists as a GitHub secret. The public half is compiled in below.
//! A download is installed only if the manifest signature verifies, the file's SHA-256 and size
//! match the manifest, and the update is newer than what is running. Local `dev` builds are never
//! overwritten.

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

pub const REPO: &str = "ItMeCasey/IMC-Browse";
/// Releases are signed with the matching private key (stored only as a GitHub Actions secret).
const PUBKEY_HEX: &str = "9e1849260e13d2db0525140285dae9c9c66d7da1b8141f964c25c1aee858c484";

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Commit id baked in by CI; "dev" for a local build.
pub const BUILD_ID: &str = match option_env!("IMC_BUILD_ID") {
    Some(s) => s,
    None => "dev",
};
/// UTC timestamp (yyyymmddHHMM) of the build, baked in by CI; 0 for a local build.
pub const BUILD_SEQ: u64 = parse_seq(match option_env!("IMC_BUILD_SEQ") {
    Some(s) => s,
    None => "0",
});
/// Chromium version of the bundled CEF runtime (CEF build only), baked in by CI.
const CEF_RUNTIME: Option<&str> = option_env!("IMC_CEF_RUNTIME");

const fn parse_seq(s: &str) -> u64 {
    let b = s.as_bytes();
    let (mut i, mut n) = (0, 0u64);
    while i < b.len() {
        if b[i] >= b'0' && b[i] <= b'9' {
            n = n * 10 + (b[i] - b'0') as u64;
        }
        i += 1;
    }
    n
}

pub fn can_install() -> bool {
    BUILD_ID != "dev"
}

pub fn release_page() -> String {
    format!("https://github.com/{REPO}/releases")
}

#[derive(Deserialize, Clone, Debug)]
pub struct Asset {
    pub name: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Manifest {
    /// "stable" or "nightly"
    pub channel: String,
    pub version: String,
    pub build: String,
    pub seq: u64,
    pub date: String,
    /// keyed by flavor: "webview2" or "cef"
    pub assets: HashMap<String, Asset>,
    #[serde(default)]
    pub cef_runtime: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Available {
    pub manifest: Manifest,
    pub flavor: &'static str,
    /// asset file name -> download URL (only github.com release URLs of this repo)
    urls: HashMap<String, String>,
    /// The update ships a new Chromium for the CEF build, so it needs the full package, not just the exe.
    pub needs_full: bool,
}

pub enum UpdMsg {
    Checked(Result<Option<Available>, String>),
    Installed(Result<String, String>),
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Checks `manifest` against an Ed25519 signature (hex) made with the key `key_hex`.
fn verify_with(key_hex: &str, manifest: &[u8], sig_hex: &str) -> Result<Manifest, String> {
    let key: [u8; 32] = hex_decode(key_hex).and_then(|v| v.try_into().ok()).ok_or("bad public key")?;
    let sig: [u8; 64] = hex_decode(sig_hex).and_then(|v| v.try_into().ok()).ok_or("bad signature file")?;
    let key = VerifyingKey::from_bytes(&key).map_err(|e| e.to_string())?;
    key.verify(manifest, &Signature::from_bytes(&sig)).map_err(|_| "update signature is not valid".to_string())?;
    serde_json::from_slice(manifest).map_err(|e| format!("bad update manifest: {e}"))
}

pub fn verify(manifest: &[u8], sig_hex: &str) -> Result<Manifest, String> {
    verify_with(PUBKEY_HEX, manifest, sig_hex)
}

fn semver(s: &str) -> (u64, u64, u64) {
    let mut p = s.trim_start_matches('v').split(['.', '-', '+']).map(|x| x.parse::<u64>().unwrap_or(0));
    (p.next().unwrap_or(0), p.next().unwrap_or(0), p.next().unwrap_or(0))
}

/// Is `m` newer than the running build? Stable compares versions; nightly compares build time.
pub fn is_newer(m: &Manifest, nightly: bool) -> bool {
    if nightly { m.seq > BUILD_SEQ && m.build != BUILD_ID } else { semver(&m.version) > semver(VERSION) }
}

fn curl(url: &str, secs: u32) -> Result<Vec<u8>, String> {
    use std::os::windows::process::CommandExt;
    let out = Command::new(crate::sandbox::system_exe("curl.exe"))
        .args(["-fsSL", "--max-time", &secs.to_string(), "-A", "IMC-Browse-updater", url])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()
        .map_err(|e| format!("curl is not available: {e}"))?;
    if out.status.success() { Ok(out.stdout) } else { Err(format!("download failed ({url})")) }
}

fn release_assets(nightly: bool) -> Result<HashMap<String, String>, String> {
    let url = if nightly { format!("https://api.github.com/repos/{REPO}/releases/tags/nightly") } else { format!("https://api.github.com/repos/{REPO}/releases/latest") };
    let v: serde_json::Value = serde_json::from_slice(&curl(&url, 20)?).map_err(|e| e.to_string())?;
    let prefix = format!("https://github.com/{REPO}/releases/download/");
    let mut m = HashMap::new();
    for a in v["assets"].as_array().into_iter().flatten() {
        if let (Some(n), Some(u)) = (a["name"].as_str(), a["browser_download_url"].as_str()) {
            if u.starts_with(&prefix) {
                m.insert(n.to_string(), u.to_string());
            }
        }
    }
    Ok(m)
}

/// Looks for a newer build on the chosen channel. `flavor` is "webview2" or "cef".
pub fn check(nightly: bool, flavor: &'static str) -> Result<Option<Available>, String> {
    let urls = release_assets(nightly)?;
    let (mu, su) = (urls.get("update.json"), urls.get("update.json.sig"));
    let (Some(mu), Some(su)) = (mu, su) else { return Ok(None) }; // nothing published on this channel yet
    let bytes = curl(mu, 20)?;
    let m = verify(&bytes, &String::from_utf8_lossy(&curl(su, 20)?))?;
    if m.channel != if nightly { "nightly" } else { "stable" } {
        return Err("update manifest is for another channel".into());
    }
    if !is_newer(&m, nightly) || !m.assets.contains_key(flavor) {
        return Ok(None);
    }
    let needs_full = flavor == "cef" && m.cef_runtime.is_some() && m.cef_runtime.as_deref() != CEF_RUNTIME;
    Ok(Some(Available { manifest: m, flavor, urls, needs_full }))
}

fn check_download(bytes: &[u8], asset: &Asset) -> Result<(), String> {
    if bytes.len() as u64 != asset.size {
        return Err("downloaded file has the wrong size".into());
    }
    if hex(&Sha256::digest(bytes)) != asset.sha256.to_ascii_lowercase() {
        return Err("downloaded file does not match its checksum".into());
    }
    Ok(())
}

fn side(exe: &Path, ext: &str) -> PathBuf {
    let mut n = exe.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    n.push(ext);
    exe.with_file_name(n)
}

/// The app itself is a DLL next to the launcher exe (CEF's bootstrap), named like it: imc-browse.dll.
/// That is the file updates replace; the launcher rarely changes.
fn app_dll() -> Option<PathBuf> {
    std::env::current_exe().ok().map(|e| e.with_extension("dll"))
}

/// Replaces `exe` with `new_bytes`. A running exe can be renamed but not overwritten, so the old
/// one is moved aside (`.old`, removed on the next start) and the new one takes its name.
fn swap_in(exe: &Path, new_bytes: &[u8]) -> Result<(), String> {
    let (new, old) = (side(exe, ".new"), side(exe, ".old"));
    let _ = fs::remove_file(&old);
    fs::write(&new, new_bytes).map_err(|e| format!("could not write the update: {e}"))?;
    fs::rename(exe, &old).map_err(|e| {
        let _ = fs::remove_file(&new);
        format!("could not move the current version aside: {e}")
    })?;
    if let Err(e) = fs::rename(&new, exe) {
        let _ = fs::rename(&old, exe); // put the working version back
        return Err(format!("could not put the update in place: {e}"));
    }
    Ok(())
}

/// Downloads, verifies and installs; returns the installed version. Takes effect on the next start.
pub fn install(a: &Available) -> Result<String, String> {
    if !can_install() {
        return Err("This is a local build, so it is never overwritten.".into());
    }
    if a.needs_full {
        return Err("This update includes a new Chromium. Download the full package from the release page.".into());
    }
    let asset = a.manifest.assets.get(a.flavor).ok_or("no download for this build")?;
    let url = a.urls.get(&asset.name).ok_or("download not found on the release")?;
    let bytes = curl(url, 180)?;
    check_download(&bytes, asset)?;
    let exe = app_dll().ok_or("cannot find the app file")?;
    swap_in(&exe, &bytes)?;
    Ok(if a.manifest.channel == "nightly" { format!("{} (nightly {})", a.manifest.version, a.manifest.build) } else { a.manifest.version.clone() })
}

/// Removes files left by a previous update.
pub fn cleanup() {
    if let Some(exe) = app_dll() {
        let _ = fs::remove_file(side(&exe, ".old"));
        let _ = fs::remove_file(side(&exe, ".new"));
    }
}

/// Update state shared by both builds' apps.
pub struct Ctl {
    pub available: Option<Available>,
    pub status: String,
    pub checking: bool,
    pub installing: bool,
    pub restart_pending: bool,
    manual: bool,
    notified: String,
    started: Instant,
    did_initial: bool,
}

pub struct Out {
    /// (message, optional (button label, command))
    pub toasts: Vec<(String, Option<(&'static str, &'static str)>)>,
    pub install: bool,
    pub checked: bool,
}

impl Ctl {
    pub fn new() -> Self {
        Self { available: None, status: String::new(), checking: false, installing: false, restart_pending: false, manual: false, notified: String::new(), started: Instant::now(), did_initial: false }
    }

    /// First check 30 s after start, then every 6 hours.
    pub fn due(&mut self, last_check_unix: u64) -> bool {
        if self.checking || self.installing || self.restart_pending {
            return false;
        }
        if !self.did_initial {
            if self.started.elapsed() < Duration::from_secs(30) {
                return false;
            }
            self.did_initial = true;
            return true;
        }
        now_unix().saturating_sub(last_check_unix) > 6 * 3600
    }

    pub fn start_check(&mut self, nightly: bool, flavor: &'static str, manual: bool, send: impl Fn(UpdMsg) + Send + 'static) {
        if self.checking || self.installing {
            return;
        }
        self.checking = true;
        self.manual = manual;
        self.status = "Checking for updates...".into();
        std::thread::spawn(move || send(UpdMsg::Checked(check(nightly, flavor))));
    }

    pub fn start_install(&mut self, send: impl Fn(UpdMsg) + Send + 'static) {
        let Some(a) = self.available.clone() else { return };
        if self.installing {
            return;
        }
        self.installing = true;
        self.status = "Downloading the update...".into();
        std::thread::spawn(move || send(UpdMsg::Installed(install(&a))));
    }

    /// Applies a result and says what to show or do next.
    pub fn handle(&mut self, msg: UpdMsg, auto: bool) -> Out {
        let mut out = Out { toasts: vec![], install: false, checked: false };
        match msg {
            UpdMsg::Checked(r) => {
                self.checking = false;
                out.checked = true;
                match r {
                    Ok(Some(a)) => {
                        let label = format!("{} ({})", a.manifest.version, if a.manifest.channel == "nightly" { format!("nightly {}", a.manifest.build) } else { "stable".into() });
                        self.status = format!("Upgrade available: {label}");
                        let fresh = self.notified != a.manifest.build;
                        self.notified = a.manifest.build.clone();
                        if auto && can_install() && !a.needs_full {
                            out.install = true;
                        } else if fresh || self.manual {
                            out.toasts.push((format!("Upgrade Available: IMC Browse {label}"), if can_install() && !a.needs_full { Some(("Update now", "update_install")) } else { Some(("View release", "update_page")) }));
                        }
                        self.available = Some(a);
                    }
                    Ok(None) => {
                        self.available = None;
                        self.status = "You are up to date.".into();
                        if self.manual {
                            out.toasts.push(("You are up to date.".into(), None));
                        }
                    }
                    Err(e) => {
                        self.status = format!("Could not check for updates: {e}");
                        if self.manual {
                            out.toasts.push((self.status.clone(), None));
                        }
                    }
                }
                self.manual = false;
            }
            UpdMsg::Installed(r) => {
                self.installing = false;
                match r {
                    Ok(v) => {
                        self.available = None;
                        self.restart_pending = true;
                        self.status = format!("Updated to {v}. Restart to use it.");
                        out.toasts.push((format!("Updated to {v}. Restart to finish."), Some(("Restart now", "restart"))));
                    }
                    Err(e) => {
                        self.status = format!("Update failed: {e}");
                        out.toasts.push((self.status.clone(), None));
                    }
                }
            }
        }
        out
    }

    pub fn state_json(&self, auto: bool, nightly: bool) -> serde_json::Value {
        let avail = self.available.as_ref().map(|a| serde_json::json!({"version": a.manifest.version, "channel": a.manifest.channel, "build": a.manifest.build, "needs_full": a.needs_full}));
        serde_json::json!({
            "version": VERSION,
            "build": BUILD_ID,
            "can_install": can_install(),
            "auto": auto,
            "nightly": nightly,
            "status": self.status,
            "available": avail,
            "checking": self.checking,
            "installing": self.installing,
            "restart_pending": self.restart_pending,
        })
    }
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn manifest_json(channel: &str, version: &str, build: &str, seq: u64) -> Vec<u8> {
        serde_json::json!({"channel": channel, "version": version, "build": build, "seq": seq, "date": "2026-10-30",
            "assets": {"webview2": {"name": "imc-browse.exe", "sha256": "00", "size": 1}}})
        .to_string()
        .into_bytes()
    }

    fn signed(sk: &SigningKey, bytes: &[u8]) -> (String, String) {
        (hex(sk.verifying_key().as_bytes()), hex(&sk.sign(bytes).to_bytes()))
    }

    #[test]
    fn signature_must_match_manifest_and_key() {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let m = manifest_json("stable", "9.9.9", "abc", 1);
        let (pk, sig) = signed(&sk, &m);
        assert_eq!(verify_with(&pk, &m, &sig).unwrap().version, "9.9.9");
        // tampered manifest
        let evil = manifest_json("stable", "9.9.9", "evil", 1);
        assert!(verify_with(&pk, &evil, &sig).is_err());
        // someone else's key
        let other = SigningKey::from_bytes(&[8u8; 32]);
        let (_, sig2) = signed(&other, &m);
        assert!(verify_with(&pk, &m, &sig2).is_err());
        // junk signature
        assert!(verify_with(&pk, &m, "zz").is_err());
    }

    #[test]
    fn newer_rules() {
        let m = |v: &str, seq| serde_json::from_slice::<Manifest>(&manifest_json("stable", v, "other", seq)).unwrap();
        assert!(is_newer(&m("99.0.0", 0), false));
        assert!(!is_newer(&m(VERSION, 0), false));
        assert!(!is_newer(&m("0.0.1", 0), false));
        // nightly: only a later build time counts
        assert!(is_newer(&m("0.1.0", BUILD_SEQ + 1), true));
        assert!(!is_newer(&m("0.1.0", BUILD_SEQ), true));
        assert_eq!(semver("v1.2.3-rc1"), (1, 2, 3));
    }

    #[test]
    fn download_must_match_size_and_hash() {
        let data = b"hello world";
        let good = Asset { name: "x".into(), sha256: hex(&Sha256::digest(data)), size: data.len() as u64 };
        assert!(check_download(data, &good).is_ok());
        assert!(check_download(b"hello worle", &good).is_err());
        assert!(check_download(data, &Asset { size: 3, ..good.clone() }).is_err());
    }

    #[test]
    fn swap_replaces_and_keeps_old_until_cleanup() {
        let d = std::env::temp_dir().join(format!("imc-upd-{}", now_unix()));
        fs::create_dir_all(&d).unwrap();
        let exe = d.join("app.exe");
        fs::write(&exe, b"old").unwrap();
        swap_in(&exe, b"new").unwrap();
        assert_eq!(fs::read(&exe).unwrap(), b"new");
        assert_eq!(fs::read(side(&exe, ".old")).unwrap(), b"old");
        assert!(!side(&exe, ".new").exists());
        let _ = fs::remove_dir_all(&d);
    }
}
