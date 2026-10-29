//! Extension installer + Firefox compatibility shim.
//!
//! Runtime: the embedded Chromium's own extension host (loaded with --load-extension), so
//! `chrome.*` is real, not emulated. It only runs Manifest V3, so older (MV2) Chrome extensions and
//! most Firefox add-ons are converted to V3 when installed (best effort, with warnings). This module gets extensions *onto disk* in
//! the unpacked form that host loads: Chrome Web Store ids, `.crx`, Firefox
//! `.xpi`/AMO URLs, `.zip`, or an unpacked folder. For Firefox add-ons it adds a
//! `browser` -> `chrome` alias so `browser.*` code runs unmodified.

use crate::config::extensions_dir;
use serde_json::{Value, json};
use std::{
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    process::Command,
};

const COMPAT_JS: &str = "if (typeof globalThis.browser === 'undefined' && typeof globalThis.chrome !== 'undefined') { globalThis.browser = globalThis.chrome; }\n";

/// The 32-letter extension id in a Chrome Web Store address, or the .xpi address for a Firefox add-on page.
fn store_link(src: &str) -> Option<String> {
    let rest = src.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/')?;
    if host == "chromewebstore.google.com" || (host == "chrome.google.com" && path.starts_with("webstore")) {
        let id = path.split(['/', '?', '#']).find(|s| s.len() == 32 && s.bytes().all(|b| (b'a'..=b'p').contains(&b)))?;
        return Some(id.to_string());
    }
    if host == "addons.mozilla.org" {
        let mut parts = path.split(['/', '?', '#']);
        parts.find(|p| *p == "addon")?;
        let slug = parts.next().filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'%' || b == b'.'))?;
        return Some(format!("https://addons.mozilla.org/firefox/downloads/latest/{slug}/latest.xpi"));
    }
    None
}

pub fn install(src: &str) -> Result<String, String> {
    let src = src.trim();
    if let Some(found) = store_link(src) {
        return install(&found);
    }
    let is_cws_id = src.len() == 32 && src.bytes().all(|b| (b'a'..=b'p').contains(&b));
    if is_cws_id {
        let url = format!(
            "https://clients2.google.com/service/update2/crx?response=redirect&prodversion=130.0&acceptformat=crx2,crx3&x=id%3D{src}%26uc"
        );
        return install_bytes(&download(&url)?, Some(src));
    }
    if src.starts_with("https://") {
        return install_bytes(&download(src)?, None);
    }
    let p = PathBuf::from(src);
    if p.is_dir() {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "ext".into());
        let dest = extensions_dir().join(sanitize(&name));
        copy_dir(&p, &dest).map_err(|e| e.to_string())?;
        finish(&dest)
    } else if p.is_file() {
        install_bytes(&fs::read(&p).map_err(|e| e.to_string())?, None)
    } else {
        Err(format!("not a Web Store id, https URL, file or folder: {src}"))
    }
}

pub fn list() -> Vec<(String, String, String)> {
    let mut out = vec![];
    if let Ok(rd) = fs::read_dir(extensions_dir()) {
        for e in rd.flatten() {
            if e.path().join(REMOVED_MARK).exists() {
                continue;
            }
            if let Some(m) = read_manifest(&e.path()) {
                out.push((
                    e.file_name().to_string_lossy().into_owned(),
                    m["name"].as_str().unwrap_or("?").to_string(),
                    m["version"].as_str().unwrap_or("?").to_string(),
                ));
            }
        }
    }
    out
}

pub fn remove(name: &str) -> Result<(), String> {
    let dir = extensions_dir().join(sanitize(name));
    if !dir.is_dir() {
        return Err(format!("no such extension: {name}"));
    }
    // The engine still has the extension loaded: pulling its files away makes it open a window of its own.
    // Hide it now and delete it on the next start.
    fs::write(dir.join(REMOVED_MARK), b"").map_err(|e| e.to_string())
}

const REMOVED_MARK: &str = ".imc-removed";

/// Deletes extensions removed in an earlier run. Called before the engine starts.
pub fn purge_removed() {
    if let Ok(rd) = fs::read_dir(extensions_dir()) {
        for e in rd.flatten() {
            if e.path().join(REMOVED_MARK).exists() {
                let _ = fs::remove_dir_all(e.path());
            }
        }
    }
}

