#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod adblock;
mod bookmarks;
mod cards;
mod passwords;
mod config;
mod downloads;
mod ext;
mod internal;
mod updater;
mod sandbox;
mod vpn;
mod winhook;

use config::Config;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    process::Command,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tao::{
    dpi::{LogicalPosition, LogicalSize},
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    window::{Fullscreen, Window, WindowBuilder},
};
use winhook::{DlEvent, Shield};
use wry::{
    NewWindowResponse, PageLoadEvent, PermissionKind, PermissionResponse, Rect, WebContext, WebView, WebViewBuilder,
    WebViewBuilderExtWindows, WebViewExtWindows,
};

const TOOLBAR_H: f64 = 52.0;
const TABSTRIP_H: f64 = 36.0;
const SIDEBAR_W: f64 = 230.0;
/// Own title row, only needed with vertical tabs (otherwise the tab strip is the titlebar).
const TITLEBAR_H: f64 = 32.0;
/// Which download this build updates itself from (see the release manifest).
const FLAVOR: &str = "webview2";
const CHROME_HTML: &str = include_str!("ui/chrome.html");
/// Injected into every page: right-button drag recognition (see ui/gesture.js).
const GESTURE_JS: &str = include_str!("ui/gesture.js");

/// Everything the OS/Chromium would otherwise phone home with, plus GPU raster and strict site isolation.
const BASE_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,OptimizationHints,MediaRouter,Translate,AutofillServerCommunication \
--disable-background-networking --disable-sync --disable-component-update --disable-domain-reliability \
--disable-client-side-phishing-detection --disable-breakpad --no-pings --no-first-run";

/// GPU flags follow the Hardware acceleration setting.
fn gpu_args(on: bool) -> &'static str {
    if on { " --enable-gpu-rasterization --enable-zero-copy" } else { " --disable-gpu --disable-gpu-compositing" }
}

enum Ev {
    Ipc(String),
    Url(usize, String),
    Title(usize, String),
    Load(usize, bool),
    Audio(usize, bool),
    Go(usize, String),
    Open(String),
    /// A drawn gesture path (U/D/L/R) reported by a page.
    Gesture(String),
    Update(updater::UpdMsg),
    Download(DlEvent),
    /// Zoom request from a page: "in", "out" or "reset".
    Zoom(String),
    /// Certificate/connection JSON for a tab (DevTools Security domain).
    Security(usize, String),
    Toast(String),
    Tick,
    /// (id, name) pairs WebView2 reports for installed extensions.
    ExtIds(Vec<(String, String)>),
}

struct Tab {
    id: usize,
    wv: WebView,
    url: String,
    title: String,
    loading: bool,
    frozen: bool,
    audible: bool,
    muted: bool,
    /// Ghost tab: throwaway profile, no extensions, never saved or restored.
    private: bool,
    /// Current zoom factor (1.0 = 100%).
    zoom: f64,
    last_active: Instant,
}

struct App {
    window: Window,
    chrome: WebView,
    ctx: WebContext,
    /// Created with the first Ghost tab, dropped and deleted with the last one.
    private_ctx: Option<WebContext>,
    tabs: Vec<Tab>,
    active: usize,
    next: usize,
    cfg: Config,
    proxy: EventLoopProxy<Ev>,
    shield: Arc<Shield>,
    insecure_ok: Arc<Mutex<HashSet<String>>>,
    args: String,
    panel: bool,
    bookmarks: Vec<bookmarks::Bookmark>,
    /// URLs of recently closed tabs (not Ghost), newest last.
    closed: Vec<String>,
    ext_items: Vec<ext::Item>,
    ext_ids: Vec<(String, String)>,
    sec: std::collections::HashMap<usize, String>,
    /// Live data behind the imc:// pages.
    internal: Arc<RwLock<internal::Internal>>,
    downloads: Vec<downloads::Item>,
    dl_ids: Arc<std::sync::atomic::AtomicU64>,
    last_dl_push: Instant,
    upd: updater::Ctl,
}

/// Page area. The chrome webview spans the whole window underneath; pages are child
/// windows on top, so the chrome only shows where they don't cover it (toolbar, tab strip/sidebar).
fn content_bounds(w: &Window, hidden: bool, vertical: bool) -> Rect {
    let s: LogicalSize<f64> = w.inner_size().to_logical(w.scale_factor());
    let (left, top) = if vertical { (SIDEBAR_W, TITLEBAR_H + TOOLBAR_H) } else { (0.0, TOOLBAR_H + TABSTRIP_H) };
    let (cw, ch) = if hidden { (0.0, 0.0) } else { ((s.width - left).max(0.0), (s.height - top).max(0.0)) };
    Rect { position: LogicalPosition::new(left, top).into(), size: LogicalSize::new(cw, ch).into() }
}

fn host_of(u: &str) -> &str {
    u.split("://").nth(1).and_then(|r| r.split(['/', '?', '#', ':']).next()).unwrap_or("")
}

fn is_local(h: &str) -> bool {
    h == "localhost" || h.ends_with(".localhost") || h == "127.0.0.1" || h == "[::1]"
}

fn search_base(engine: &str) -> &'static str {
    match engine {
        "google" => "https://www.google.com/search?q=",
        "bing" => "https://www.bing.com/search?q=",
        "brave" => "https://search.brave.com/search?q=",
        "yahoo" => "https://search.yahoo.com/search?p=",
        _ => "https://duckduckgo.com/?q=",
    }
}

/// Omnibox text -> (url, explicit_http)
fn normalize(t: &str, engine: &str) -> (String, bool) {
    let t = t.trim();
    if let Some(u) = internal::from_typed(t) {
        return (u, false);
    }
    let lower = t.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("file://") {
        return (t.into(), false);
    }
    if lower.starts_with("http://") {
        return (t.into(), true);
    }
    if !t.contains(' ') && !t.contains("://") && (t.contains('.') || lower.starts_with("localhost")) && !t.starts_with('.') {
        return (format!("https://{t}"), false);
    }
    let q: String = t
        .bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect();
    (format!("{}{q}", search_base(engine)), false)
}

fn load_engine() -> adblock::Engine {
    let mut e = adblock::Engine::new();
    e.load_list(include_str!("../data/builtin.txt"));
    if let Ok(rd) = std::fs::read_dir(config::lists_dir()) {
        for f in rd.flatten() {
            if f.path().extension().is_some_and(|x| x == "txt") {
                if let Ok(s) = std::fs::read_to_string(f.path()) {
                    e.load_list(&s);
                }
            }
        }
    }
    e
}

impl App {
    fn private_dir() -> std::path::PathBuf {
        config::private_root().join(std::process::id().to_string())
    }

