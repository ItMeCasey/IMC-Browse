//! Settings and on-disk layout (`%LOCALAPPDATA%\IMC-Browse`).

use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs, path::PathBuf};

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    /// Seconds a background tab may sit idle before it is suspended.
    pub freeze_after_secs: u64,
    pub adblock: bool,
    pub https_first: bool,
    pub home: String,
    /// Address-bar search provider: google, bing, duckduckgo, brave or yahoo.
    pub search_engine: String,
    pub vertical_tabs: bool,
    /// Draw and decode with the GPU. Off adds --disable-gpu (needs a restart).
    pub hardware_acceleration: bool,
    /// Items on either side of the address bar, by id (see the TB_DEFS table in chrome.html).
    pub toolbar: Toolbar,
    pub gestures: Gestures,
    /// Remembered zoom factor per site (host), like Chrome. Ghost tabs never write here.
    pub zoom: HashMap<String, f64>,
    pub update: UpdateCfg,
    pub doh: Doh,
    pub theme: Theme,
    pub vpn: Vpn,
    /// origin -> permission names that stay granted ("camera", "geolocation", ...).
    /// Everything not listed here is denied.
    pub permission_allow: HashMap<String, Vec<String>>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Toolbar {
    pub left: Vec<String>,
    pub right: Vec<String>,
}

impl Default for Toolbar {
    fn default() -> Self {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect();
        Self { left: v(&["back", "forward", "reload"]), right: v(&["engine", "bookmark", "shield", "vpn", "downloads"]) }
    }
}

/// Secure DNS (DNS over HTTPS): look names up through an encrypted HTTPS server instead of the
/// network's plain DNS, so the network cannot read or tamper with which sites you visit.
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Doh {
    pub enabled: bool,
    /// "cloudflare", "google", "quad9" or "custom"
    pub provider: String,
    /// https:// template used when provider is "custom"
    pub custom: String,
    /// Never fall back to the network's normal DNS (sites fail to load if the DoH server is unreachable).
    pub strict: bool,
}

impl Default for Doh {
    fn default() -> Self {
        Self { enabled: false, provider: "cloudflare".into(), custom: String::new(), strict: false }
    }
}

pub const DOH_PROVIDERS: &[(&str, &str, &str)] = &[
    ("cloudflare", "Cloudflare (1.1.1.1)", "https://cloudflare-dns.com/dns-query"),
    ("google", "Google Public DNS", "https://dns.google/dns-query"),
    ("quad9", "Quad9 (blocks known malicious sites)", "https://dns.quad9.net/dns-query"),
];

impl Doh {
    /// The DoH server URL, or None if the setting is off or the custom address is not a plain https URL.
    pub fn template(&self) -> Option<String> {
        if !self.enabled {
            return None;
        }
        if self.provider == "custom" {
            let c = self.custom.trim();
            let ok = c.starts_with("https://") && c.len() < 300 && c.len() > 12 && !c.chars().any(|ch| ch.is_whitespace() || ch.is_control() || ch == '"' || ch == ',');
            return ok.then(|| c.to_string());
        }
        DOH_PROVIDERS.iter().find(|(id, _, _)| *id == self.provider).map(|(_, _, url)| url.to_string())
    }

    /// Chromium's (`dns_over_https.mode`, `dns_over_https.templates`) for this setting.
    /// Off means off: the engine's own "upgrade to my DNS provider's DoH" is switched off too.
    pub fn prefs(&self) -> (&'static str, String) {
        match self.template() {
            Some(t) => (if self.strict { "secure" } else { "automatic" }, t),
            None => ("off", String::new()),
        }
    }

    pub fn describe(&self) -> String {
        match self.template() {
            None if self.enabled => "On, but the custom server address is not valid, so it is not applied".into(),
            None => "Off (using the network's DNS)".into(),
            Some(t) => format!("On: {} ({})", t, if self.strict { "strict, no fallback" } else { "falls back to normal DNS if the server is unreachable" }),
        }
    }
}

