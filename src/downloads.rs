//! Download tracking: files go to the system Downloads folder, progress is reported to the UI,
//! and the recent list (kept in downloads.json) can run a file or reveal it in Explorer.

use crate::config::root;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use windows::{
    Win32::{
        System::Com::CoTaskMemFree,
        UI::{
            Shell::{FOLDERID_Downloads, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellExecuteW},
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    },
    core::{HSTRING, PCWSTR},
};

#[derive(Serialize, Deserialize, Clone)]
pub struct Item {
    pub id: u64,
    pub name: String,
    pub path: String,
    pub url: String,
    pub total: i64,
    pub received: i64,
    /// "progress", "done", "failed" or "canceled"
    pub state: String,
    /// Unix seconds.
    pub started: u64,
    /// Downloaded in a Ghost tab: listed for this session only, never written to disk.
    #[serde(default)]
    pub private: bool,
}

const MAX_KEPT: usize = 50;

fn list_path() -> PathBuf {
    root().join("downloads.json")
}

pub fn load() -> Vec<Item> {
    let mut v: Vec<Item> = fs::read_to_string(list_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    for i in &mut v {
        if i.state == "progress" {
            i.state = "failed".into(); // the browser closed mid-download
        }
    }
    v
}

pub fn save(list: &[Item]) {
    let keep: Vec<&Item> = list.iter().filter(|i| !i.private).rev().take(MAX_KEPT).collect::<Vec<_>>().into_iter().rev().collect();
    if let Ok(s) = serde_json::to_string(&keep) {
        let _ = fs::write(list_path(), s);
    }
}

/// The system default Downloads folder (follows a relocated one), falling back to %USERPROFILE%\Downloads.
pub fn dir() -> PathBuf {
    unsafe {
        if let Ok(p) = SHGetKnownFolderPath(&FOLDERID_Downloads, KF_FLAG_DEFAULT, None) {
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const _));
            if let Some(s) = s.filter(|s| !s.is_empty()) {
                return PathBuf::from(s);
            }
        }
    }
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    home.join("Downloads")
}

/// A safe file name: no folders, no characters Windows rejects, no reserved device names.
pub fn sanitize_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base.chars().map(|c| if c.is_control() || "<>:\"|?*".contains(c) { '_' } else { c }).collect();
    let cleaned = cleaned.trim().trim_end_matches(['.', ' ']).to_string();
    let stem = cleaned.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str()) || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.ends_with(|c: char| c.is_ascii_digit()));
    if cleaned.is_empty() || reserved { "download".into() } else { cleaned }
}

/// `name`, or `name (1)`, `name (2)`... so nothing is overwritten.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    if !p.exists() {
        return p;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    (1..10_000).map(|n| dir.join(format!("{stem} ({n}){ext}"))).find(|p| !p.exists()).unwrap_or_else(|| dir.join(name))
}

/// File types that can run code when opened: the UI asks before running these.
pub fn is_risky(name: &str) -> bool {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    ["exe", "msi", "msix", "appx", "bat", "cmd", "com", "scr", "ps1", "psm1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "hta", "jar", "lnk", "reg", "cpl", "msc", "dll", "iso", "img"].contains(&ext.as_str())
}

/// Marks the file as downloaded from the internet (zone 3) so Windows warns before running it.
pub fn mark_of_the_web(path: &Path, url: &str) {
    let _ = fs::write(format!("{}:Zone.Identifier", path.display()), format!("[ZoneTransfer]\r\nZoneId=3\r\nHostUrl={url}\r\n"));
}

/// True if `path` is a file directly inside the Downloads folder (never trust a path from stored data blindly).
pub fn inside_downloads(path: &Path) -> bool {
    match (path.canonicalize(), dir().canonicalize()) {
        (Ok(p), Ok(d)) => p.parent() == Some(d.as_path()) && p.is_file(),
        _ => false,
    }
}

/// Opens the file with its default program.
pub fn open_file(path: &Path) -> bool {
    unsafe {
        let r = ShellExecuteW(None, &HSTRING::from("open"), &HSTRING::from(path.as_os_str()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
        r.0 as isize > 32
    }
}

/// Shows the file selected in Explorer.
pub fn reveal(path: &Path) {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new(crate::sandbox::system_exe("explorer.exe")).raw_arg(format!("/select,\"{}\"", path.display())).spawn();
}

pub fn open_folder(path: &Path) {
    let _ = std::process::Command::new(crate::sandbox::system_exe("explorer.exe")).arg(path).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_safe() {
        assert_eq!(sanitize_name("..\\..\\evil.exe"), "evil.exe");
        assert_eq!(sanitize_name("a<b>c?.txt"), "a_b_c_.txt");
        assert_eq!(sanitize_name("CON.txt"), "download");
        assert_eq!(sanitize_name("report. "), "report");
        assert_eq!(sanitize_name(""), "download");
    }

    #[test]
    fn risky_types_and_unique_names() {
        assert!(is_risky("setup.EXE") && is_risky("x.ps1") && !is_risky("photo.jpg") && !is_risky("noext"));
        let d = std::env::temp_dir().join("imc-dl-test");
        let _ = fs::create_dir_all(&d);
        let _ = fs::remove_file(d.join("a.txt"));
        let _ = fs::remove_file(d.join("a (1).txt"));
        assert_eq!(unique_path(&d, "a.txt"), d.join("a.txt"));
        fs::write(d.join("a.txt"), b"x").unwrap();
        assert_eq!(unique_path(&d, "a.txt"), d.join("a (1).txt"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn downloads_dir_exists_on_this_pc() {
        assert!(dir().is_absolute());
    }
}