    /// Deletes the Ghost profile once no Ghost tab is left (retries while Chromium releases its files).
    fn end_private_if_unused(&mut self, wait: bool) {
        if self.tabs.iter().any(|t| t.private) || self.private_ctx.is_none() {
            return;
        }
        self.private_ctx = None;
        let dir = Self::private_dir();
        let wipe = move || {
            for _ in 0..40 {
                if std::fs::remove_dir_all(&dir).is_ok() || !dir.exists() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        };
        if wait {
            wipe();
        } else {
            std::thread::spawn(wipe);
        }
    }

    fn new_tab(&mut self, url: &str, private: bool) -> usize {
        let id = self.next;
        self.next += 1;
        let (p_nav, p_title, p_load, p_win) = (self.proxy.clone(), self.proxy.clone(), self.proxy.clone(), self.proxy.clone());
        let cur = Arc::new(Mutex::new(String::new()));
        let (cur_nav, cur_perm) = (cur.clone(), cur.clone());
        let https_first = self.cfg.https_first;
        let insecure = self.insecure_ok.clone();
        let allow = self.cfg.permission_allow.clone();
        let css = self.shield.engine.read().unwrap().cosmetic_css();
        let css_js = format!(
            "(()=>{{const c={};if(!c)return;const a=()=>{{const s=document.createElement('style');s.textContent=c;(document.head||document.documentElement).appendChild(s)}};\
             document.readyState==='loading'?document.addEventListener('DOMContentLoaded',a):a()}})();",
            Value::String(css)
        );
        let bounds = content_bounds(&self.window, self.panel, self.cfg.vertical_tabs);
        let p_audio = self.proxy.clone();
        let p_gest = self.proxy.clone();
        let p_sec = self.proxy.clone();
        let p_zoom = self.proxy.clone();
        let p_settings = self.proxy.clone();
        let (internal_data, internal_shield) = (self.internal.clone(), self.shield.clone());
        let init_js = format!("{css_js}\n{GESTURE_JS}\nwindow.__imcGset({});", self.cfg.gestures.page_config());

        let ctx = if private {
            self.private_ctx.get_or_insert_with(|| WebContext::new(Some(Self::private_dir())))
        } else {
            &mut self.ctx
        };
        let wv = WebViewBuilder::new_with_web_context(ctx)
            .with_bounds(bounds)
            .with_url(url)
            .with_initialization_script(init_js)
            // Pages can only ever send a clean gesture path through here; the host maps it to the user's action.
            .with_ipc_handler(move |r| {
                if let Some(path) = r.body().strip_prefix("IMCG:").and_then(config::Gestures::clean_path) {
                    let _ = p_gest.send_event(Ev::Gesture(path));
                } else if let Some(d) = r.body().strip_prefix("IMCZ:").filter(|d| ["in", "out", "reset"].contains(d)) {
                    let _ = p_zoom.send_event(Ev::Zoom(d.to_string()));
                }
            })
            .with_additional_browser_args(self.args.clone())
            .with_browser_extensions_enabled(!private)
            .with_extensions_path(config::extensions_dir())
            .with_custom_protocol("imc".into(), move |_id, req| {
                let name = req.uri().path().trim_matches('/').to_string();
                let (status, html) = match internal::page(&name, &internal_data.read().unwrap(), &internal_shield) {
                    Some(h) => (200, h),
                    None => (404, internal::not_found(&name)),
                };
                wry::http::Response::builder()
                    .status(status)
                    .header("Content-Type", "text/html; charset=utf-8")
                    .header("Content-Security-Policy", "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; img-src data:; frame-ancestors 'none'")
                    .header("X-Frame-Options", "DENY")
                    .header("Cache-Control", "no-store")
                    .body(std::borrow::Cow::Owned(html.into_bytes()))
                    .unwrap()
            })
            .with_navigation_handler(move |u| {
                // imc://settings opens the settings panel instead of loading a page
                if let Some(section) = internal::settings_section(&u) {
                    let _ = p_settings.send_event(Ev::Ipc(json!({"cmd": "open_settings", "section": section}).to_string()));
                    return false;
                }
                if https_first && u.starts_with("http://") {
                    let h = host_of(&u);
                    if !is_local(h) && !insecure.lock().unwrap().contains(h) {
                        let _ = p_nav.send_event(Ev::Go(id, u.replacen("http://", "https://", 1)));
                        return false;
                    }
                }
                *cur_nav.lock().unwrap() = u.clone();
                let _ = p_nav.send_event(Ev::Url(id, u));
                true
            })
            .with_document_title_changed_handler(move |t| {
                let _ = p_title.send_event(Ev::Title(id, t));
            })
            .with_on_page_load_handler(move |e, _| {
                let _ = p_load.send_event(Ev::Load(id, matches!(e, PageLoadEvent::Started)));
            })
            .with_new_window_req_handler(move |u, _| {
                let _ = p_win.send_event(Ev::Open(u));
                NewWindowResponse::Deny
            })
            // Zero ambient authority: deny unless this exact origin was whitelisted in config.json.
            .with_permission_handler(move |kind: PermissionKind| {
                let url = cur_perm.lock().unwrap().clone();
                let origin = url.split('/').take(3).collect::<Vec<_>>().join("/");
                match allow.get(&origin) {
                    Some(list) if list.iter().any(|k| *k == kind.to_string()) => PermissionResponse::Allow,
                    _ => PermissionResponse::Deny,
                }
            })
            .build_as_child(&self.window);

        let wv = match wv {
            Ok(w) => w,
            Err(e) => {
                eprintln!("tab creation failed: {e}");
                std::process::exit(1);
            }
        };
        if let Err(e) = winhook::install_shield(&wv, self.shield.clone()) {
            eprintln!("request shield failed to install: {e}");
        }
        if let Err(e) = winhook::watch_audio(&wv, move |on| {
            let _ = p_audio.send_event(Ev::Audio(id, on));
        }) {
            eprintln!("audio events unavailable: {e}");
        }
        if let Err(e) = winhook::watch_security(&wv, move |j| {
            let _ = p_sec.send_event(Ev::Security(id, j));
        }) {
            eprintln!("certificate info unavailable: {e}");
        }
        winhook::disable_native_zoom(&wv);
        winhook::enable_status_bar(&wv);
        {
            let p = self.proxy.clone();
            if let Err(e) = winhook::watch_downloads(&wv, self.dl_ids.clone(), downloads::dir(), private, move |ev| {
                let _ = p.send_event(Ev::Download(ev));
            }) {
                eprintln!("download tracking unavailable: {e}");
            }
        }
        winhook::background(&wv);
        self.tabs.push(Tab {
            id,
            wv,
            url: url.into(),
            title: String::new(),
            loading: true,
            frozen: false,
            audible: false,
            muted: false,
            private,
            zoom: 1.0,
            last_active: Instant::now(),
        });
        id
    }

    fn activate(&mut self, id: usize) {
        if !self.tabs.iter().any(|t| t.id == id) {
            return;
        }
        let prev = self.active;
        let hide_all = self.panel;
        for t in &mut self.tabs {
            if t.id == id {
                if t.frozen {
                    winhook::resume(&t.wv);
                    t.frozen = false;
                }
                t.last_active = Instant::now();
                let _ = t.wv.set_bounds(content_bounds(&self.window, hide_all, self.cfg.vertical_tabs));
                if !hide_all {
                    let _ = t.wv.set_visible(true);
                    let _ = t.wv.set_memory_usage_level(wry::MemoryUsageLevel::Normal);
                    let _ = t.wv.focus();
                }
            } else if t.id == prev || !t.frozen {
                if t.id == prev {
                    t.last_active = Instant::now();
                }
                winhook::background(&t.wv);
            }
        }
        self.active = id;
        self.push();
    }

    fn layout(&self) {
        let s: LogicalSize<f64> = self.window.inner_size().to_logical(self.window.scale_factor());
        let _ = self.chrome.set_bounds(Rect {
            position: LogicalPosition::new(0.0, 0.0).into(),
            size: LogicalSize::new(s.width, s.height).into(),
        });
        for t in &self.tabs {
            let _ = t.wv.set_bounds(content_bounds(&self.window, self.panel, self.cfg.vertical_tabs));
            if t.id == self.active && !t.frozen {
                let _ = t.wv.set_visible(!self.panel);
            }
        }
    }

    fn close(&mut self, id: usize) {
        if let Some(t) = self.tabs.iter().find(|t| t.id == id && !t.private && t.url.starts_with("http")) {
            self.closed.push(t.url.clone());
            if self.closed.len() > 20 {
                self.closed.remove(0);
            }
        }
        self.tabs.retain(|t| t.id != id);
        self.end_private_if_unused(false);
        if self.tabs.is_empty() {
            let home = self.cfg.home.clone();
            let n = self.new_tab(&home, false);
            self.activate(n);
        } else if self.active == id {
            let n = self.tabs.last().unwrap().id;
            self.activate(n);
        } else {
            self.push();
        }
    }

    fn navigate(&mut self, text: &str) {
        let (url, explicit_http) = normalize(text, &self.cfg.search_engine);
        if explicit_http {
            self.insecure_ok.lock().unwrap().insert(host_of(&url).to_string());
        }
        self.go(self.active, &url);
    }

    fn go(&mut self, id: usize, url: &str) {
        if let Some(t) = self.tabs.iter().find(|t| t.id == id) {
            let _ = t.wv.load_url(url);
        }
    }

    fn push(&self) {
        // Taskbar / Alt-Tab title follows the open tab (Ghost tabs stay generic so their pages are not shown there).
        let title = match self.tabs.iter().find(|t| t.id == self.active) {
            Some(t) if t.private => "Ghost tab - IMC Browse".to_string(),
            Some(t) if !t.title.is_empty() => format!("{} - IMC Browse", t.title),
            _ => "IMC Browse".to_string(),
        };
        if self.window.title() != title {
            self.window.set_title(&title);
        }
        let down = self.cfg.vpn.enabled && vpn::TUNNEL_DOWN.load(Ordering::Relaxed);
        let state = json!({
            "tabs": self.tabs.iter().map(|t| json!({"id": t.id, "title": t.title, "url": t.url, "frozen": t.frozen, "loading": t.loading, "audible": t.audible, "muted": t.muted, "private": t.private, "zoom": (t.zoom * 100.0).round()})).collect::<Vec<_>>(),
            "active": self.active,
            "bookmarks": self.bookmarks,
            "blocked": self.shield.blocked.load(Ordering::Relaxed),
            "adblock": self.shield.enabled.load(Ordering::Relaxed),
            "vpn": {"enabled": self.cfg.vpn.enabled, "down": down},
            "cfg": self.cfg,
            "extensions": self.ext_items.iter().map(|e| json!({"dir": e.dir, "name": e.name, "icon": e.icon, "popup": e.popup.is_some() || e.options.is_some()})).collect::<Vec<_>>(),
            "maximized": self.window.is_maximized(),
            "gesture_actions": config::GESTURE_ACTIONS.iter().map(|(id, l)| json!({"id": id, "label": l})).collect::<Vec<_>>(),
        });
        let _ = self.chrome.evaluate_script(&format!("window.ui&&window.ui.set({state})"));
    }

    /// Runs the action bound to a drawn gesture by feeding the same command the toolbar would send.
    fn gesture(&mut self, path: &str, flow: &mut ControlFlow) {
        let Some(action) = self.cfg.gestures.action_for(path).map(String::from) else { return };
        let cmd = match action.as_str() {
            "back" => json!({"cmd": "back"}),
            "forward" => json!({"cmd": "forward"}),
            "reload" => json!({"cmd": "reload"}),
            "stop" => json!({"cmd": "stop"}),
            "new_tab" => json!({"cmd": "new"}),
            "new_ghost" => json!({"cmd": "new", "private": true}),
            "close_tab" => json!({"cmd": "close", "id": self.active}),
            "reopen" => json!({"cmd": "reopen"}),
            "next_tab" => json!({"cmd": "tab_next"}),
            "prev_tab" => json!({"cmd": "tab_prev"}),
            "home" => json!({"cmd": "go", "text": self.cfg.home}),
            "bookmark" => json!({"cmd": "bookmark"}),
            "mute_tab" => json!({"cmd": "mute", "id": self.active}),
            "fullscreen" => json!({"cmd": "fullscreen"}),
            "print" => json!({"cmd": "print"}),
            "focus_url" => json!({"cmd": "focus_url"}),
            "scroll_top" => json!({"cmd": "scroll_top"}),
            "scroll_bottom" => json!({"cmd": "scroll_bottom"}),
            _ => return,
        };
        self.ipc(&cmd.to_string(), flow);
    }

    /// Applies the remembered zoom for the tab's site (Ghost tabs keep their own session-only level).
    fn apply_zoom(&mut self, id: usize) {
        let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) else { return };
        if t.private {
            return;
        }
        let want = self.cfg.zoom.get(host_of(&t.url)).copied().unwrap_or(1.0);
        if (t.zoom - want).abs() > 0.001 {
            t.zoom = want;
            let _ = t.wv.zoom(want);
        }
    }