fn download(url: &str) -> Result<Vec<u8>, String> {
    // curl.exe ships with Windows 10 1803+; avoids linking a TLS stack just for installs.
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("imc-dl-{}-{nanos}.bin", std::process::id()));
    use std::os::windows::process::CommandExt;
    if !url.starts_with("https://") {
        return Err("only https addresses can be downloaded".into());
    }
    let st = Command::new(crate::sandbox::system_exe("curl.exe"))
        .creation_flags(0x0800_0000) // no console window
        // https only, also across redirects; the address goes after `--` so it can never be read as an option
        .args(["-fsSL", "--proto", "=https", "--proto-redir", "=https", "--max-filesize", "104857600", "--max-time", "120", "-o"])
        .arg(&tmp)
        .arg("--")
        .arg(url)
        .status()
        .map_err(|e| format!("curl.exe failed to start: {e}"))?;
    if !st.success() {
        let _ = fs::remove_file(&tmp);
        return Err(format!("download failed: {url}"));
    }
    let b = fs::read(&tmp).map_err(|e| e.to_string())?;
    let _ = fs::remove_file(tmp);
    Ok(b)
}

/// Strip the CRX2/CRX3 header; plain zips/xpis pass through.
fn zip_payload(b: &[u8]) -> Result<&[u8], String> {
    if b.len() > 16 && &b[..4] == b"Cr24" {
        let ver = u32::from_le_bytes(b[4..8].try_into().unwrap());
        let start = match ver {
            3 => 12 + u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize,
            2 => 16 + u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize + u32::from_le_bytes(b[12..16].try_into().unwrap()) as usize,
            v => return Err(format!("unsupported CRX version {v}")),
        };
        return b.get(start..).ok_or_else(|| "truncated CRX".to_string());
    }
    Ok(b)
}

fn install_bytes(b: &[u8], id: Option<&str>) -> Result<String, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(zip_payload(b)?)).map_err(|e| format!("not a valid extension package: {e}"))?;
    let mut name = id.map(str::to_string);
    if name.is_none() {
        let mut s = String::new();
        zip.by_name("manifest.json").map_err(|_| "package has no manifest.json".to_string())?.read_to_string(&mut s).map_err(|e| e.to_string())?;
        let m: Value = serde_json::from_str(s.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
        name = Some(sanitize(m["name"].as_str().unwrap_or("extension")));
    }
    let dest = extensions_dir().join(name.unwrap());
    // a package is not allowed to be a zip bomb
    const MAX_FILES: usize = 20_000;
    const MAX_FILE: u64 = 50 << 20;
    const MAX_TOTAL: u64 = 250 << 20;
    if zip.len() > MAX_FILES {
        return Err("package has too many files".into());
    }
    let mut total = 0u64;
    for i in 0..zip.len() {
        let f = zip.by_index(i).map_err(|e| e.to_string())?;
        total += f.size();
        if f.size() > MAX_FILE || total > MAX_TOTAL {
            return Err("package is too large".into());
        }
    }
    let _ = fs::remove_dir_all(&dest);
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| e.to_string())?;
        // enclosed_name() rejects `..` and absolute paths (zip-slip).
        let Some(rel) = f.enclosed_name() else { continue };
        let out = dest.join(rel);
        if f.is_dir() {
            fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        } else {
            if let Some(p) = out.parent() {
                fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            let mut data = Vec::new();
            (&mut f).take(MAX_FILE + 1).read_to_end(&mut data).map_err(|e| e.to_string())?;
            if data.len() as u64 > MAX_FILE {
                return Err("package is too large".into());
            }
            fs::write(&out, data).map_err(|e| e.to_string())?;
        }
    }
    finish(&dest)
}

pub fn read_manifest(dir: &Path) -> Option<Value> {
    let s = fs::read_to_string(dir.join("manifest.json")).ok()?;
    serde_json::from_str(s.trim_start_matches('\u{feff}')).ok()
}

fn is_host_pattern(p: &str) -> bool {
    p == "<all_urls>" || p.contains("://")
}