/// The engines (WebView2 and CEF) read Secure DNS from the profile's `Local State` file, and there is
/// no command-line switch for it. This sets it before the engine starts; it must not run while the
/// engine is running (the engine rewrites the file on exit). Other settings in the file are kept.
pub fn write_secure_dns(user_data_dir: &std::path::Path, doh: &Doh) {
    let path = user_data_dir.join("Local State");
    let mut v: serde_json::Value = fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).filter(|v: &serde_json::Value| v.is_object()).unwrap_or_else(|| serde_json::json!({}));
    let (mode, templates) = doh.prefs();
    v["dns_over_https"] = serde_json::json!({"mode": mode, "templates": templates});
    let _ = fs::create_dir_all(user_data_dir);
    if let Ok(s) = serde_json::to_string(&v) {
        let _ = fs::write(&path, s);
    }
}

/// Update preferences. Nothing is contacted in the background unless one of the first two is on.
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct UpdateCfg {
    /// Install updates automatically (from the nightly channel if subscribed, else the stable one).
    pub auto: bool,
    /// Follow the nightly channel instead of stable releases.
    pub nightly: bool,
    /// Unix seconds of the last check.
    pub last_check: u64,
}

/// Every action a gesture can trigger: (id, label shown in the UI and on the gesture HUD).
pub const GESTURE_ACTIONS: &[(&str, &str)] = &[
    ("back", "Back"),
    ("forward", "Forward"),
    ("reload", "Reload"),
    ("stop", "Stop loading"),
    ("new_tab", "New tab"),
    ("new_ghost", "New Ghost tab"),
    ("close_tab", "Close tab"),
    ("reopen", "Reopen closed tab"),
    ("next_tab", "Next tab"),
    ("prev_tab", "Previous tab"),
    ("home", "Home"),
    ("bookmark", "Bookmark page"),
    ("mute_tab", "Mute / unmute tab"),
    ("fullscreen", "Full screen"),
    ("print", "Print"),
    ("focus_url", "Focus address bar"),
    ("scroll_top", "Scroll to top"),
    ("scroll_bottom", "Scroll to bottom"),
];

#[derive(Serialize, Deserialize, Clone)]
pub struct GestureBind {
    /// Directions drawn with the right button held, e.g. "DR" (down then right).
    pub path: String,
    pub action: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Gestures {
    pub enabled: bool,
    /// Pixels the pointer must travel before a direction counts.
    pub min_distance: u32,
    /// Draw the path while gesturing.
    pub trail: bool,
    pub binds: Vec<GestureBind>,
}

impl Default for Gestures {
    fn default() -> Self {
        let b = |p: &str, a: &str| GestureBind { path: p.into(), action: a.into() };
        Self {
            enabled: true,
            min_distance: 28,
            trail: true,
            binds: vec![b("L", "back"), b("R", "forward"), b("D", "new_tab"), b("DR", "close_tab"), b("UD", "reload"), b("DL", "reopen"), b("UL", "prev_tab"), b("UR", "next_tab"), b("U", "scroll_top"), b("LR", "stop")],
        }
    }
}

impl Gestures {
    /// Only U/D/L/R, at most 8 long; anything else is dropped.
    pub fn clean_path(p: &str) -> Option<String> {
        (!p.is_empty() && p.len() <= 8 && p.chars().all(|c| "UDLR".contains(c))).then(|| p.to_string())
    }

    pub fn action_for(&self, path: &str) -> Option<&str> {
        if !self.enabled {
            return None;
        }
        self.binds.iter().find(|b| b.path == path).map(|b| b.action.as_str())
    }