    fn zoom_cmd(&mut self, dir: &str) {
        let Some(t) = self.tabs.iter_mut().find(|t| t.id == self.active) else { return };
        let next = config::zoom_step(t.zoom, dir);
        if (next - t.zoom).abs() < 0.001 {
            return;
        }
        t.zoom = next;
        let _ = t.wv.zoom(next);
        if !t.private && !t.url.starts_with("imc://") {
            let host = host_of(&t.url).to_string();
            if !host.is_empty() {
                if (next - 1.0).abs() < 0.001 {
                    self.cfg.zoom.remove(&host);
                } else {
                    self.cfg.zoom.insert(host, next);
                }
                self.cfg.save();
            }
        }
        self.push();
    }

    /// Refreshes what the imc:// pages show about the running browser.
    fn sync_internal(&self) {
        let mut d = self.internal.write().unwrap();
        d.webview = wry::webview_version().unwrap_or_else(|_| "unknown".into());
        d.flags = self.args.replace("--", "\n--").trim().to_string();
        d.hw = self.cfg.hardware_acceleration;
        d.proxy = if self.cfg.vpn.enabled { format!("{} {}:{}{}", self.cfg.vpn.kind, self.cfg.vpn.host, self.cfg.vpn.port, if self.cfg.vpn.kill_switch { " (kill switch on)" } else { "" }) } else { "Direct connection (no proxy)".into() };
        d.ext_ids = self.ext_ids.clone();
        d.gestures = self.cfg.gestures.clone();
        d.doh = self.cfg.doh.describe();
    }