/// Moves URL-pattern entries out of `from` into `to` (MV2 mixed them; MV3 separates them).
fn split_perms(m: &mut Value, from: &str, to: &str) {
    let Some(arr) = m.get(from).and_then(Value::as_array).cloned() else { return };
    let (hosts, perms): (Vec<Value>, Vec<Value>) = arr.into_iter().partition(|v| v.as_str().is_some_and(is_host_pattern));
    m[from] = Value::Array(perms);
    if !hosts.is_empty() {
        let mut all = m.get(to).and_then(Value::as_array).cloned().unwrap_or_default();
        all.extend(hosts);
        m[to] = Value::Array(all);
    }
}

/// Best-effort Manifest V2 -> V3 conversion. Returns warnings about what could not be carried over.
/// The original is kept as manifest.mv2.json.
pub fn convert_to_mv3(dir: &Path, m: &mut Value) -> Vec<String> {
    let mut w = vec![];
    m["manifest_version"] = json!(3);
    if let Some(o) = m.as_object_mut() {
        let a = o.remove("browser_action");
        let b = o.remove("page_action");
        if let Some(act) = a.or(b) {
            o.insert("action".into(), act);
        }
        if o.remove("sidebar_action").is_some() {
            w.push("sidebar panel is not supported and was removed".into());
        }
    }
    split_perms(m, "permissions", "host_permissions");
    split_perms(m, "optional_permissions", "optional_host_permissions");
    if let Some(perms) = m.get_mut("permissions").and_then(Value::as_array_mut) {
        let before = perms.len();
        perms.retain(|p| !matches!(p.as_str(), Some("webRequestBlocking") | Some("background") | Some("browsingData")));
        if perms.len() != before {
            w.push("blocking webRequest and a few legacy permissions are not available in Manifest V3 (the browser has a built-in blocker)".into());
        }
    }
    if let Some(arr) = m.get("web_accessible_resources").and_then(Value::as_array).cloned() {
        if arr.iter().all(Value::is_string) {
            m["web_accessible_resources"] = json!([{"resources": arr, "matches": ["<all_urls>"]}]);
        }
    }
    if m["content_security_policy"].is_string() {
        m["content_security_policy"] = json!({"extension_pages": "script-src 'self'; object-src 'self'"});
        w.push("custom content security policy replaced with the Manifest V3 default".into());
    }
    if let Some(bg) = m.get("background").and_then(Value::as_object).cloned() {
        if let Some(scripts) = bg.get("scripts").and_then(Value::as_array) {
            let list: Vec<String> = scripts.iter().filter_map(|v| v.as_str()).map(|s| format!("{:?}", s.trim_start_matches('/'))).collect();
            let js = format!("// generated: runs the old background scripts in a Manifest V3 service worker\nglobalThis.browser = globalThis.chrome;\nimportScripts({});\n", list.join(", "));
            if fs::write(dir.join("imc-bg.js"), js).is_ok() {
                m["background"] = json!({"service_worker": "imc-bg.js"});
                w.push("background scripts now run as a service worker (no page or DOM); add-ons that need a persistent page may not work".into());
            }
        } else if let Some(page) = bg.get("page").and_then(Value::as_str) {
            // a background page becomes a service worker that loads the same scripts, in order
            let page = page.trim_start_matches('/');
            let base = Path::new(page).parent().map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
            let html = fs::read_to_string(dir.join(page)).unwrap_or_default();
            let mut list: Vec<String> = vec![];
            let lower = html.to_ascii_lowercase();
            let mut at = 0;
            while let Some(i) = lower[at..].find("<script") {
                let start = at + i;
                let end = lower[start..].find('>').map(|e| start + e).unwrap_or(lower.len());
                let tag = &html[start..end];
                let tl = tag.to_ascii_lowercase();
                if let Some(k) = tl.find("src=") {
                    let rest = &tag[k + 4..];
                    let q = rest.chars().next().filter(|c| *c == '"' || *c == '\'');
                    let src = match q {
                        Some(q) => rest[1..].split(q).next().unwrap_or(""),
                        None => rest.split(|c: char| c.is_whitespace() || c == '>').next().unwrap_or(""),
                    };
                    if !src.is_empty() && !src.contains("://") {
                        let full = if src.starts_with('/') || base.is_empty() { src.trim_start_matches('/').to_string() } else { format!("{base}/{src}") };
                        list.push(format!("{full:?}"));
                    }
                }
                at = end.max(start + 1);
            }
            if list.is_empty() {
                m.as_object_mut().map(|o| o.remove("background"));
                w.push("background page has no scripts to carry over and was removed".into());
            } else {
                let js = format!("// generated: runs the old background page's scripts in a Manifest V3 service worker
globalThis.browser = globalThis.chrome;
importScripts({});
", list.join(", "));
                if fs::write(dir.join("imc-bg.js"), js).is_ok() {
                    m["background"] = json!({"service_worker": "imc-bg.js"});
                    w.push("background page now runs as a service worker (no DOM); add-ons that need a page may not work".into());
                }
            }
        }
    }
    w
}

/// Adds the `browser` alias to the extension's own HTML pages (popup, options) so `browser.*` runs there too.
fn shim_pages(dir: &Path, m: &Value) {
    let mut pages: Vec<String> = vec![];
    for p in ["/action/default_popup", "/browser_action/default_popup", "/options_ui/page", "/options_page"] {
        if let Some(s) = m.pointer(p).and_then(Value::as_str) {
            pages.push(s.trim_start_matches('/').to_string());
        }
    }
    for page in pages {
        let path = dir.join(&page);
        let Ok(html) = fs::read_to_string(&path) else { continue };
        if html.contains("imc-compat.js") {
            continue;
        }
        let tag = "<script src=\"/imc-compat.js\"></script>";
        let out = match html.to_ascii_lowercase().find("<head>") {
            Some(i) => format!("{}{tag}{}", &html[..i + 6], &html[i + 6..]),
            None => format!("{tag}{html}"),
        };
        let _ = fs::write(&path, out);
    }
}

fn finish(dir: &Path) -> Result<String, String> {
    let mut m = read_manifest(dir).ok_or("manifest.json missing or invalid")?;
    let firefox = m.get("browser_specific_settings").is_some() || m.get("applications").is_some();
    let mut note = String::new();
    let mut changed = false;
    if m["manifest_version"].as_u64().unwrap_or(2) < 3 {
        let _ = fs::copy(dir.join("manifest.json"), dir.join("manifest.mv2.json"));
        let warns = convert_to_mv3(dir, &mut m);
        for w in warns {
            note.push_str(&format!("\n  note: {w}"));
        }
        changed = true;
    }
    if firefox {
        fs::write(dir.join("imc-compat.js"), COMPAT_JS).map_err(|e| e.to_string())?;
        if let Some(cs) = m.get_mut("content_scripts").and_then(Value::as_array_mut) {
            for c in cs {
                if let Some(js) = c.get_mut("js").and_then(Value::as_array_mut) {
                    if !js.iter().any(|v| v == "imc-compat.js") {
                        js.insert(0, json!("imc-compat.js"));
                        changed = true;
                    }
                }
            }
        }
        if m.pointer("/background/service_worker").is_some() && !dir.join("imc-bg.js").exists() {
            note.push_str("\n  note: MV3 service worker: the browser.* alias is not injected there");
        }
        shim_pages(dir, &m);
    }
    if changed {
        fs::write(dir.join("manifest.json"), serde_json::to_string_pretty(&m).unwrap()).map_err(|e| e.to_string())?;
    }
    Ok(format!(
        "installed {} {} -> {}{note}\nRestart the browser to load it.",
        m["name"].as_str().unwrap_or("?"),
        m["version"].as_str().unwrap_or(""),
        dir.display()
    ))
}

/// The id Chromium gives an unpacked extension: SHA-256 of the folder path (as UTF-16), first 16
/// bytes, written with the letters a-p.
fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = vec![];
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b' ' | 13 | 10 => continue,
            _ => return None,
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// The id Chromium gives an extension: unpacked ones from the folder path, but one whose manifest has a
/// `key` (everything from the Chrome Web Store) from that key, so it keeps its store id.
pub fn ext_id(dir: &Path) -> String {
    use sha2::{Digest, Sha256};
    if let Some(key) = read_manifest(dir).and_then(|m| m["key"].as_str().and_then(b64_decode)) {
        return Sha256::digest(&key)[..16].iter().flat_map(|b| [b >> 4, b & 15]).map(|n| (b'a' + n) as char).collect();
    }
    id_for_path(dir)
}

pub fn id_for_path(dir: &Path) -> String {
    use sha2::{Digest, Sha256};
    let canon = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let text = canon.to_string_lossy().trim_start_matches(r"\\?\").to_string();
    let bytes: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    Sha256::digest(&bytes)[..16].iter().flat_map(|b| [b >> 4, b & 15]).map(|n| (b'a' + n) as char).collect()
}

/// Folders to pass to --load-extension, and the ones skipped (with why), so the engine never shows its own error box.
/// A disabled extension keeps its files; a marker file inside its folder tells the browser not to load it.
const DISABLED_MARK: &str = ".imc-disabled";

pub fn set_enabled(dir: &str, on: bool) -> Result<(), String> {
    let d = extensions_dir().join(sanitize(dir));
    if !d.is_dir() {
        return Err(format!("no such extension: {dir}"));
    }
    let mark = d.join(DISABLED_MARK);
    if on { let _ = fs::remove_file(mark); Ok(()) } else { fs::write(mark, b"").map_err(|e| e.to_string()) }
}

pub fn load_dirs() -> (Vec<String>, Vec<(String, String)>) {
    let (mut ok, mut skipped) = (vec![], vec![]);
    purge_removed();
    if let Ok(rd) = fs::read_dir(extensions_dir()) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().join(DISABLED_MARK).exists() {
                continue;
            }
            match read_manifest(&e.path()) {
                None => skipped.push((name, "manifest.json is missing or invalid".into())),
                Some(m) if m["manifest_version"].as_u64() == Some(3) => ok.push(e.path().to_string_lossy().into_owned()),
                Some(_) => skipped.push((name, "uses the old Manifest V2 format: reinstall it to convert".into())),
            }
        }
    }
    (ok, skipped)
}