    /// What the injected page script needs (see ui/gesture.js).
    pub fn page_config(&self) -> String {
        let map: serde_json::Map<String, serde_json::Value> = self
            .binds
            .iter()
            .filter_map(|b| GESTURE_ACTIONS.iter().find(|(id, _)| *id == b.action).map(|(_, l)| (b.path.clone(), serde_json::Value::String((*l).into()))))
            .collect();
        serde_json::json!({"enabled": self.enabled, "min": self.min_distance.clamp(10, 120), "trail": self.trail, "map": map}).to_string()
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Vpn {
    pub enabled: bool,
    /// "socks5" or "http"
    pub kind: String,
    pub host: String,
    pub port: u16,
    /// Block every request while the proxy is unreachable.
    pub kill_switch: bool,
    /// Hosts that skip the proxy (split tunnelling), e.g. "*.corp.example", "localhost".
    pub bypass: Vec<String>,
}

/// `name` is a preset ("dark", "light", "midnight", "forest", "rose", "contrast", "system")
/// or "custom", in which case the four colours below are used.
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Theme {
    pub name: String,
    pub bg: String,
    pub bar: String,
    pub fg: String,
    pub acc: String,
    /// Switch between `light_name` and `dark_name` with the Windows light/dark setting.
    pub follow_system: bool,
    pub light_name: String,
    pub dark_name: String,
}

impl Default for Theme {
    fn default() -> Self {
        Self { name: "dark".into(), bg: "#14161a".into(), bar: "#1c1f25".into(), fg: "#e6e8ec".into(), acc: "#4da3ff".into(), follow_system: false, light_name: "light".into(), dark_name: "dark".into() }
    }
}

impl Default for Vpn {
    fn default() -> Self {
        Self { enabled: false, kind: "socks5".into(), host: "127.0.0.1".into(), port: 1080, kill_switch: true, bypass: vec![] }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            freeze_after_secs: 3600,
            adblock: true,
            https_first: true,
            home: "https://duckduckgo.com/".into(),
            search_engine: "duckduckgo".into(),
            vertical_tabs: false,
            hardware_acceleration: true,
            toolbar: Toolbar::default(),
            gestures: Gestures::default(),
            zoom: HashMap::new(),
            update: UpdateCfg::default(),
            doh: Doh::default(),
            theme: Theme::default(),
            vpn: Vpn::default(),
            permission_allow: HashMap::new(),
        }
    }
}

pub fn root() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let p = base.join("IMC-Browse");
    let _ = fs::create_dir_all(&p);
    p
}
pub fn profile_dir() -> PathBuf {
    root().join("profile")
}
/// Parent of the throwaway profiles behind Ghost (private) tabs; wiped on close and at launch.
pub fn private_root() -> PathBuf {
    root().join("private")
}
pub fn lists_dir() -> PathBuf {
    let p = root().join("lists");
    let _ = fs::create_dir_all(&p);
    p
}
pub fn extensions_dir() -> PathBuf {
    let p = root().join("extensions");
    let _ = fs::create_dir_all(&p);
    p
}

impl Config {
    pub fn load() -> Self {
        let mut c: Config = fs::read_to_string(root().join("config.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if c.freeze_after_secs == 120 {
            c.freeze_after_secs = 3600; // the old default was two minutes
        }
        c
    }
    pub fn save(&self) {
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = fs::write(root().join("config.json"), s);
        }
    }
}

#[cfg(test)]
mod gesture_tests {
    use super::*;

    #[test]
    fn paths_are_validated_and_mapped() {
        assert_eq!(Gestures::clean_path("DR").as_deref(), Some("DR"));
        assert!(Gestures::clean_path("").is_none() && Gestures::clean_path("DX").is_none() && Gestures::clean_path("UUUUUUUUU").is_none());
        let g = Gestures::default();
        assert_eq!(g.action_for("DR"), Some("close_tab"));
        assert_eq!(g.action_for("RRR"), None);
        let off = Gestures { enabled: false, ..Gestures::default() };
        assert_eq!(off.action_for("DR"), None);
    }

