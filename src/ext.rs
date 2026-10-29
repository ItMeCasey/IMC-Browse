//! Extension installer + Firefox compatibility shim.
//!
//! Runtime: WebView2's native Chromium extension host (enabled in main.rs), so
//! `chrome.*` is real, not emulated. This module gets extensions *onto disk* in
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

pub fn install(src: &str) -> Result<String, String> {
    let src = src.trim();
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
    fs::remove_dir_all(dir).map_err(|e| e.to_string())
}

fn download(url: &str) -> Result<Vec<u8>, String> {
    // curl.exe ships with Windows 10 1803+; avoids linking a TLS stack just for installs.
    let tmp = std::env::temp_dir().join(format!("imc-dl-{}.bin", std::process::id()));
    let st = Command::new("curl.exe")
        .args(["-fsSL", "--max-filesize", "104857600", "-o"])
        .arg(&tmp)
        .arg(url)
        .status()
        .map_err(|e| format!("curl.exe failed to start: {e}"))?;
    if !st.success() {
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
            f.read_to_end(&mut data).map_err(|e| e.to_string())?;
            fs::write(&out, data).map_err(|e| e.to_string())?;
        }
    }
    finish(&dest)
}

pub fn read_manifest(dir: &Path) -> Option<Value> {
    let s = fs::read_to_string(dir.join("manifest.json")).ok()?;
    serde_json::from_str(s.trim_start_matches('\u{feff}')).ok()
}

fn finish(dir: &Path) -> Result<String, String> {
    let mut m = read_manifest(dir).ok_or("manifest.json missing or invalid")?;
    let firefox = m.get("browser_specific_settings").is_some() || m.get("applications").is_some();
    let mut note = String::new();
    if firefox {
        fs::write(dir.join("imc-compat.js"), COMPAT_JS).map_err(|e| e.to_string())?;
        let mut patched = false;
        if let Some(arr) = m.pointer_mut("/background/scripts").and_then(Value::as_array_mut) {
            arr.insert(0, json!("imc-compat.js"));
            patched = true;
        }
        if let Some(cs) = m.get_mut("content_scripts").and_then(Value::as_array_mut) {
            for c in cs {
                if let Some(js) = c.get_mut("js").and_then(Value::as_array_mut) {
                    js.insert(0, json!("imc-compat.js"));
                    patched = true;
                }
            }
        }
        if m.pointer("/background/service_worker").is_some() {
            note.push_str(" (MV3 service worker: browser.* alias not injected there)");
        }
        if patched {
            fs::write(dir.join("manifest.json"), serde_json::to_string_pretty(&m).unwrap()).map_err(|e| e.to_string())?;
        }
    }
    Ok(format!(
        "installed {} {} -> {}{note}\nRestart the browser to load it.",
        m["name"].as_str().unwrap_or("?"),
        m["version"].as_str().unwrap_or(""),
        dir.display()
    ))
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
            let Some(m) = read_manifest(&e.path()) else { continue };
            let dir = e.file_name().to_string_lossy().into_owned();
            let act = if m["action"].is_object() { &m["action"] } else { &m["browser_action"] };
            out.push(Item {
                name: localized(&e.path(), &m, m["name"].as_str().unwrap_or(&dir)),
                icon: icon_uri(&e.path(), &m),
                popup: act["default_popup"].as_str().map(|s| s.trim_start_matches('/').to_string()),
                options: m["options_page"].as_str().or_else(|| m["options_ui"]["page"].as_str()).map(|s| s.trim_start_matches('/').to_string()),
                dir,
            });
        }
    }
    out
}