fn sanitize(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    if t.is_empty() { "ext".into() } else { t }
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)? {
        let e = e?;
        let t = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &t)?;
        } else {
            fs::copy(e.path(), t)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crx3_header_is_stripped() {
        let mut b = b"Cr24".to_vec();
        b.extend(3u32.to_le_bytes());
        b.extend(4u32.to_le_bytes());
        b.extend([9, 9, 9, 9]);
        b.extend(b"PKzip");
        assert_eq!(zip_payload(&b).unwrap(), b"PKzip");
    }
}

/// What the toolbar needs to show and open an installed extension.
#[derive(Clone)]
pub struct Item {
    pub dir: String,
    pub name: String,
    /// data: URI of the toolbar icon, if the manifest has one.
    pub icon: Option<String>,
    pub popup: Option<String>,
    pub options: Option<String>,
    /// Chromium's id for it (used in chrome-extension:// addresses).
    pub id: String,
    /// Why it is not loaded, if it is not.
    pub error: Option<String>,
    /// Made for Firefox (it has Firefox-specific settings).
    pub firefox: bool,
    pub version: String,
    /// Switched off in Settings (kept on disk, not loaded).
    pub disabled: bool,
}

fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len() * 4 / 3 + 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// "__MSG_appName__" -> the string from _locales (default locale, then en).
fn localized(dir: &Path, m: &Value, s: &str) -> String {
    let Some(key) = s.strip_prefix("__MSG_").and_then(|k| k.strip_suffix("__")) else { return s.to_string() };
    let def = m["default_locale"].as_str().unwrap_or("en");
    for loc in [def, "en", "en_US"] {
        if let Some(t) = fs::read_to_string(dir.join("_locales").join(loc).join("messages.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
            if let Some(v) = t.as_object().and_then(|o| o.iter().find(|(k, _)| k.eq_ignore_ascii_case(key))).and_then(|(_, v)| v["message"].as_str()) {
                return v.to_string();
            }
        }
    }
    s.to_string()
}

fn icon_uri(dir: &Path, m: &Value) -> Option<String> {
    let act = if m["action"].is_object() { &m["action"] } else { &m["browser_action"] };
    let pick = |v: &Value| -> Option<String> {
        match v {
            Value::String(s) => Some(s.clone()),
            Value::Object(o) => o.iter().filter_map(|(k, v)| Some((k.parse::<u32>().ok()?, v.as_str()?.to_string()))).max_by_key(|(k, _)| *k).map(|(_, v)| v),
            _ => None,
        }
    };
    let rel = pick(&act["default_icon"]).or_else(|| pick(&m["icons"]))?;
    let path = dir.join(rel.trim_start_matches('/'));
    let bytes = fs::read(&path).ok().filter(|b| b.len() < 200_000)?;
    let mime = match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("svg") => "image/svg+xml",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        _ => "image/png",
    };
    Some(format!("data:{mime};base64,{}", b64(&bytes)))
}