    fn on_download(&mut self, e: DlEvent) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let mut progress_only = false;
        match e {
            DlEvent::Started { id, name, path, url, total, private } => {
                self.toast(&format!("Downloading {name}"));
                self.downloads.push(downloads::Item { id, name, path, url, total, received: 0, state: "progress".into(), started: now, private });
            }
            DlEvent::Progress { id, received, total } => {
                if let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) {
                    d.received = received;
                    d.total = total;
                }
                progress_only = true;
            }
            DlEvent::State { id, state } => {
                let mut msg = None;
                if let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) {
                    d.state = state.into();
                    if state == "done" {
                        d.received = d.total.max(d.received);
                        downloads::mark_of_the_web(std::path::Path::new(&d.path), &d.url);
                        msg = Some(format!("Downloaded {}", d.name));
                    } else if state == "failed" {
                        msg = Some(format!("Download failed: {}", d.name));
                    }
                }
                if let Some(m) = msg {
                    self.toast(&m);
                }
                downloads::save(&self.downloads);
            }
        }
        // progress ticks arrive many times a second; keep the UI to about five updates
        if progress_only && self.last_dl_push.elapsed() < Duration::from_millis(200) {
            return;
        }
        self.last_dl_push = Instant::now();
        self.push_downloads(false);
    }

    /// Recent downloads for the UI (`open` also opens the popup).
    fn push_downloads(&self, open: bool) {
        let list: Vec<Value> = self
            .downloads
            .iter()
            .rev()
            .take(30)
            .map(|d| {
                let p = std::path::Path::new(&d.path);
                json!({"id": d.id, "name": d.name, "state": d.state, "received": d.received, "total": d.total, "started": d.started,
                       "exists": d.state != "done" || p.is_file(), "risky": downloads::is_risky(&d.name), "private": d.private})
            })
            .collect();
        let _ = self.chrome.evaluate_script(&format!("window.ui.downloads({},{open})", Value::Array(list)));
    }

    fn upd_send(&self) -> impl Fn(updater::UpdMsg) + Send + 'static {
        let p = self.proxy.clone();
        move |m| {
            let _ = p.send_event(Ev::Update(m));
        }
    }

    fn push_update(&self) {
        let st = self.upd.state_json(self.cfg.update.auto, self.cfg.update.nightly);
        let _ = self.chrome.evaluate_script(&format!("window.ui.updateState({st})"));
    }

    fn toast_act(&self, msg: &str, label: &str, cmd: &str) {
        let _ = self.chrome.evaluate_script(&format!("window.ui.toast({},{{label:{},cmd:{}}})", Value::String(msg.into()), Value::String(label.into()), Value::String(cmd.into())));
    }

    fn check_updates(&mut self, manual: bool) {
        let send = self.upd_send();
        self.upd.start_check(self.cfg.update.nightly, FLAVOR, manual, send);
        self.push_update();
    }

    /// Shows what a check or install found, and starts the install when Auto update is on.
    fn on_update(&mut self, m: updater::UpdMsg) {
        let out = self.upd.handle(m, self.cfg.update.auto);
        if out.checked {
            self.cfg.update.last_check = updater::now_unix();
            self.cfg.save();
        }
        for (msg, act) in out.toasts {
            match act {
                Some((label, cmd)) => self.toast_act(&msg, label, cmd),
                None => self.toast(&msg),
            }
        }
        if out.install {
            let send = self.upd_send();
            self.upd.start_install(send);
        }
        self.push_update();
    }

    fn save_session(&self) {
        let urls: Vec<&str> = self.tabs.iter().filter(|t| !t.private).map(|t| t.url.as_str()).filter(|u| u.starts_with("http")).collect();
        let _ = std::fs::write(config::root().join("session.json"), serde_json::to_string(&urls).unwrap());
    }

    fn relaunch(&self) {
        self.save_session();
        if let Ok(exe) = std::env::current_exe() {
            // Wait for this process (and the WebView2 browser it owns) to release the profile, then start fresh.
            use std::os::windows::process::CommandExt;
            let _ = Command::new("cmd")
                .raw_arg(format!("/C timeout /t 2 /nobreak >nul & start \"\" \"{}\"", exe.display()))
                .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                .spawn();
        }
    }

    fn ipc(&mut self, body: &str, flow: &mut ControlFlow) {
        let Ok(m) = serde_json::from_str::<Value>(body) else { return };
        match m["cmd"].as_str().unwrap_or("") {
            "ready" => self.push(),
            "new" => {
                let home = self.cfg.home.clone();
                let n = self.new_tab(&home, m["private"].as_bool().unwrap_or(false));
                self.activate(n);
            }
            "engine" => {
                if let Some(e) = m["id"].as_str().filter(|e| ["google", "bing", "duckduckgo", "brave", "yahoo"].contains(e)) {
                    self.cfg.search_engine = e.into();
                    self.cfg.save();
                    self.push();
                }
            }
            "close" => {
                if let Some(id) = m["id"].as_u64() {
                    self.close(id as usize);
                }
            }
            "switch" => {
                if let Some(id) = m["id"].as_u64() {
                    self.activate(id as usize);
                }
            }
            "go" => self.navigate(m["text"].as_str().unwrap_or("")),
            "back" => self.eval_active("history.back()"),
            "forward" => self.eval_active("history.forward()"),
            "reload" => {
                if let Some(t) = self.tabs.iter().find(|t| t.id == self.active) {
                    let _ = t.wv.reload();
                }
            }
            "toolbar" => {
                let list = |k: &str| m[k].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect::<Vec<_>>()).unwrap_or_default();
                self.cfg.toolbar = config::Toolbar { left: list("left"), right: list("right") };
                self.cfg.save();
                self.push();
            }
            "stop" => self.eval_active("window.stop()"),
            "print" => self.eval_active("window.print()"),
            "fullscreen" => {
                let on = self.window.fullscreen().is_none();
                self.window.set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
            }
            "reopen" => {
                if let Some(u) = self.closed.pop() {
                    let n = self.new_tab(&u, false);
                    self.activate(n);
                }
            }
            "ext_open" => {
                let dir = m["dir"].as_str().unwrap_or("");
                let Some(it) = self.ext_items.iter().find(|e| e.dir == dir).cloned() else { return };
                let id = self
                    .ext_ids
                    .iter()
                    .find(|(_, n)| *n == it.name)
                    .or_else(|| if self.ext_items.len() == 1 { self.ext_ids.first() } else { None })
                    .map(|(i, _)| i.clone());
                match (id, it.popup.or(it.options)) {
                    (Some(id), Some(page)) => {
                        let n = self.new_tab(&format!("chrome-extension://{id}/{page}"), false);
                        self.activate(n);
                    }
                    (None, _) => self.toast("Extension ids are not available yet. Try again in a few seconds."),
                    (_, None) => self.toast("This extension has no popup or options page to open."),
                }
            }
            "cards" => self.push_cards(),
            "card_add" => {
                let mut list = cards::load();
                let (month, year) = (m["month"].as_u64().unwrap_or(0) as u8, m["year"].as_u64().unwrap_or(0) as u16);
                match cards::add(&mut list, m["label"].as_str().unwrap_or(""), m["name"].as_str().unwrap_or(""), m["number"].as_str().unwrap_or(""), month, year) {
                    Ok(what) => {
                        cards::save(&list);
                        self.toast(&format!("Saved {what}. The number is encrypted; the security code is never stored."));
                    }
                    Err(e) => self.toast(&e),
                }
                self.push_cards();
            }
            "card_delete" => {
                let mut list = cards::load();
                if let Some(i) = m["i"].as_u64().map(|i| i as usize).filter(|&i| i < list.len()) {
                    list.remove(i);
                    cards::save(&list);
                }
                self.push_cards();
            }
            "card_fill" => self.fill_card(m["i"].as_u64().unwrap_or(u64::MAX) as usize),
            "gestures" => {
                if let Ok(mut g) = serde_json::from_value::<config::Gestures>(m["gestures"].clone()) {
                    let mut seen = HashSet::new();
                    g.binds.retain(|b| config::Gestures::clean_path(&b.path).is_some() && config::GESTURE_ACTIONS.iter().any(|(id, _)| *id == b.action) && seen.insert(b.path.clone()));
                    g.min_distance = g.min_distance.clamp(10, 120);
                    self.cfg.gestures = g;
                    self.cfg.save();
                    self.sync_internal();
                    let js = format!("window.__imcGset&&window.__imcGset({})", self.cfg.gestures.page_config());
                    for t in &self.tabs {
                        let _ = t.wv.evaluate_script(&js);
                    }
                    self.push();
                }
            }
            // Dev hook (only with IMC_SELFTEST): run script in the active page so gestures can be exercised unattended.
            "selftest_eval" if std::env::var_os("IMC_SELFTEST").is_some() => self.eval_active(m["js"].as_str().unwrap_or("")),
            "tab_next" | "tab_prev" => {
                if let Some(i) = self.tabs.iter().position(|t| t.id == self.active) {
                    let n = self.tabs.len();
                    let j = if m["cmd"] == "tab_next" { (i + 1) % n } else { (i + n - 1) % n };
                    let id = self.tabs[j].id;
                    self.activate(id);
                }
            }
            "focus_url" => {
                let _ = self.chrome.focus();
                let _ = self.chrome.evaluate_script("document.getElementById('url').focus()");
            }
            "scroll_top" => self.eval_active("window.scrollTo({top:0,behavior:'smooth'})"),
            "scroll_bottom" => self.eval_active("window.scrollTo({top:document.documentElement.scrollHeight,behavior:'smooth'})"),
            "site_info" => {
                let Some(t) = self.tabs.iter().find(|t| t.id == self.active) else { return };
                let url = t.url.clone();
                let st: Value = self.sec.get(&t.id).and_then(|s| serde_json::from_str(s).ok()).unwrap_or(Value::Null);
                let c = &st["visibleSecurityState"]["certificateSecurityState"];
                let cert = if c.is_object() {
                    json!({"subject": c["subjectName"], "issuer": c["issuer"], "from": c["validFrom"], "to": c["validTo"], "protocol": c["protocol"], "cipher": c["cipher"], "keyExchange": c["keyExchange"], "error": c["certificateNetworkError"]})
                } else {
                    Value::Null
                };
                let web = url.starts_with("http://") || url.starts_with("https://");
                let info = json!({"url": url, "host": host_of(&url), "secure": url.starts_with("https://"), "local": !web, "cert": cert, "clearable": web});
                if std::env::var_os("IMC_SELFTEST").is_some() {
                    eprintln!("SELFTEST siteinfo {info}");
                }
                let _ = self.chrome.evaluate_script(&format!("window.ui.siteInfo({info})"));
            }
            "site_clear" => {
                let Some(t) = self.tabs.iter().find(|t| t.id == self.active) else { return };
                let (url, kind) = (t.url.clone(), m["kind"].as_str().unwrap_or("").to_string());
                let host = host_of(&url).to_string();
                let site = adblock::registrable(&host).to_string();
                let origin = url.split('/').take(3).collect::<Vec<_>>().join("/");
                let p = self.proxy.clone();
                let sent = |msg: String| {
                    let _ = p.send_event(Ev::Toast(msg));
                };
                let ok = match kind.as_str() {
                    "cookies" => {
                        let label = site.clone();
                        let p2 = self.proxy.clone();
                        winhook::clear_cookies(&t.wv, site, move |n| {
                            let _ = p2.send_event(Ev::Toast(format!("Cleared {n} cookies for {label}.")));
                        })
                        .is_ok()
                    }
                    "data" => {
                        let (p2, id) = (self.proxy.clone(), t.id);
                        let _ = id;
                        winhook::cdp(&t.wv, "Storage.clearDataForOrigin", &json!({"origin": origin, "storageTypes": "all"}).to_string(), move |_| {
                            let _ = p2.send_event(Ev::Toast(format!("Cleared site data for {host}. Reloading.")));
                        })
                        .is_ok()
                    }
                    "cache" => {
                        let p2 = self.proxy.clone();
                        winhook::cdp(&t.wv, "Network.clearBrowserCache", "{}", move |_| {
                            let _ = p2.send_event(Ev::Toast("Cleared the browser cache.".into()));
                        })
                        .is_ok()
                    }
                    _ => false,
                };
                if !ok {
                    sent("Could not clear that data.".into());
                } else if kind == "data" {
                    let _ = t.wv.reload();
                }
            }
            "dl_list" => self.push_downloads(true),
            "dl_open" | "dl_show" | "dl_cancel" | "dl_remove" => {
                let id = m["id"].as_u64().unwrap_or(0);
                let Some(i) = self.downloads.iter().position(|d| d.id == id) else { return };
                let path = std::path::PathBuf::from(&self.downloads[i].path);
                match m["cmd"].as_str().unwrap_or("") {
                    "dl_open" => {
                        if self.downloads[i].state == "done" && downloads::inside_downloads(&path) {
                            if !downloads::open_file(&path) {
                                self.toast("Windows could not open that file.");
                            }
                        } else {
                            self.toast("That file is no longer in your Downloads folder.");
                        }
                    }
                    "dl_show" => {
                        if path.is_file() {
                            downloads::reveal(&path);
                        } else {
                            downloads::open_folder(&downloads::dir());
                        }
                    }
                    "dl_cancel" => winhook::cancel_download(id),
                    _ => {
                        self.downloads.remove(i);
                        downloads::save(&self.downloads);
                        self.push_downloads(false);
                    }
                }
            }
            "dl_clear" => {
                self.downloads.retain(|d| d.state == "progress");
                downloads::save(&self.downloads);
                self.push_downloads(false);
            }
            "dl_folder" => downloads::open_folder(&downloads::dir()),
            "open_settings" => {
                let sec = m["section"].as_str().unwrap_or("general").to_string();
                let _ = self.chrome.evaluate_script(&format!("panel(true);section({})", Value::String(sec)));
            }
            "hwaccel" => {
                self.cfg.hardware_acceleration = m["enabled"].as_bool().unwrap_or(true);
                self.cfg.save();
                self.sync_internal();
            }
            "restart" => {
                self.relaunch();
                *flow = ControlFlow::Exit;
            }
            "doh" => {
                let provider = m["provider"].as_str().unwrap_or("cloudflare");
                let ok = provider == "custom" || config::DOH_PROVIDERS.iter().any(|(id, _, _)| *id == provider);
                self.cfg.doh = config::Doh {
                    enabled: m["enabled"].as_bool().unwrap_or(false),
                    provider: if ok { provider.into() } else { "cloudflare".into() },
                    custom: m["custom"].as_str().unwrap_or("").trim().chars().take(300).collect(),
                    strict: m["strict"].as_bool().unwrap_or(false),
                };
                self.cfg.save();
                self.sync_internal();
                if self.cfg.doh.enabled && self.cfg.doh.template().is_none() {
                    self.toast("That server address is not valid. It must start with https://");
                } else {
                    self.toast("Secure DNS saved. Restart the browser to apply it.");
                }
            }
            "update_state" => self.push_update(),
            "update_prefs" => {
                self.cfg.update.auto = m["auto"].as_bool().unwrap_or(false);
                let nightly = m["nightly"].as_bool().unwrap_or(false);
                let channel_changed = nightly != self.cfg.update.nightly;
                self.cfg.update.nightly = nightly;
                self.cfg.save();
                if channel_changed {
                    self.upd.available = None;
                }
                if self.cfg.update.auto || self.cfg.update.nightly {
                    self.check_updates(false);
                } else {
                    self.push_update();
                }
            }
            "update_check" => self.check_updates(true),
            "update_install" => {
                let send = self.upd_send();
                self.upd.start_install(send);
                self.push_update();
            }
            "update_page" => {
                let n = self.new_tab(&updater::release_page(), false);
                self.activate(n);
            }
            "zoom_in" => self.zoom_cmd("in"),
            "zoom_out" => self.zoom_cmd("out"),
            "zoom_reset" => self.zoom_cmd("reset"),
            "win_drag" => {
                let _ = self.window.drag_window();
            }
            "win_min" => self.window.set_minimized(true),
            "win_max" => self.window.set_maximized(!self.window.is_maximized()),
            "win_close" => {
                self.save_session();
                *flow = ControlFlow::Exit;
            }
            "adblock" => {
                let on = !self.shield.enabled.load(Ordering::Relaxed);
                self.shield.enabled.store(on, Ordering::Relaxed);
                self.cfg.adblock = on;
                self.cfg.save();
                self.push();
            }
            "panel" => {
                self.panel = m["open"].as_bool().unwrap_or(false);
                self.layout();
            }
            "settings" => {
                if let Ok(mut c) = serde_json::from_value::<Config>(m["cfg"].clone()) {
                    c.permission_allow = self.cfg.permission_allow.clone();
                    c.home = self.cfg.home.clone();
                    c.search_engine = self.cfg.search_engine.clone();
                    c.toolbar = self.cfg.toolbar.clone();
                    c.gestures = self.cfg.gestures.clone();
                    c.zoom = self.cfg.zoom.clone();
                    c.hardware_acceleration = self.cfg.hardware_acceleration;
                    c.update = self.cfg.update.clone();
                    c.doh = self.cfg.doh.clone();
                    c.theme = self.cfg.theme.clone();
                    c.vertical_tabs = self.cfg.vertical_tabs;
                    c.save();
                    self.relaunch();
                    *flow = ControlFlow::Exit;
                }
            }
            // Live preferences (no restart): theme, tab layout, sleep delay.
            "prefs" => {
                if let Ok(t) = serde_json::from_value::<config::Theme>(m["theme"].clone()) {
                    self.cfg.theme = t;
                }
                self.cfg.vertical_tabs = m["vertical_tabs"].as_bool().unwrap_or(self.cfg.vertical_tabs);
                self.cfg.freeze_after_secs = m["freeze_after_secs"].as_u64().unwrap_or(self.cfg.freeze_after_secs).max(10);
                self.cfg.save();
                self.layout();
                self.push();
            }
            "mute" => {
                if let Some(t) = m["id"].as_u64().and_then(|id| self.tabs.iter_mut().find(|t| t.id == id as usize)) {
                    t.muted = !t.muted;
                    winhook::set_muted(&t.wv, t.muted);
                }
                self.push();
            }
            "bookmark" => {
                let Some(t) = self.tabs.iter().find(|t| t.id == self.active && !t.private) else { return };
                if let Some(i) = self.bookmarks.iter().position(|b| b.url == t.url) {
                    self.bookmarks.remove(i);
                } else if t.url.starts_with("http") {
                    let title = if t.title.is_empty() { t.url.clone() } else { t.title.clone() };
                    self.bookmarks.push(bookmarks::Bookmark { title, url: t.url.clone(), folder: String::new() });
                }
                bookmarks::save(&self.bookmarks);
                self.push();
            }
            "bm_delete" => {
                if let Some(i) = m["i"].as_u64().map(|i| i as usize).filter(|&i| i < self.bookmarks.len()) {
                    self.bookmarks.remove(i);
                    bookmarks::save(&self.bookmarks);
                    self.push();
                }
            }
            "import_bookmarks" => {
                let found = bookmarks::parse_auto(m["text"].as_str().unwrap_or(""));
                let (total, added) = (found.len(), bookmarks::merge(&mut self.bookmarks, found));
                bookmarks::save(&self.bookmarks);
                self.toast(&format!("Bookmarks: {added} imported, {} duplicates/invalid skipped", total - added));
                self.push();
            }
            "import_browsers" => {
                let mut report = vec![];
                for (name, path) in bookmarks::browser_files() {
                    let text = std::fs::read_to_string(&path).unwrap_or_default();
                    let found = bookmarks::parse_auto(&text);
                    report.push(format!("{name}: {}", bookmarks::merge(&mut self.bookmarks, found)));
                }
                bookmarks::save(&self.bookmarks);
                self.toast(&if report.is_empty() {
                    "No Chrome/Edge/Brave/Vivaldi profile found. Export bookmarks as HTML and import the file.".to_string()
                } else {
                    format!("Imported bookmarks - {}", report.join(", "))
                });
                self.push();
            }
            "creds" => self.push_creds(),
            "import_passwords" => {
                let mut c = passwords::load();
                match passwords::import_csv(&mut c, m["text"].as_str().unwrap_or("")) {
                    Ok((ok, skip)) => {
                        passwords::save(&c);
                        self.toast(&format!("Passwords: {ok} imported and encrypted, {skip} skipped. Delete the CSV file now: it is plaintext."));
                    }
                    Err(e) => self.toast(&format!("Password import failed: {e}")),
                }
                self.push_creds();
            }
            "pw_delete" => {
                let mut c = passwords::load();
                if let Some(i) = m["i"].as_u64().map(|i| i as usize).filter(|&i| i < c.len()) {
                    c.remove(i);
                    passwords::save(&c);
                }
                self.push_creds();
            }
            "pw_reveal" => {
                let c = passwords::load();
                if let Some(pw) = m["i"].as_u64().and_then(|i| c.get(i as usize)).and_then(|c| passwords::decrypt(&c.enc)) {
                    let _ = self.chrome.evaluate_script(&format!("window.ui.revealed({},{})", m["i"], Value::String(pw)));
                }
            }
            "pw_fill" => self.fill_password(m["i"].as_u64().unwrap_or(u64::MAX) as usize),
            _ => {}
        }
    }

    /// List without numbers: brand, last four and expiry only.
    fn push_cards(&self) {
        let list: Vec<Value> = cards::load()
            .iter()
            .enumerate()
            .map(|(i, c)| json!({"i": i, "label": c.label, "name": c.name, "brand": c.brand, "last4": c.last4, "month": c.exp_month, "year": c.exp_year}))
            .collect();
        let _ = self.chrome.evaluate_script(&format!("window.ui.cardsList({})", Value::Array(list)));
    }

    /// Fill only on an explicit click, only on an HTTPS page, only the main frame's matching fields.
    fn fill_card(&self, i: usize) {
        let Some(tab) = self.tabs.iter().find(|t| t.id == self.active) else { return };
        if !tab.url.starts_with("https://") {
            self.toast("Not filled: card details are only filled on HTTPS pages.");
            return;
        }
        let list = cards::load();
        let Some(card) = list.get(i) else { return };
        let Some(num) = cards::number(card) else {
            self.toast("This card could not be decrypted (it was saved under another Windows account).");
            return;
        };
        let js = format!(
            "(function(num,name,mm,yyyy){{const set=(e,v)=>{{const P=e.tagName==='SELECT'?HTMLSelectElement.prototype:HTMLInputElement.prototype;\
             Object.getOwnPropertyDescriptor(P,'value').set.call(e,v);e.dispatchEvent(new Event('input',{{bubbles:true}}));e.dispatchEvent(new Event('change',{{bubbles:true}}))}};\
             const find=(ac,rx)=>document.querySelector('[autocomplete~=\"'+ac+'\"]')||[...document.querySelectorAll('input,select')].find(e=>e.type!=='hidden'&&rx.test([e.name,e.id,e.getAttribute('aria-label'),e.placeholder].join(' ')));\
             const pick=(e,vals)=>{{if(!e)return;if(e.tagName==='SELECT'){{const o=[...e.options].find(o=>vals.includes(o.value)||vals.includes(o.text.trim()));if(o)set(e,o.value)}}else set(e,vals[0])}};\
             pick(find('cc-number',/card.?n|cc.?num|number/i),[num]);pick(find('cc-name',/card.?holder|name.?on|cc.?name/i),[name]);\
             const m=find('cc-exp-month',/exp.*month|month/i),y=find('cc-exp-year',/exp.*year|year/i),exp=find('cc-exp',/exp|mm.?.?yy/i);\
             pick(m,[mm,String(+mm)]);pick(y,[yyyy,yyyy.slice(2)]);\
             if(exp&&exp!==m&&exp!==y)pick(exp,[mm+'/'+yyyy.slice(2),mm+' / '+yyyy.slice(2),mm+yyyy.slice(2)])}})({},{},{},{})",
            Value::String(num),
            Value::String(card.name.clone()),
            Value::String(format!("{:02}", card.exp_month)),
            Value::String(card.exp_year.to_string())
        );
        let _ = tab.wv.evaluate_script(&js);
        self.toast(&format!("Filled {} ending {}. Enter the security code yourself.", card.brand, card.last4));
    }

    fn toast(&self, msg: &str) {
        let _ = self.chrome.evaluate_script(&format!("window.ui.toast({})", Value::String(msg.into())));
    }

    /// Credential list without secrets; `match` flags entries for the active site.
    fn push_creds(&self) {
        let active_host = self.tabs.iter().find(|t| t.id == self.active).map(|t| passwords::host(&t.url)).unwrap_or_default();
        let list: Vec<Value> = passwords::load()
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let h = passwords::host(&c.url);
                let hit = !active_host.is_empty() && adblock::registrable(&h) == adblock::registrable(&active_host);
                json!({"i": i, "url": c.url, "host": h, "username": c.username, "match": hit})
            })
            .collect();
        let _ = self.chrome.evaluate_script(&format!("window.ui.creds({})", Value::Array(list)));
    }

    /// Fill only on explicit click, only over HTTPS, only on the saved site's own domain.
    fn fill_password(&self, i: usize) {
        let Some(tab) = self.tabs.iter().find(|t| t.id == self.active) else { return };
        let c = passwords::load();
        let Some(cred) = c.get(i) else { return };
        let same = adblock::registrable(&passwords::host(&cred.url)) == adblock::registrable(&passwords::host(&tab.url));
        if !tab.url.starts_with("https://") || !same {
            self.toast("Not filled: the open page is not an HTTPS page on this login's site.");
            return;
        }
        let Some(pw) = passwords::decrypt(&cred.enc) else { return };
        let js = format!(
            "(function(u,p){{const ins=[...document.querySelectorAll('input')],pw=ins.find(e=>e.type==='password');if(!pw)return;\
             const us=ins.slice(0,ins.indexOf(pw)).reverse().find(e=>['text','email','tel'].includes(e.type||'text'));\
             const set=(e,v)=>{{Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(e,v);\
             e.dispatchEvent(new Event('input',{{bubbles:true}}));e.dispatchEvent(new Event('change',{{bubbles:true}}))}};\
             if(us)set(us,u);set(pw,p)}})({},{})",
            Value::String(cred.username.clone()),
            Value::String(pw)
        );
        let _ = tab.wv.evaluate_script(&js);
        self.toast("Filled.");
    }

    fn eval_active(&self, js: &str) {
        if let Some(t) = self.tabs.iter().find(|t| t.id == self.active) {
            let _ = t.wv.evaluate_script(js);
        }
    }

    /// Hibernate idle background tabs.
    fn tick(&mut self) {
        let limit = Duration::from_secs(self.cfg.freeze_after_secs);
        let mut changed = false;
        for t in &mut self.tabs {
            if t.id != self.active && !t.frozen && !t.audible && t.last_active.elapsed() >= limit {
                winhook::suspend(&t.wv);
                t.frozen = true;
                changed = true;
            }
        }
        if changed || self.shield.blocked.load(Ordering::Relaxed) > 0 {
            self.push();
        }
    }
}

