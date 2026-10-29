//! Registers IMC Browse as a web browser in Windows (per user, no admin rights) so it shows up in
//! Settings, Default apps, and can check whether it is the default. Windows does not let a program
//! make itself the default, so "Make default" opens that Settings page for the user to confirm.

use std::os::windows::process::CommandExt;
use windows::{
    Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegOpenKeyExW,
        RegQueryValueExW, RegSetValueExW,
    },
    core::PCWSTR,
};

const APP: &str = "IMC Browse";
const URL_ID: &str = "IMCBrowseURL";
const HTML_ID: &str = "IMCBrowseHTML";
const CAPS: &str = r"Software\Clients\StartMenuInternet\IMC Browse\Capabilities";

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Writes `value` (REG_SZ) under HKCU\key; `name` empty means the key's default value.
fn set(key: &str, name: &str, value: &str) -> bool {
    unsafe {
        let mut h = HKEY::default();
        let k = w(key);
        if RegCreateKeyExW(HKEY_CURRENT_USER, PCWSTR(k.as_ptr()), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &mut h, None).is_err() {
            return false;
        }
        let n = w(name);
        let v: Vec<u8> = w(value).iter().flat_map(|u| u.to_le_bytes()).collect();
        let ok = RegSetValueExW(h, PCWSTR(n.as_ptr()), None, REG_SZ, Some(&v)).is_ok();
        let _ = RegCloseKey(h);
        ok
    }
}

fn get(key: &str, name: &str) -> Option<String> {
    unsafe {
        let mut h = HKEY::default();
        let k = w(key);
        if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(k.as_ptr()), None, KEY_READ, &mut h).is_err() {
            return None;
        }
        let n = w(name);
        let mut buf = [0u16; 512];
        let mut len = (buf.len() * 2) as u32;
        let r = RegQueryValueExW(h, PCWSTR(n.as_ptr()), None, None, Some(buf.as_mut_ptr() as *mut u8), Some(&mut len));
        let _ = RegCloseKey(h);
        r.is_ok().then(|| String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1).min(buf.len())]))
    }
}

/// Makes the browser appear in Windows' list of browsers. Cheap and idempotent: skipped when the
/// recorded path is already this executable.
pub fn register() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let exe = exe.to_string_lossy().trim_start_matches(r"\?\").to_string();
    let cmd = format!("\"{exe}\" \"%1\"");
    let app_key = r"Software\Clients\StartMenuInternet\IMC Browse";
    if get(&format!(r"{app_key}\shell\open\command"), "").as_deref() == Some(&format!("\"{exe}\"")) {
        return true;
    }
    let icon = format!("{exe},0");
    let mut ok = true;
    for (id, desc, is_url) in [(URL_ID, "IMC Browse URL", true), (HTML_ID, "IMC Browse HTML Document", false)] {
        let base = format!(r"Software\Classes\{id}");
        ok &= set(&base, "", desc);
        if is_url {
            ok &= set(&base, "URL Protocol", "");
        }
        ok &= set(&format!(r"{base}\DefaultIcon"), "", &icon);
        ok &= set(&format!(r"{base}\shell\open\command"), "", &cmd);
    }
    ok &= set(app_key, "", APP);
    ok &= set(&format!(r"{app_key}\DefaultIcon"), "", &icon);
    ok &= set(&format!(r"{app_key}\shell\open\command"), "", &format!("\"{exe}\""));
    ok &= set(CAPS, "ApplicationName", APP);
    ok &= set(CAPS, "ApplicationDescription", "A fast, private browser with built-in blocking.");
    ok &= set(CAPS, "ApplicationIcon", &icon);
    for p in ["http", "https"] {
        ok &= set(&format!(r"{CAPS}\URLAssociations"), p, URL_ID);
    }
    for e in [".htm", ".html", ".xhtml", ".shtml"] {
        ok &= set(&format!(r"{CAPS}\FileAssociations"), e, HTML_ID);
    }
    ok &= set(r"Software\RegisteredApplications", APP, CAPS);
    ok
}

/// True when Windows currently sends both http and https links to this browser.
pub fn is_default() -> bool {
    ["http", "https"].iter().all(|p| {
        get(&format!(r"Software\Microsoft\Windows\Shell\Associations\UrlAssociations\{p}\UserChoice"), "ProgId").as_deref() == Some(URL_ID)
    })
}

/// Opens Windows' Default apps settings (the only supported way for the user to pick a browser).
pub fn open_settings() {
    let _ = std::process::Command::new("explorer.exe").arg("ms-settings:defaultapps").creation_flags(0x0800_0000).spawn();
}