pub fn items() -> Vec<Item> {
    let mut out = vec![];
    if let Ok(rd) = fs::read_dir(extensions_dir()) {
        for e in rd.flatten() {
            if e.path().join(REMOVED_MARK).exists() {
                continue;
            }
            let Some(m) = read_manifest(&e.path()) else { continue };
            let dir = e.file_name().to_string_lossy().into_owned();
            let act = if m["action"].is_object() { &m["action"] } else { &m["browser_action"] };
            out.push(Item {
                name: localized(&e.path(), &m, m["name"].as_str().unwrap_or(&dir)),
                icon: icon_uri(&e.path(), &m),
                popup: act["default_popup"].as_str().map(|s| s.trim_start_matches('/').to_string()),
                options: m["options_page"].as_str().or_else(|| m["options_ui"]["page"].as_str()).map(|s| s.trim_start_matches('/').to_string()),
                id: ext_id(&e.path()),
                error: (m["manifest_version"].as_u64() != Some(3) && !e.path().join(DISABLED_MARK).exists()).then(|| "old Manifest V2 format: reinstall to convert".to_string()),
                firefox: m.get("browser_specific_settings").is_some() || m.get("applications").is_some(),
                version: m["version"].as_str().unwrap_or("").to_string(),
                disabled: e.path().join(DISABLED_MARK).exists(),
                dir,
            });
        }
    }
    out
}