fn cli(args: &[String]) -> bool {
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    let Some(first) = args.first() else { return false };
    if !first.starts_with("--") {
        return false;
    }
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
    match first.as_str() {
        "--install-extension" => match args.get(1).map(|s| ext::install(s)) {
            Some(Ok(m)) => println!("{m}"),
            Some(Err(e)) => eprintln!("error: {e}"),
            None => eprintln!("usage: imc-browse --install-extension <web-store-id | .crx/.xpi/.zip | https-url | folder>"),
        },
        "--list-extensions" => {
            for (dir, name, ver) in ext::list() {
                println!("{dir}\t{name}\t{ver}");
            }
        }
        "--remove-extension" => match args.get(1).map(|s| ext::remove(s)) {
            Some(Ok(())) => println!("removed"),
            Some(Err(e)) => eprintln!("error: {e}"),
            None => eprintln!("usage: imc-browse --remove-extension <folder-name>"),
        },
        "--update-lists" => {
            const LISTS: &[(&str, &str)] = &[
                ("easylist", "https://easylist.to/easylist/easylist.txt"),
                ("easyprivacy", "https://easylist.to/easylist/easyprivacy.txt"),
                ("ublock-filters", "https://raw.githubusercontent.com/uBlockOrigin/uAssets/master/filters/filters.txt"),
                ("ublock-privacy", "https://raw.githubusercontent.com/uBlockOrigin/uAssets/master/filters/privacy.txt"),
                ("ublock-badware", "https://raw.githubusercontent.com/uBlockOrigin/uAssets/master/filters/badware.txt"),
            ];
            for (name, url) in LISTS {
                let out = config::lists_dir().join(format!("{name}.txt"));
                let ok = Command::new("curl.exe").args(["-fsSL", "-o"]).arg(&out).arg(url).status().is_ok_and(|s| s.success());
                println!("{} {name}", if ok { "updated" } else { "FAILED " });
            }
        }
        _ => println!(
            "imc-browse [URL...]\n  --install-extension <id|file|url|folder>\n  --list-extensions\n  --remove-extension <name>\n  --update-lists"
        ),
    }
    true
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if cli(&args) {
        return;
    }
    sandbox::contain_process_tree();

    let cfg = Config::load();
    updater::cleanup();
    // Secure DNS lives in the engine's Local State file; set it before the engine starts.
    config::write_secure_dns(&config::profile_dir().join("EBWebView"), &cfg.doh);
    vpn::start_watchdog(cfg.vpn.clone());

    let shield = Arc::new(Shield {
        engine: Arc::new(RwLock::new(Arc::new(load_engine()))),
        enabled: AtomicBool::new(cfg.adblock),
        kill_switch: AtomicBool::new(cfg.vpn.enabled && cfg.vpn.kill_switch),
        blocked: AtomicUsize::new(0),
    });

    let event_loop = EventLoopBuilder::<Ev>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let window = WindowBuilder::new()
        .with_title("IMC Browse")
        .with_decorations(false)
        .with_inner_size(LogicalSize::new(1280.0, 800.0))
        .build(&event_loop)
        .expect("window");

    let browser_args = format!("{BASE_ARGS} --enable-features=StrictOriginIsolation{}{}", gpu_args(cfg.hardware_acceleration), vpn::browser_args(&cfg.vpn));
    let p_ipc = proxy.clone();
    let mut ctx = WebContext::new(Some(config::profile_dir()));
    let size: LogicalSize<f64> = window.inner_size().to_logical(window.scale_factor());
    let chrome = WebViewBuilder::new_with_web_context(&mut ctx)
        .with_bounds(Rect {
            position: LogicalPosition::new(0.0, 0.0).into(),
            size: LogicalSize::new(size.width, size.height).into(),
        })
        .with_html(CHROME_HTML)
        .with_additional_browser_args(browser_args.clone())
        .with_browser_extensions_enabled(true)
        .with_ipc_handler(move |r| {
            let _ = p_ipc.send_event(Ev::Ipc(r.body().clone()));
        })
        .build_as_child(&window)
        .expect("chrome ui");

    let mut app = App {
        window,
        chrome,
        ctx,
        private_ctx: None,
        tabs: vec![],
        active: 0,
        next: 1,
        cfg: cfg.clone(),
        proxy: proxy.clone(),
        shield,
        insecure_ok: Arc::new(Mutex::new(HashSet::new())),
        args: browser_args,
        panel: false,
        bookmarks: bookmarks::load(),
        closed: vec![],
        ext_items: ext::items(),
        ext_ids: vec![],
        sec: Default::default(),
        downloads: downloads::load(),
        dl_ids: Arc::new(std::sync::atomic::AtomicU64::new(1)),
        last_dl_push: Instant::now(),
        upd: updater::Ctl::new(),
        internal: Arc::new(RwLock::new(internal::Internal { webview: String::new(), flags: String::new(), hw: true, proxy: String::new(), doh: String::new(), ext_ids: vec![], gestures: cfg.gestures.clone() })),
    };
    app.sync_internal();
    // Dev hook: dump every imc:// page to disk so the styling can be reviewed without opening the app.
    if let Some(dir) = std::env::var_os("IMC_DUMP_PAGES") {
        let dir = std::path::PathBuf::from(dir);
        let _ = std::fs::create_dir_all(&dir);
        for name in ["", "version", "gpu", "extensions", "bookmarks", "blocker", "shortcuts"] {
            if let Some(h) = internal::page(name, &app.internal.read().unwrap(), &app.shield) {
                let _ = std::fs::write(dir.join(format!("{}.html", if name.is_empty() { "index" } else { name })), h);
            }
        }
    }

    // Tabs: CLI urls > restored session > home.
    let _ = std::fs::remove_dir_all(config::private_root()); // leftovers from a crashed run
    let mut urls: Vec<String> = args.iter().map(|a| normalize(a, &cfg.search_engine).0).collect();
    let session = config::root().join("session.json");
    if urls.is_empty() {
        if let Some(s) = std::fs::read_to_string(&session).ok().and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok()) {
            urls = s;
        }
    }
    let _ = std::fs::remove_file(&session);
    if urls.is_empty() {
        urls.push(cfg.home.clone());
    }
    let mut first = 0;
    for u in &urls {
        let id = app.new_tab(u, false);
        if first == 0 {
            first = id;
        }
    }
    app.activate(first);

    // Dev hook: scripted UI commands so behaviour can be checked without touching the screen.
    if std::env::var_os("IMC_SELFTEST").is_some() {
        let t = proxy.clone();
        let dirs: Vec<String> = ext::items().into_iter().map(|e| e.dir).collect();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(14));
            for d in dirs {
                let _ = t.send_event(Ev::Ipc(json!({"cmd": "ext_open", "dir": d}).to_string()));
            }
        });
        // Certificate popup + clearing: set a cookie, read site info, clear cookies, clear site data.
        let t3 = proxy.clone();
        std::thread::spawn(move || {
            let send = |ms: u64, v: Value| {
                std::thread::sleep(Duration::from_millis(ms));
                let _ = t3.send_event(Ev::Ipc(v.to_string()));
            };
            send(5000, json!({"cmd": "selftest_eval", "js": "document.cookie='imc_test=1; path=/'; document.cookie='imc_test2=2; path=/'; console.log(document.cookie)"}));
            send(600, json!({"cmd": "site_info"}));
            send(600, json!({"cmd": "site_clear", "kind": "cookies"}));
            send(5000, json!({"cmd": "site_clear", "kind": "data"}));
            send(1000, json!({"cmd": "site_clear", "kind": "cache"}));
        });
        // Zoom: two Ctrl+wheel notches in, one out, then Ctrl+0 via the keyboard shortcut.
        let t4 = proxy.clone();
        std::thread::spawn(move || {
            let wheel = |dy: i32| format!("window.dispatchEvent(new WheelEvent('wheel',{{deltaY:{dy},ctrlKey:true,bubbles:true,cancelable:true}}))");
            let key = "window.dispatchEvent(new KeyboardEvent('keydown',{key:'0',ctrlKey:true,bubbles:true,cancelable:true}))".to_string();
            for (wait, js) in [(3000, wheel(-100)), (800, wheel(-100)), (800, wheel(100)), (1500, key)] {
                std::thread::sleep(Duration::from_millis(wait));
                let _ = t4.send_event(Ev::Ipc(json!({"cmd": "selftest_eval", "js": js}).to_string()));
            }
        });
        // Which browser-internal pages does WebView2 let us open?
        if let Ok(list) = std::env::var("IMC_SELFTEST_URLS") {
            let t5 = proxy.clone();
            let urls: Vec<String> = list.split(' ').map(String::from).collect();
            std::thread::spawn(move || {
                for u in urls {
                    std::thread::sleep(Duration::from_secs(3));
                    let _ = t5.send_event(Ev::Open(u));
                }
            });
        }
        // Synthetic right-button drags: D = new tab, UL = previous tab, UR = next tab, DR = close tab.
        let t2 = proxy.clone();
        std::thread::spawn(move || {
            let drag = |pts: &str| format!("(()=>{{const p={pts};const ev=(t,x,y,b)=>window.dispatchEvent(new MouseEvent(t,{{button:2,buttons:b,clientX:x,clientY:y,bubbles:true,cancelable:true}}));ev('mousedown',p[0][0],p[0][1],2);for(const q of p.slice(1))ev('mousemove',q[0],q[1],2);const l=p[p.length-1];ev('mouseup',l[0],l[1],0)}})()");
            for (wait, pts) in [(9, "[[200,200],[200,260],[200,320]]"), (4, "[[200,200],[200,140],[200,80],[140,80],[80,80]]"), (4, "[[200,200],[200,140],[200,80],[260,80],[320,80]]"), (4, "[[200,200],[200,260],[200,320],[260,320],[320,320]]")] {
                std::thread::sleep(Duration::from_secs(wait));
                let _ = t2.send_event(Ev::Ipc(json!({"cmd": "selftest_eval", "js": drag(pts)}).to_string()));
            }
        });
    }
    let p_tick = proxy.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(5));
        if p_tick.send_event(Ev::Tick).is_err() {
            break;
        }
    });

    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                app.save_session();
                app.tabs.retain(|t| !t.private);
                app.end_private_if_unused(true);
                *flow = ControlFlow::Exit;
            }
            Event::WindowEvent { event: WindowEvent::Resized(_), .. } => {
                app.layout();
                app.push();
            }
            Event::UserEvent(ev) => match ev {
                Ev::Ipc(s) => app.ipc(&s, flow),
                Ev::Url(id, u) => {
                    if let Some(t) = app.tabs.iter_mut().find(|t| t.id == id) {
                        t.url = internal::display(&u).unwrap_or(u);
                    }
                    app.apply_zoom(id);
                    app.push();
                }
                Ev::Title(id, title) => {
                    if let Some(t) = app.tabs.iter_mut().find(|t| t.id == id) {
                        t.title = title;
                    }
                    app.push();
                }
                Ev::Load(id, started) => {
                    if let Some(t) = app.tabs.iter_mut().find(|t| t.id == id) {
                        t.loading = started;
                    }
                    app.push();
                }
                Ev::Audio(id, on) => {
                    if let Some(t) = app.tabs.iter_mut().find(|t| t.id == id) {
                        t.audible = on;
                    }
                    app.push();
                }
                Ev::Go(id, u) => app.go(id, &u),
                Ev::Open(u) => {
                    let private = app.tabs.iter().any(|t| t.id == app.active && t.private);
                    let n = app.new_tab(&u, private);
                    app.activate(n);
                }
                Ev::Tick => {
                    app.tick();
                    if (app.cfg.update.auto || app.cfg.update.nightly) && app.upd.due(app.cfg.update.last_check) {
                        app.check_updates(false);
                    }
                    if std::env::var_os("IMC_SELFTEST").is_some() {
                        let tabs: Vec<String> = app.tabs.iter().map(|t| format!("{}{}|{}|{}|z{}", if t.id == app.active { "*" } else { "" }, t.id, t.url, t.title, (t.zoom * 100.0).round())).collect();
                        eprintln!("SELFTEST downloads {:?}", app.downloads.iter().map(|d| (d.name.clone(), d.state.clone(), d.received, d.total, d.path.clone())).collect::<Vec<_>>());
                        eprintln!("SELFTEST tabs {tabs:?} ext_ids={:?} items={:?}", app.ext_ids, app.ext_items.iter().map(|e| (e.dir.clone(), e.name.clone(), e.icon.as_ref().map(|i| i.len()), e.popup.clone())).collect::<Vec<_>>());
                    }
                    if app.ext_ids.is_empty() && !app.ext_items.is_empty() {
                        let p = app.proxy.clone();
                        let _ = winhook::extension_ids(&app.chrome, move |ids| {
                            let _ = p.send_event(Ev::ExtIds(ids));
                        });
                    }
                }
                Ev::ExtIds(ids) => {
                    app.ext_ids = ids;
                    app.sync_internal();
                }
                Ev::Gesture(path) => app.gesture(&path, flow),
                Ev::Download(e) => app.on_download(e),
                Ev::Update(m) => app.on_update(m),
                Ev::Zoom(d) => app.zoom_cmd(&d),
                Ev::Security(id, j) => {
                    app.sec.insert(id, j);
                }
                Ev::Toast(m) => {
                    if std::env::var_os("IMC_SELFTEST").is_some() {
                        eprintln!("SELFTEST toast {m}");
                    }
                    app.toast(&m)
                }
            },
            _ => {}
        }
    });
}