    #[test]
    fn every_default_action_exists_and_page_config_has_labels() {
        let g = Gestures::default();
        for b in &g.binds {
            assert!(GESTURE_ACTIONS.iter().any(|(id, _)| *id == b.action), "unknown action {}", b.action);
        }
        let cfg: serde_json::Value = serde_json::from_str(&g.page_config()).unwrap();
        assert_eq!(cfg["map"]["DR"], "Close tab");
        assert_eq!(cfg["min"], 28);
    }
}

/// Zoom levels Ctrl+wheel / Ctrl+plus / Ctrl+minus step through (25% to 500%).
pub const ZOOM_STEPS: &[f64] = &[0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0];

/// Next zoom factor for "in", "out" or "reset".
pub fn zoom_step(cur: f64, dir: &str) -> f64 {
    match dir {
        "reset" => 1.0,
        "in" => ZOOM_STEPS.iter().copied().find(|z| *z > cur + 0.001).unwrap_or(cur),
        "out" => ZOOM_STEPS.iter().rev().copied().find(|z| *z < cur - 0.001).unwrap_or(cur),
        _ => cur,
    }
}

#[cfg(test)]
mod zoom_tests {
    use super::*;

    #[test]
    fn steps_clamp_and_reset() {
        assert_eq!(zoom_step(1.0, "in"), 1.1);
        assert_eq!(zoom_step(1.0, "out"), 0.9);
        assert_eq!(zoom_step(5.0, "in"), 5.0);
        assert_eq!(zoom_step(0.25, "out"), 0.25);
        assert_eq!(zoom_step(2.5, "reset"), 1.0);
        // off-grid values (e.g. from an old config) snap to the next step
        assert_eq!(zoom_step(1.05, "in"), 1.1);
        assert_eq!(zoom_step(1.05, "out"), 1.0);
    }
}

#[cfg(test)]
mod doh_tests {
    use super::*;

    #[test]
    fn prefs_follow_the_setting() {
        assert_eq!(Doh::default().prefs(), ("off", String::new()));
        let d = Doh { enabled: true, ..Doh::default() };
        assert_eq!(d.prefs(), ("automatic", "https://cloudflare-dns.com/dns-query".to_string()));
        let strict = Doh { enabled: true, strict: true, provider: "google".into(), ..Doh::default() };
        assert_eq!(strict.prefs(), ("secure", "https://dns.google/dns-query".to_string()));
        // an invalid custom address must not turn anything on
        let bad = Doh { enabled: true, provider: "custom".into(), custom: "http://nope".into(), strict: true };
        assert_eq!(bad.prefs().0, "off");
    }

    #[test]
    fn custom_server_must_be_a_plain_https_url() {
        let c = |u: &str| Doh { enabled: true, provider: "custom".into(), custom: u.into(), strict: false };
        assert!(c("https://dns.example.net/dns-query").template().is_some());
        assert!(c("http://dns.example.net/dns-query").template().is_none());
        assert!(c("https://x y/dns").template().is_none());
        assert!(c("https://a.example/q,--evil-flag").template().is_none());
        assert!(c("").template().is_none());
    }

    #[test]
    fn local_state_keeps_other_settings() {
        let d = std::env::temp_dir().join(format!("imc-ls-{}", std::process::id()));
        let _ = fs::create_dir_all(&d);
        fs::write(d.join("Local State"), r#"{"keep":{"me":1},"dns_over_https":{"mode":"off"}}"#).unwrap();
        write_secure_dns(&d, &Doh { enabled: true, strict: true, ..Doh::default() });
        let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(d.join("Local State")).unwrap()).unwrap();
        assert_eq!(v["keep"]["me"], 1);
        assert_eq!(v["dns_over_https"]["mode"], "secure");
        assert_eq!(v["dns_over_https"]["templates"], "https://cloudflare-dns.com/dns-query");
        let _ = fs::remove_dir_all(&d);
    }
}