#[cfg(test)]
mod convert_tests {
    use super::*;

    #[test]
    fn mv2_becomes_mv3() {
        let d = std::env::temp_dir().join(format!("imc-ext-{}", std::process::id()));
        let _ = fs::create_dir_all(&d);
        let mut m = json!({
            "manifest_version": 2, "name": "T", "version": "1",
            "browser_action": {"default_popup": "p.html"},
            "permissions": ["storage", "https://a.example/*", "<all_urls>", "webRequestBlocking"],
            "web_accessible_resources": ["img/x.png"],
            "content_security_policy": "script-src 'self' 'unsafe-eval'",
            "background": {"scripts": ["a.js", "/lib/b.js"], "persistent": false}
        });
        let w = convert_to_mv3(&d, &mut m);
        assert_eq!(m["manifest_version"], 3);
        assert!(m.get("browser_action").is_none() && m["action"]["default_popup"] == "p.html");
        assert_eq!(m["permissions"], json!(["storage"]));
        assert_eq!(m["host_permissions"], json!(["https://a.example/*", "<all_urls>"]));
        assert_eq!(m["web_accessible_resources"][0]["resources"][0], "img/x.png");
        assert_eq!(m["content_security_policy"]["extension_pages"], "script-src 'self'; object-src 'self'");
        assert_eq!(m["background"]["service_worker"], "imc-bg.js");
        let bg = fs::read_to_string(d.join("imc-bg.js")).unwrap();
        assert!(bg.contains("importScripts(\"a.js\", \"lib/b.js\")"));
        assert!(w.len() >= 3);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn ids_use_only_a_to_p_and_differ_per_path() {
        let a = id_for_path(Path::new(r"C:\x\one"));
        let b = id_for_path(Path::new(r"C:\x\two"));
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| (b'a'..=b'p').contains(&c)));
        assert_ne!(a, b);
    }
}

#[cfg(test)]
mod store_tests {
    use super::store_link;

    #[test]
    fn store_pages_become_installable_addresses() {
        assert_eq!(store_link("https://chromewebstore.google.com/detail/ublock-origin/cjpalhdlnbpafiamejdnhcphjbkeiagm?hl=en").as_deref(), Some("cjpalhdlnbpafiamejdnhcphjbkeiagm"));
        assert_eq!(store_link("https://addons.mozilla.org/en-US/firefox/addon/ublock-origin/").as_deref(), Some("https://addons.mozilla.org/firefox/downloads/latest/ublock-origin/latest.xpi"));
        assert_eq!(store_link("https://example.com/addon/x"), None);
    }
}
