//! IMC Browse on CEF (Chromium 156). The tao window owns the frame; the chrome UI and every tab are
//! CEF child windows, laid out exactly like the WebView2 build. Single-threaded: CEF is pumped from
//! the tao loop, so every CEF callback runs on the main thread.
//!
//! The Windows sandbox is off until the bootstrap launcher is set up.

mod adblock;
mod bookmarks;
mod cards;
mod config;
mod downloads;
mod passwords;
mod internal;
mod sandbox;
mod updater;
mod vpn;

use cef::*;
use config::Config;
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    platform::windows::WindowExtWindows,
    window::{Fullscreen, Window, WindowBuilder},
};
use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{HWND_TOP, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW, DestroyWindow, SetWindowPos},
};

/// Which download this build updates itself from (see the release manifest).
const FLAVOR: &str = "cef";
const CHROME_HTML: &str = include_str!("ui/chrome.html");
/// Injected into every page: right-button drag recognition (see ui/gesture.js).
const GESTURE_JS: &str = include_str!("ui/gesture.js");
const TOOLBAR_H: f64 = 52.0;
const TABSTRIP_H: f64 = 36.0;
const SIDEBAR_W: f64 = 230.0;
const TITLEBAR_H: f64 = 32.0;
const UI_ID: usize = 0;

/// Download progress, as reported by CEF's download handler.
pub enum DlEvent {
    Started { id: u64, name: String, path: String, url: String, total: i64, private: bool },
    Progress { id: u64, received: i64, total: i64 },
    State { id: u64, state: &'static str },
}

enum Ev {
    Ipc(String),
    Url(usize, String),
    Title(usize, String),
    Load(usize, bool),
    Open(String),
    Audio(usize, bool),
    /// A drawn gesture path (U/D/L/R) reported by a page.
    Gesture(String),
    Download(DlEvent),
    Toast(String),
    Update(updater::UpdMsg),
    /// Zoom request from a page: "in", "out" or "reset".
    Zoom(String),
    Tick,
    /// A browser finished closing (safe to release anything it was using).
    Closed,
}

thread_local! {
    /// Browser objects are not Send, so they stay on the main thread: id -> browser (0 is the chrome UI).
    static BROWSERS: RefCell<HashMap<usize, Browser>> = RefCell::new(HashMap::new());
}

/// Request filter shared with CEF's IO thread.
struct Shield {
    engine: adblock::Engine,
    css_js: String,
    enabled: AtomicBool,
    blocked: AtomicUsize,
    kill_switch: bool,
    /// Page-side gesture config JSON (ui/gesture.js); kept current when the user edits gestures.
    gest: std::sync::RwLock<String>,
}

fn load_engine() -> adblock::Engine {
    let mut e = adblock::Engine::new();
    e.load_list(include_str!("../data/builtin.txt"));
    if let Ok(rd) = std::fs::read_dir(config::lists_dir()) {
        for f in rd.flatten() {
            if f.path().extension().is_some_and(|x| x == "txt") {
                if let Ok(t) = std::fs::read_to_string(f.path()) {
                    e.load_list(&t);
                }
            }
        }
    }
    e
}

fn host_of(u: &str) -> &str {
    u.split("://").nth(1).and_then(|r| r.split(['/', '?', '#', ':']).next()).unwrap_or("")
}

fn kind_of(rt: &ResourceType) -> u16 {
    if *rt == ResourceType::MAIN_FRAME { adblock::T_DOC }
    else if *rt == ResourceType::SUB_FRAME { adblock::T_SUBDOC }
    else if *rt == ResourceType::STYLESHEET { adblock::T_STYLE }
    else if *rt == ResourceType::SCRIPT { adblock::T_SCRIPT }
    else if *rt == ResourceType::IMAGE { adblock::T_IMAGE }
    else if *rt == ResourceType::FONT_RESOURCE { adblock::T_FONT }
    else if *rt == ResourceType::MEDIA { adblock::T_MEDIA }
    else if *rt == ResourceType::XHR { adblock::T_XHR }
    else if *rt == ResourceType::PING { adblock::T_PING }
    else { adblock::T_OTHER }
}

fn cs(s: &str) -> CefString {
    CefString::from(s)
}

// ---- CEF handlers -------------------------------------------------------------------------

wrap_display_handler! {
    struct Disp {
        id: usize,
        tx: EventLoopProxy<Ev>,
    }
    impl DisplayHandler {
        fn on_address_change(&self, _b: Option<&mut Browser>, frame: Option<&mut Frame>, url: Option<&CefString>) {
            if self.id != UI_ID && frame.is_some_and(|f| f.is_main() != 0) {
                if let Some(u) = url {
                    let _ = self.tx.send_event(Ev::Url(self.id, u.to_string()));
                }
            }
        }
        fn on_title_change(&self, _b: Option<&mut Browser>, title: Option<&CefString>) {
            if self.id != UI_ID {
                if let Some(t) = title {
                    let _ = self.tx.send_event(Ev::Title(self.id, t.to_string()));
                }
            }
        }
        // The chrome UI talks to Rust through console lines; only the UI's own client reads them.
        fn on_console_message(&self, _b: Option<&mut Browser>, _l: LogSeverity, msg: Option<&CefString>, _s: Option<&CefString>, _line: i32) -> i32 {
            if self.id != UI_ID {
                if let Some(m) = msg.map(|m| m.to_string()) {
                    if let Some(d) = m.strip_prefix("IMCZOOM:").filter(|d| ["in", "out", "reset"].contains(d)) {
                        let _ = self.tx.send_event(Ev::Zoom(d.to_string()));
                        return 1;
                    }
                    if let Some(p) = m.strip_prefix("IMCGESTURE:").and_then(config::Gestures::clean_path) {
                        let _ = self.tx.send_event(Ev::Gesture(p));
                        return 1;
                    }
                    if let Some(v) = m.strip_prefix("IMCAUDIO:") {
                        let _ = self.tx.send_event(Ev::Audio(self.id, v == "1"));
                        return 1;
                    }
                }
            }
            if self.id == UI_ID {
                if let Some(m) = msg.map(|m| m.to_string()) {
                    if let Some(body) = m.strip_prefix("IMCIPC:") {
                        let _ = self.tx.send_event(Ev::Ipc(body.to_string()));
                        return 1;
                    }
                }
            }
            0
        }
    }
}

wrap_load_handler! {
    struct Load {
        id: usize,
        tx: EventLoopProxy<Ev>,
        shield: Option<Arc<Shield>>,
    }
    impl LoadHandler {
        // Generic cosmetic filters (no init-script hook in CEF, so this runs after load).
        fn on_load_end(&self, _b: Option<&mut Browser>, frame: Option<&mut Frame>, _status: i32) {
            if let (Some(sh), Some(f)) = (&self.shield, frame) {
                if f.is_main() != 0 {
                    if sh.enabled.load(Ordering::Relaxed) && !sh.css_js.is_empty() {
                        f.execute_java_script(Some(&cs(&sh.css_js)), Some(&cs("imc://cosmetic")), 0);
                    }
                    let g = format!("window.__imcGset&&window.__imcGset({})", sh.gest.read().unwrap());
                    f.execute_java_script(Some(&cs(&g)), Some(&cs("imc://gestures")), 0);
                }
            }
        }
        fn on_loading_state_change(&self, _b: Option<&mut Browser>, is_loading: i32, _back: i32, _fwd: i32) {
            if self.id != UI_ID {
                let _ = self.tx.send_event(Ev::Load(self.id, is_loading != 0));
            }
        }
    }
}

wrap_life_span_handler! {
    struct Life {
        tx: EventLoopProxy<Ev>,
    }
    impl LifeSpanHandler {
        // Default (false) makes CEF send WM_CLOSE to the top-level owner, which is the whole app window.
        // Tabs are children of it, so we take over: returning true lets CEF tear down just this page.
        fn do_close(&self, b: Option<&mut Browser>) -> i32 {
            // Destroy only this page's window; CEF then finishes teardown and calls on_before_close.
            if let Some(h) = b.and_then(|b| b.host()) {
                unsafe {
                    let _ = DestroyWindow(HWND(h.window_handle().0 as *mut _));
                }
            }
            1
        }
        fn on_before_close(&self, _b: Option<&mut Browser>) {
            let _ = self.tx.send_event(Ev::Closed);
        }
        // Pop-ups and target=_blank become new tabs.
        fn on_before_popup(
            &self,
            _b: Option<&mut Browser>,
            _f: Option<&mut Frame>,
            _popup_id: i32,
            target_url: Option<&CefString>,
            _name: Option<&CefString>,
            _disp: WindowOpenDisposition,
            _gesture: i32,
            _features: Option<&PopupFeatures>,
            _wi: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra: Option<&mut Option<DictionaryValue>>,
            _nojs: Option<&mut i32>,
        ) -> i32 {
            if let Some(u) = target_url {
                let _ = self.tx.send_event(Ev::Open(u.to_string()));
            }
            1
        }
    }
}

// Cancels a request the filter already decided to block.
wrap_resource_request_handler! {
    struct Cancel;
    impl ResourceRequestHandler {
        fn on_before_resource_load(&self, _b: Option<&mut Browser>, _f: Option<&mut Frame>, _r: Option<&mut Request>, _cb: Option<&mut Callback>) -> ReturnValue {
            ReturnValue::CANCEL
        }
    }
}

wrap_request_handler! {
    struct Req {
        shield: Arc<Shield>,
        tx: EventLoopProxy<Ev>,
    }
    impl RequestHandler {
        // imc://settings opens the settings panel instead of loading a page
        fn on_before_browse(&self, _b: Option<&mut Browser>, _f: Option<&mut Frame>, request: Option<&mut Request>, _gesture: i32, _redirect: i32) -> i32 {
            let Some(request) = request else { return 0 };
            let url = CefString::from(&request.url()).to_string();
            if let Some(section) = internal::settings_section(&url) {
                let _ = self.tx.send_event(Ev::Ipc(json!({"cmd": "open_settings", "section": section}).to_string()));
                return 1;
            }
            0
        }
        // IO thread, once per request: decide here so allowed requests cost no allocation.
        fn resource_request_handler(
            &self,
            _b: Option<&mut Browser>,
            _f: Option<&mut Frame>,
            request: Option<&mut Request>,
            _nav: i32,
            _dl: i32,
            initiator: Option<&CefString>,
            _disable: Option<&mut i32>,
        ) -> Option<ResourceRequestHandler> {
            let request = request?;
            let url = CefString::from(&request.url()).to_string();
            // imc:// pages are private to the browser: other sites may not load them as subresources
            if url.starts_with("imc://") {
                let from = initiator.map(|i| i.to_string()).unwrap_or_default();
                let kind = kind_of(&request.resource_type());
                return (kind != adblock::T_DOC && !from.is_empty() && !from.starts_with("imc://")).then(Cancel::new);
            }
            let down = self.shield.kill_switch && vpn::TUNNEL_DOWN.load(Ordering::Relaxed);
            if !down && !self.shield.enabled.load(Ordering::Relaxed) {
                return None;
            }
            if !(url.starts_with("http://") || url.starts_with("https://") || url.starts_with("ws")) {
                return None;
            }
            let host = host_of(&url);
            let local = host == "localhost" || host.ends_with(".localhost") || host == "127.0.0.1" || host == "[::1]";
            // Kill switch: fail closed while the proxy endpoint is unreachable.
            if down && !local {
                return Some(Cancel::new());
            }
            let kind = kind_of(&request.resource_type());
            if kind == adblock::T_DOC {
                return None;
            }
            let source = initiator.map(|i| i.to_string()).unwrap_or_default();
            let hit = self.shield.engine.should_block(&adblock::Request { url: &url, source_host: host_of(&source), kind });
            if hit {
                self.shield.blocked.fetch_add(1, Ordering::Relaxed);
                Some(Cancel::new())
            } else {
                None
            }
        }
    }
}

// Zero ambient authority: pages never get camera, microphone, location, etc.
wrap_permission_handler! {
    struct Perm;
    impl PermissionHandler {
        fn on_request_media_access_permission(&self, _b: Option<&mut Browser>, _f: Option<&mut Frame>, _o: Option<&CefString>, _p: u32, _cb: Option<&mut MediaAccessCallback>) -> i32 {
            0
        }
        // Alloy default handling for an unanswered prompt is deny.
        fn on_show_permission_prompt(&self, _b: Option<&mut Browser>, _id: u64, _o: Option<&CefString>, _p: u32, _cb: Option<&mut PermissionPromptCallback>) -> i32 {
            0
        }
    }
}

thread_local! {
    /// CEF's per-session item id -> our id (CEF ids restart at 1 every launch; ours do not).
    static DL_ITEMS: RefCell<HashMap<u32, u64>> = RefCell::new(HashMap::new());
    /// Running downloads, kept so they can be cancelled.
    static DL_CBS: RefCell<HashMap<u64, DownloadItemCallback>> = RefCell::new(HashMap::new());
}

fn cancel_download(id: u64) {
    DL_CBS.with(|m| {
        if let Some(cb) = m.borrow().get(&id) {
            cb.cancel();
        }
    });
}

// Downloads go straight to the system Downloads folder (no dialog) and are tracked in our list.
wrap_download_handler! {
    struct Dl {
        tx: EventLoopProxy<Ev>,
        private: bool,
        ids: Arc<AtomicU64>,
    }
    impl DownloadHandler {
        fn can_download(&self, _b: Option<&mut Browser>, _url: Option<&CefString>, _method: Option<&CefString>) -> i32 {
            1
        }
        fn on_before_download(&self, _b: Option<&mut Browser>, item: Option<&mut DownloadItem>, suggested: Option<&CefString>, cb: Option<&mut BeforeDownloadCallback>) -> i32 {
            let (Some(item), Some(cb)) = (item, cb) else { return 0 };
            let name = downloads::sanitize_name(&suggested.map(|s| s.to_string()).unwrap_or_default());
            let path = downloads::unique_path(&downloads::dir(), &name);
            let id = self.ids.fetch_add(1, Ordering::Relaxed);
            DL_ITEMS.with(|m| m.borrow_mut().insert(item.id(), id));
            cb.cont(Some(&cs(&path.to_string_lossy())), 0);
            let _ = self.tx.send_event(Ev::Download(DlEvent::Started {
                id,
                name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(name),
                path: path.display().to_string(),
                url: CefString::from(&item.url()).to_string(),
                total: item.total_bytes(),
                private: self.private,
            }));
            1
        }
        fn on_download_updated(&self, _b: Option<&mut Browser>, item: Option<&mut DownloadItem>, cb: Option<&mut DownloadItemCallback>) {
            let Some(item) = item else { return };
            let Some(id) = DL_ITEMS.with(|m| m.borrow().get(&item.id()).copied()) else { return };
            let state = if item.is_complete() != 0 {
                Some("done")
            } else if item.is_canceled() != 0 {
                Some("canceled")
            } else if item.is_interrupted() != 0 {
                Some("failed")
            } else {
                None
            };
            match state {
                Some(state) => {
                    DL_CBS.with(|m| m.borrow_mut().remove(&id));
                    DL_ITEMS.with(|m| m.borrow_mut().remove(&item.id()));
                    let _ = self.tx.send_event(Ev::Download(DlEvent::State { id, state }));
                }
                None => {
                    if let Some(cb) = cb {
                        DL_CBS.with(|m| m.borrow_mut().insert(id, cb.clone()));
                    }
                    let _ = self.tx.send_event(Ev::Download(DlEvent::Progress { id, received: item.received_bytes(), total: item.total_bytes() }));
                }
            }
        }
    }
}

/// Serves imc:// pages from memory.
wrap_resource_handler! {
    struct ImcRes {
        body: Arc<Vec<u8>>,
        status: i32,
        pos: Arc<AtomicUsize>,
    }
    impl ResourceHandler {
        fn open(&self, _r: Option<&mut Request>, handle_request: Option<&mut i32>, _cb: Option<&mut Callback>) -> i32 {
            if let Some(h) = handle_request {
                *h = 1; // we have the whole page already
            }
            1
        }
        fn response_headers(&self, response: Option<&mut Response>, length: Option<&mut i64>, _redirect: Option<&mut CefString>) {
            if let Some(r) = response {
                r.set_status(self.status);
                r.set_mime_type(Some(&cs("text/html")));
                for (k, v) in [
                    ("Content-Security-Policy", "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; img-src data:; frame-ancestors 'none'"),
                    ("X-Frame-Options", "DENY"),
                    ("Cache-Control", "no-store"),
                ] {
                    r.set_header_by_name(Some(&cs(k)), Some(&cs(v)), 1);
                }
            }
            if let Some(l) = length {
                *l = self.body.len() as i64;
            }
        }
        fn read(&self, data_out: *mut u8, bytes_to_read: i32, bytes_read: Option<&mut i32>, _cb: Option<&mut ResourceReadCallback>) -> i32 {
            let pos = self.pos.load(Ordering::Relaxed);
            let n = (self.body.len().saturating_sub(pos)).min(bytes_to_read.max(0) as usize);
            if n > 0 {
                unsafe { std::ptr::copy_nonoverlapping(self.body[pos..].as_ptr(), data_out, n) };
                self.pos.store(pos + n, Ordering::Relaxed);
            }
            if let Some(b) = bytes_read {
                *b = n as i32;
            }
            (n > 0) as i32
        }
    }
}

wrap_scheme_handler_factory! {
    struct ImcSchemes {
        data: Arc<RwLock<internal::Internal>>,
        shield: Arc<Shield>,
    }
    impl SchemeHandlerFactory {
        fn create(&self, _b: Option<&mut Browser>, _f: Option<&mut Frame>, _scheme: Option<&CefString>, request: Option<&mut Request>) -> Option<ResourceHandler> {
            let url = CefString::from(&request?.url()).to_string();
            let name = internal::page_name(&url);
            let stats = internal::Stats {
                enabled: self.shield.enabled.load(Ordering::Relaxed),
                blocked: self.shield.blocked.load(Ordering::Relaxed),
                rules: self.shield.engine.rule_count,
                cosmetic: self.shield.engine.cosmetic.len(),
            };
            let (status, html) = match internal::page(&name, &self.data.read().unwrap(), &stats) {
                Some(h) => (200, h),
                None => (404, internal::not_found(&name)),
            };
            Some(ImcRes::new(Arc::new(html.into_bytes()), status, Arc::new(AtomicUsize::new(0))))
        }
    }
}

// Deletes every cookie whose domain belongs to `site` (the registrable domain, so subdomains go too).
wrap_cookie_visitor! {
    struct CookieSweep {
        site: String,
        deleted: Arc<AtomicUsize>,
        tx: EventLoopProxy<Ev>,
    }
    impl CookieVisitor {
        fn visit(&self, cookie: Option<&Cookie>, count: i32, total: i32, delete_cookie: Option<&mut i32>) -> i32 {
            if let Some(c) = cookie {
                let domain = c.domain.to_string();
                let d = domain.trim_start_matches('.');
                if d == self.site || d.ends_with(&format!(".{}", self.site)) {
                    if let Some(del) = delete_cookie {
                        *del = 1;
                    }
                    self.deleted.fetch_add(1, Ordering::Relaxed);
                }
            }
            if count + 1 >= total {
                let _ = self.tx.send_event(Ev::Toast(format!("Cleared {} cookies for {}.", self.deleted.load(Ordering::Relaxed), self.site)));
            }
            1
        }
    }
}

// CEF wants a completion callback for cache clearing; this one tells the user when it is done.
wrap_completion_callback! {
    struct CacheDone {
        tx: EventLoopProxy<Ev>,
    }
    impl CompletionCallback {
        fn on_complete(&self) {
            let _ = self.tx.send_event(Ev::Toast("Cleared the browser cache.".into()));
        }
    }
}

wrap_client! {
    struct Cl {
        id: usize,
        tx: EventLoopProxy<Ev>,
        shield: Option<Arc<Shield>>,
        private: bool,
        dl_ids: Arc<AtomicU64>,
    }
    impl Client {
        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(Disp::new(self.id, self.tx.clone()))
        }
        fn load_handler(&self) -> Option<LoadHandler> {
            Some(Load::new(self.id, self.tx.clone(), self.shield.clone()))
        }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(Life::new(self.tx.clone()))
        }
        fn request_handler(&self) -> Option<RequestHandler> {
            self.shield.clone().map(|s| Req::new(s, self.tx.clone()))
        }
        fn download_handler(&self) -> Option<DownloadHandler> {
            (self.id != UI_ID).then(|| Dl::new(self.tx.clone(), self.private, self.dl_ids.clone()))
        }
        fn permission_handler(&self) -> Option<PermissionHandler> {
            Some(Perm::new())
        }
    }
}

/// Runs at document start in every page, before the page's own scripts. Reports "is this tab
/// making sound" without touching playback (CEF's audio-capture hook silenced it).
const AUDIO_PROBE: &str = r#"(()=>{if(window.__imcA)return;window.__imcA=1;let last=-1;const ctxs=new Set();
const AC=window.AudioContext||window.webkitAudioContext;
if(AC){const W=class extends AC{constructor(...a){super(...a);ctxs.add(this)}};window.AudioContext=W;if(window.webkitAudioContext)window.webkitAudioContext=W}
const poll=()=>{let on=false;
 for(const m of document.querySelectorAll('video,audio'))if(!m.paused&&!m.ended&&!m.muted&&m.volume>0&&m.readyState>2){on=true;break}
 if(!on)for(const c of ctxs)if(c.state==='running'){on=true;break}
 if(on!==last){last=on;console.debug('IMCAUDIO:'+(on?1:0))}};
setInterval(poll,1000)})();"#;

wrap_render_process_handler! {
    struct Rph;
    impl RenderProcessHandler {
        fn on_context_created(&self, _b: Option<&mut Browser>, frame: Option<&mut Frame>, context: Option<&mut V8Context>) {
            if let (Some(f), Some(c)) = (frame, context) {
                if f.is_main() != 0 {
                    let mut rv = None;
                    let mut ex = None;
                    c.eval(Some(&cs(AUDIO_PROBE)), Some(&cs("imc://probe")), 0, Some(&mut rv), Some(&mut ex));
                    let cfg = command_line_get_global().map(|cl| CefString::from(&cl.switch_value(Some(&cs("imc-gestures")))).to_string()).filter(|v| !v.is_empty()).unwrap_or_else(|| "{}".into());
                    let js = format!("{GESTURE_JS}\nwindow.__imcGset({cfg});");
                    c.eval(Some(&cs(&js)), Some(&cs("imc://gesture")), 0, Some(&mut rv), Some(&mut ex));
                }
            }
        }
    }
}

wrap_app! {
    struct ImcApp {
        vpn: config::Vpn,
        gestures: String,
        hw: bool,
    }
    impl App {
        fn render_process_handler(&self) -> Option<RenderProcessHandler> {
            Some(Rph::new())
        }
        // Same privacy posture as the WebView2 build: no background phoning home.
        // imc:// is a real (standard, secure) scheme so internal pages get proper origins.
        fn on_register_custom_schemes(&self, registrar: Option<&mut SchemeRegistrar>) {
            if let Some(r) = registrar {
                r.add_custom_scheme(Some(&cs("imc")), 1 | 8); // STANDARD | SECURE
            }
        }
        fn on_before_command_line_processing(&self, process_type: Option<&CefString>, cmd: Option<&mut CommandLine>) {
            let (Some(cmd), true) = (cmd, process_type.is_none_or(|p| p.to_string().is_empty())) else { return };
            for s in ["disable-background-networking", "disable-sync", "disable-component-update", "disable-domain-reliability",
                      "disable-client-side-phishing-detection", "disable-breakpad", "no-pings", "no-first-run"] {
                cmd.append_switch(Some(&cs(s)));
            }
            cmd.append_switch_with_value(Some(&cs("disable-features")), Some(&cs("MediaRouter,Translate,OptimizationHints,AutofillServerCommunication")));
            cmd.append_switch_with_value(Some(&cs("enable-features")), Some(&cs("StrictOriginIsolation")));
            if !self.hw {
                cmd.append_switch(Some(&cs("disable-gpu")));
                cmd.append_switch(Some(&cs("disable-gpu-compositing")));
            }
            cmd.append_switch_with_value(Some(&cs("imc-gestures")), Some(&cs(&self.gestures)));
            // Proxy, bypass list and WebRTC leak policy come from the same builder as the WebView2 build.
            for tok in vpn::browser_args(&self.vpn).split_whitespace() {
                let tok = tok.trim_start_matches("--").replace('"', "");
                match tok.split_once('=') {
                    Some((k, v)) => cmd.append_switch_with_value(Some(&cs(k)), Some(&cs(v))),
                    None => cmd.append_switch(Some(&cs(&tok))),
                }
            }
        }
    }
}

// ---- Application state --------------------------------------------------------------------

struct Tab {
    id: usize,
    url: String,
    title: String,
    loading: bool,
    audible: bool,
    muted: bool,
    private: bool,
    /// Hibernated: the page was discarded and is rebuilt from `url` when the tab is opened.
    frozen: bool,
    /// Current zoom factor (1.0 = 100%).
    zoom: f64,
    last_active: Instant,
}

struct Shell {
    window: Window,
    tx: EventLoopProxy<Ev>,
    tabs: Vec<Tab>,
    active: usize,
    next: usize,
    cfg: Config,
    panel: bool,
    bookmarks: Vec<bookmarks::Bookmark>,
    ui_ready: bool,
    shield: Arc<Shield>,
    /// In-memory profile shared by all Ghost tabs; dropped (and the data with it) when the last one closes.
    ghost_ctx: Option<RequestContext>,
    /// URLs of recently closed tabs (not Ghost), newest last.
    closed: Vec<String>,
    upd: updater::Ctl,
    /// Live data behind the imc:// pages.
    internal: Arc<RwLock<internal::Internal>>,
    downloads: Vec<downloads::Item>,
    dl_ids: Arc<AtomicU64>,
    last_dl_push: Instant,
}

fn host_hwnd(id: usize) -> Option<HWND> {
    BROWSERS.with(|b| b.borrow().get(&id).and_then(|b| b.host()).map(|h| HWND(h.window_handle().0 as *mut _)))
}

fn place(id: usize, x: i32, y: i32, w: i32, h: i32, show: bool) {
    if let Some(hwnd) = host_hwnd(id) {
        let flag = if show { SWP_SHOWWINDOW } else { SWP_HIDEWINDOW };
        unsafe {
            let _ = SetWindowPos(hwnd, if show && id != UI_ID { Some(HWND_TOP) } else { None }, x, y, w.max(0), h.max(0), SWP_NOACTIVATE | flag | if show && id != UI_ID { Default::default() } else { SWP_NOZORDER });
        }
    }
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

fn normalize(t: &str, engine: &str) -> String {
    let t = t.trim();
    if let Some(u) = internal::from_typed(t) {
        return u;
    }
    let lower = t.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("file://") {
        return t.into();
    }
    if !t.contains(' ') && !t.contains("://") && (t.contains('.') || lower.starts_with("localhost")) && !t.starts_with('.') {
        return format!("https://{t}");
    }
    let q: String = t
        .bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect();
    format!("{}{q}", search_base(engine))
}

impl Shell {
    fn spawn(&mut self, id: usize, url: &str, private: bool) -> bool {
        let r = self.content_rect();
        let info = WindowInfo { runtime_style: RuntimeStyle::ALLOY, ..Default::default() }.set_as_child(sys::HWND(self.window.hwnd() as *mut _), &r);
        let mut client = Cl::new(id, self.tx.clone(), Some(self.shield.clone()), private, self.dl_ids.clone());
        let mut ctx = if private {
            if self.ghost_ctx.is_none() {
                // Empty cache_path = nothing touches the disk.
                self.ghost_ctx = request_context_create_context(Some(&RequestContextSettings::default()), None);
            }
            self.ghost_ctx.clone()
        } else {
            None
        };
        let browser = browser_host_create_browser_sync(Some(&info), Some(&mut client), Some(&cs(url)), Some(&BrowserSettings::default()), None, ctx.as_mut());
        match browser {
            Some(b) => {
                BROWSERS.with(|m| m.borrow_mut().insert(id, b));
                true
            }
            None => {
                eprintln!("tab creation failed");
                false
            }
        }
    }

    fn new_tab(&mut self, url: &str, private: bool) -> usize {
        let id = self.next;
        self.next += 1;
        self.spawn(id, url, private);
        self.tabs.push(Tab { id, url: url.into(), title: String::new(), loading: true, audible: false, muted: false, private, frozen: false, zoom: 1.0, last_active: Instant::now() });
        id
    }

    /// Blank the page so its renderer memory is released; the tab keeps its URL for waking.
    fn discard(&mut self, id: usize) {
        BROWSERS.with(|m| {
            if let Some(b) = m.borrow().get(&id) {
                if let Some(f) = b.main_frame() {
                    f.load_url(Some(&cs("about:blank")));
                }
                if let Some(h) = b.host() {
                    h.was_hidden(1);
                }
            }
        });
    }

    /// Hibernate: tabs unused for `freeze_after_secs` (and not making sound) release their whole renderer.
    fn tick(&mut self) {
        let limit = Duration::from_secs(self.cfg.freeze_after_secs);
        let stale: Vec<usize> = self.tabs.iter().filter(|t| t.id != self.active && !t.frozen && !t.audible && t.last_active.elapsed() >= limit).map(|t| t.id).collect();
        for id in stale {
            self.discard(id);
            if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
                t.frozen = true;
                t.loading = false;
            }
        }
        if std::env::var_os("IMC_SELFTEST").is_some() {
            let sum: Vec<String> = self.tabs.iter().map(|t| format!("{}{}{}{}|{}|{}", t.id, if t.private { "G" } else { "" }, if t.frozen { "Z" } else { "" }, if t.id == self.active { "*" } else { "" }, t.url, t.title)).collect();
            eprintln!("SELFTEST tabs [{}] ghost_ctx={}", sum.join(" "), self.ghost_ctx.is_some());
            eprintln!("SELFTEST downloads {:?}", self.downloads.iter().map(|d| (d.name.clone(), d.state.clone(), d.received, d.total, d.path.clone())).collect::<Vec<_>>());
        }
        self.push();
    }

    fn save_session(&self) {
        let urls: Vec<&str> = self.tabs.iter().filter(|t| !t.private).map(|t| t.url.as_str()).filter(|u| u.starts_with("http")).collect();
        let _ = std::fs::write(config::root().join("session.json"), serde_json::to_string(&urls).unwrap());
    }

    fn relaunch(&self) {
        self.save_session();
        if let Ok(exe) = std::env::current_exe() {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("cmd")
                .raw_arg(format!("/C timeout /t 3 /nobreak >nul & start \"\" \"{}\"", exe.display()))
                .creation_flags(0x0800_0000)
                .spawn();
        }
    }

    fn content_rect(&self) -> Rect {
        let s = self.window.inner_size();
        let k = self.window.scale_factor();
        let (left, top) = if self.cfg.vertical_tabs { (SIDEBAR_W, TITLEBAR_H + TOOLBAR_H) } else { (0.0, TOOLBAR_H + TABSTRIP_H) };
        let (x, y) = ((left * k) as i32, (top * k) as i32);
        Rect { x, y, width: (s.width as i32 - x).max(0), height: (s.height as i32 - y).max(0) }
    }

    fn layout(&self) {
        let s = self.window.inner_size();
        place(UI_ID, 0, 0, s.width as i32, s.height as i32, true);
        let r = self.content_rect();
        for t in &self.tabs {
            let show = t.id == self.active && !self.panel;
            place(t.id, r.x, r.y, r.width, r.height, show);
        }
    }

    fn activate(&mut self, id: usize) {
        if self.tabs.iter().any(|t| t.id == id) {
            let prev = self.active;
            if let Some(t) = self.tabs.iter_mut().find(|t| t.id == prev) {
                t.last_active = Instant::now();
            }
            let wake = self.tabs.iter().find(|t| t.id == id).filter(|t| t.frozen).map(|t| t.url.clone());
            if let Some(url) = wake {
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
                    t.frozen = false;
                    t.loading = true;
                }
                BROWSERS.with(|m| {
                    if let Some(f) = m.borrow().get(&id).and_then(|b| b.main_frame()) {
                        f.load_url(Some(&cs(&url)));
                    }
                });
            }
            if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
                t.last_active = Instant::now();
            }
            self.active = id;
            BROWSERS.with(|m| {
                for (bid, b) in m.borrow().iter() {
                    if *bid != UI_ID {
                        if let Some(h) = b.host() {
                            h.was_hidden((*bid != id) as i32);
                        }
                    }
                }
            });
            self.layout();
            if let Some(b) = BROWSERS.with(|m| m.borrow().get(&id).cloned()) {
                if let Some(h) = b.host() {
                    h.set_focus(1);
                }
            }
            self.push();
        }
    }

    fn close(&mut self, id: usize) {
        if let Some(t) = self.tabs.iter().find(|t| t.id == id && !t.private && t.url.starts_with("http")) {
            self.closed.push(t.url.clone());
            if self.closed.len() > 20 {
                self.closed.remove(0);
            }
        }
        if let Some(b) = BROWSERS.with(|m| m.borrow_mut().remove(&id)) {
            if let Some(h) = b.host() {
                h.close_browser(1);
            }
        }
        self.tabs.retain(|t| t.id != id);
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

    fn with_active<R>(&self, f: impl FnOnce(&Browser) -> R) -> Option<R> {
        BROWSERS.with(|m| m.borrow().get(&self.active).map(f))
    }

    fn set_browser_zoom(&self, id: usize, factor: f64) {
        BROWSERS.with(|b| {
            if let Some(h) = b.borrow().get(&id).and_then(|b| b.host()) {
                // CEF zoom levels are logarithmic: factor = 1.2 ^ level
                h.set_zoom_level(factor.ln() / 1.2f64.ln());
            }
        });
    }

    /// Applies the remembered zoom for the tab's site (Ghost tabs keep their own session-only level).
    fn apply_zoom(&mut self, id: usize) {
        let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) else { return };
        if t.private || t.frozen {
            return;
        }
        let want = self.cfg.zoom.get(host_of(&t.url)).copied().unwrap_or(1.0);
        if (t.zoom - want).abs() > 0.001 {
            t.zoom = want;
            self.set_browser_zoom(id, want);
        }
    }

    fn zoom_cmd(&mut self, dir: &str) {
        let Some(t) = self.tabs.iter_mut().find(|t| t.id == self.active) else { return };
        let next = config::zoom_step(t.zoom, dir);
        if (next - t.zoom).abs() < 0.001 {
            return;
        }
        t.zoom = next;
        let (id, private, host) = (t.id, t.private, host_of(&t.url).to_string());
        self.set_browser_zoom(id, next);
        if !private && !host.is_empty() {
            if (next - 1.0).abs() < 0.001 {
                self.cfg.zoom.remove(&host);
            } else {
                self.cfg.zoom.insert(host, next);
            }
            self.cfg.save();
        }
        self.push();
    }

    fn upd_send(&self) -> impl Fn(updater::UpdMsg) + Send + 'static {
        let p = self.tx.clone();
        move |m| {
            let _ = p.send_event(Ev::Update(m));
        }
    }

    fn push_update(&self) {
        let st = self.upd.state_json(self.cfg.update.auto, self.cfg.update.nightly);
        self.eval_ui(&format!("window.ui.updateState({st})"));
    }

    fn toast_act(&self, msg: &str, label: &str, cmd: &str) {
        self.eval_ui(&format!("window.ui.toast({},{{label:{},cmd:{}}})", Value::String(msg.into()), Value::String(label.into()), Value::String(cmd.into())));
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

    /// Refreshes what the imc:// pages show about the running browser.
    fn sync_internal(&self) {
        let mut d = self.internal.write().unwrap();
        d.engine = format!("{}.{}.{}.{}", sys::CHROME_VERSION_MAJOR, sys::CHROME_VERSION_MINOR, sys::CHROME_VERSION_BUILD, sys::CHROME_VERSION_PATCH);
        d.flags = std::env::args().skip(1).filter(|a| a.starts_with("--")).collect::<Vec<_>>().join("
");
        d.hw = self.cfg.hardware_acceleration;
        d.proxy = if self.cfg.vpn.enabled { format!("{} {}:{}{}", self.cfg.vpn.kind, self.cfg.vpn.host, self.cfg.vpn.port, if self.cfg.vpn.kill_switch { " (kill switch on)" } else { "" }) } else { "Direct connection (no proxy)".into() };
        d.gestures = self.cfg.gestures.clone();
        d.doh = self.cfg.doh.describe();
    }

    /// Certificate and connection details for the open page (the lock popup).
    fn site_info(&self) {
        let Some(t) = self.tabs.iter().find(|t| t.id == self.active) else { return };
        let url = t.url.clone();
        let web = url.starts_with("http://") || url.starts_with("https://");
        // Windows FILETIME-style base time (microseconds since 1601) -> unix seconds
        let unix = |b: Basetime| b.val / 1_000_000 - 11_644_473_600;
        let mut cert = Value::Null;
        if let Some(entry) = BROWSERS.with(|m| m.borrow().get(&t.id).and_then(|b| b.host()).and_then(|h| h.visible_navigation_entry())) {
            if let Some(ssl) = entry.sslstatus().filter(|s| s.is_secure_connection() != 0) {
                let version = match ssl.sslversion() {
                    v if v == SslVersion::from(sys::cef_ssl_version_t::SSL_CONNECTION_VERSION_TLS1_3) => "TLS 1.3",
                    v if v == SslVersion::from(sys::cef_ssl_version_t::SSL_CONNECTION_VERSION_TLS1_2) => "TLS 1.2",
                    v if v == SslVersion::from(sys::cef_ssl_version_t::SSL_CONNECTION_VERSION_QUIC) => "QUIC",
                    _ => "",
                };
                if let Some(x) = ssl.x509_certificate() {
                    let name = |p: Option<X509CertPrincipal>| p.map(|p| CefString::from(&p.display_name()).to_string()).unwrap_or_default();
                    cert = json!({"subject": name(x.subject()), "issuer": name(x.issuer()), "from": unix(x.valid_start()), "to": unix(x.valid_expiry()),
                                  "protocol": version, "cipher": "", "keyExchange": "", "error": Value::Null});
                }
            }
        }
        let info = json!({"url": url, "host": host_of(&url), "secure": url.starts_with("https://"), "local": !web, "internal": url.starts_with("imc://"), "cert": cert, "clearable": web});
        if std::env::var_os("IMC_SELFTEST").is_some() {
            eprintln!("SELFTEST siteinfo {info}");
        }
        self.eval_ui(&format!("window.ui.siteInfo({info})"));
    }

    /// Clears cookies, site data or the HTTP cache. Site data runs in the page, so it covers that page's origin.
    fn site_clear(&self, kind: &str) {
        let Some(t) = self.tabs.iter().find(|t| t.id == self.active) else { return };
        let site = adblock::registrable(host_of(&t.url)).to_string();
        let Some(browser) = BROWSERS.with(|m| m.borrow().get(&t.id).cloned()) else { return };
        let Some(host) = browser.host() else { return };
        let ctx = host.request_context();
        match kind {
            "cookies" => {
                let manager = ctx.and_then(|c| c.cookie_manager(None));
                let mut visitor = CookieSweep::new(site.clone(), Arc::new(AtomicUsize::new(0)), self.tx.clone());
                if !manager.is_some_and(|m| m.visit_all_cookies(Some(&mut visitor)) != 0) {
                    self.toast("Could not clear cookies.");
                }
            }
            "data" => {
                if let Some(f) = browser.main_frame() {
                    let js = "(async()=>{try{localStorage.clear();sessionStorage.clear()}catch(e){}\
                        try{for(const k of await caches.keys())await caches.delete(k)}catch(e){}\
                        try{for(const d of await indexedDB.databases())indexedDB.deleteDatabase(d.name)}catch(e){}\
                        try{for(const r of await navigator.serviceWorker.getRegistrations())await r.unregister()}catch(e){}\
                        location.reload()})()";
                    f.execute_java_script(Some(&cs(js)), Some(&cs("imc://clear")), 0);
                    self.toast(&format!("Cleared site data for {}. Reloading.", host_of(&t.url)));
                }
            }
            "cache" => {
                if let Some(c) = ctx {
                    let mut done = CacheDone::new(self.tx.clone());
                    c.clear_http_cache(Some(&mut done));
                }
            }
            _ => {}
        }
    }

    fn on_download(&mut self, e: DlEvent) {
        let now = updater::now_unix();
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
        self.eval_ui(&format!("window.ui.downloads({},{open})", Value::Array(list)));
    }

    fn push_cards(&self) {
        let list: Vec<Value> = cards::load()
            .iter()
            .enumerate()
            .map(|(i, c)| json!({"i": i, "label": c.label, "name": c.name, "brand": c.brand, "last4": c.last4, "month": c.exp_month, "year": c.exp_year}))
            .collect();
        let _ = self.eval_ui_ret(&format!("window.ui.cardsList({})", Value::Array(list)));
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
        self.eval_page(&js);
        self.toast(&format!("Filled {} ending {}. Enter the security code yourself.", card.brand, card.last4));
    }

    fn toast(&self, msg: &str) {
        let _ = self.eval_ui_ret(&format!("window.ui.toast({})", Value::String(msg.into())));
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
        let _ = self.eval_ui_ret(&format!("window.ui.creds({})", Value::Array(list)));
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
        self.eval_page(&js);
        self.toast("Filled.");
    }

    fn eval_ui_ret(&self, js: &str) {
        self.eval_ui(js);
    }

    fn eval_page(&self, js: &str) {
        self.with_active(|b| {
            if let Some(f) = b.main_frame() {
                f.execute_java_script(Some(&cs(js)), Some(&cs("imc://page")), 0);
            }
        });
    }

    /// Runs the action bound to a drawn gesture by feeding the same command the toolbar would send.
    fn gesture(&mut self, path: &str) -> bool {
        let Some(action) = self.cfg.gestures.action_for(path).map(String::from) else { return false };
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
            _ => return false,
        };
        self.ipc(&cmd.to_string())
    }

    fn eval_ui(&self, js: &str) {
        BROWSERS.with(|m| {
            if let Some(f) = m.borrow().get(&UI_ID).and_then(|b| b.main_frame()) {
                f.execute_java_script(Some(&cs(js)), Some(&cs("imc://ui")), 0);
            }
        });
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
        if !self.ui_ready {
            return;
        }
        let state = json!({
            "tabs": self.tabs.iter().map(|t| json!({"id": t.id, "title": t.title, "url": t.url, "frozen": t.frozen, "loading": t.loading, "audible": t.audible, "muted": t.muted, "private": t.private, "zoom": (t.zoom * 100.0).round()})).collect::<Vec<_>>(),
            "active": self.active,
            "bookmarks": self.bookmarks,
            "blocked": self.shield.blocked.load(Ordering::Relaxed),
            "adblock": self.shield.enabled.load(Ordering::Relaxed),
            "vpn": {"enabled": self.cfg.vpn.enabled, "down": self.cfg.vpn.enabled && vpn::TUNNEL_DOWN.load(Ordering::Relaxed)},
            "cfg": self.cfg,
            "extensions": [],
            "maximized": self.window.is_maximized(),
            "gesture_actions": config::GESTURE_ACTIONS.iter().map(|(id, l)| json!({"id": id, "label": l})).collect::<Vec<_>>(),
        });
        self.eval_ui(&format!("window.ui&&window.ui.set({state})"));
    }

    /// Returns true when the app should restart/quit.
    fn ipc(&mut self, body: &str) -> bool {
        let Ok(m) = serde_json::from_str::<Value>(body) else { return false };
        let cmd = m["cmd"].as_str().unwrap_or("");
        match cmd {
            "ready" => {
                self.ui_ready = true;
                self.push();
            }
            "new" => {
                let home = self.cfg.home.clone();
                let n = self.new_tab(&home, m["private"].as_bool().unwrap_or(false));
                self.activate(n);
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
            "go" => {
                let url = normalize(m["text"].as_str().unwrap_or(""), &self.cfg.search_engine);
                self.with_active(|b| {
                    if let Some(f) = b.main_frame() {
                        f.load_url(Some(&cs(&url)));
                    }
                });
            }
            "back" => {
                self.with_active(|b| b.go_back());
            }
            "forward" => {
                self.with_active(|b| b.go_forward());
            }
            "reload" => {
                self.with_active(|b| b.reload());
            }
            "panel" => {
                self.panel = m["open"].as_bool().unwrap_or(false);
                self.layout();
            }
            "engine" => {
                if let Some(e) = m["id"].as_str().filter(|e| ["google", "bing", "duckduckgo", "brave", "yahoo"].contains(e)) {
                    self.cfg.search_engine = e.into();
                    self.cfg.save();
                    self.push();
                }
            }
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
            "bookmark" => {
                if let Some(t) = self.tabs.iter().find(|t| t.id == self.active && !t.private) {
                    if let Some(i) = self.bookmarks.iter().position(|b| b.url == t.url) {
                        self.bookmarks.remove(i);
                    } else if t.url.starts_with("http") {
                        let title = if t.title.is_empty() { t.url.clone() } else { t.title.clone() };
                        self.bookmarks.push(bookmarks::Bookmark { title, url: t.url.clone(), folder: String::new() });
                    }
                    bookmarks::save(&self.bookmarks);
                    self.push();
                }
            }
            "toolbar" => {
                let list = |k: &str| m[k].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect::<Vec<_>>()).unwrap_or_default();
                self.cfg.toolbar = config::Toolbar { left: list("left"), right: list("right") };
                self.cfg.save();
                self.push();
            }
            "stop" => {
                self.with_active(|b| b.stop_load());
            }
            "print" => {
                self.with_active(|b| {
                    if let Some(f) = b.main_frame() {
                        f.execute_java_script(Some(&cs("window.print()")), Some(&cs("imc://print")), 0);
                    }
                });
            }
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
            "gestures" => {
                if let Ok(mut g) = serde_json::from_value::<config::Gestures>(m["gestures"].clone()) {
                    let mut seen = std::collections::HashSet::new();
                    g.binds.retain(|b| config::Gestures::clean_path(&b.path).is_some() && config::GESTURE_ACTIONS.iter().any(|(id, _)| *id == b.action) && seen.insert(b.path.clone()));
                    g.min_distance = g.min_distance.clamp(10, 120);
                    self.cfg.gestures = g;
                    self.cfg.save();
                    self.sync_internal();
                    let page = self.cfg.gestures.page_config();
                    *self.shield.gest.write().unwrap() = page.clone();
                    let js = format!("window.__imcGset&&window.__imcGset({page})");
                    BROWSERS.with(|b| {
                        for (id, br) in b.borrow().iter() {
                            if *id != UI_ID {
                                if let Some(f) = br.main_frame() {
                                    f.execute_java_script(Some(&cs(&js)), Some(&cs("imc://gestures")), 0);
                                }
                            }
                        }
                    });
                    self.push();
                }
            }
            "tab_next" | "tab_prev" => {
                if let Some(i) = self.tabs.iter().position(|t| t.id == self.active) {
                    let n = self.tabs.len();
                    let j = if m["cmd"] == "tab_next" { (i + 1) % n } else { (i + n - 1) % n };
                    let id = self.tabs[j].id;
                    self.activate(id);
                }
            }
            "focus_url" => {
                BROWSERS.with(|b| {
                    if let Some(h) = b.borrow().get(&UI_ID).and_then(|b| b.host()) {
                        h.set_focus(1);
                    }
                });
                self.eval_ui("document.getElementById('url').focus()");
            }
            "scroll_top" => self.eval_page("window.scrollTo({top:0,behavior:'smooth'})"),
            "scroll_bottom" => self.eval_page("window.scrollTo({top:document.documentElement.scrollHeight,behavior:'smooth'})"),
            "hwaccel" => {
                self.cfg.hardware_acceleration = m["enabled"].as_bool().unwrap_or(true);
                self.cfg.save();
                self.sync_internal();
            }
            "restart" => {
                self.relaunch();
                return true;
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
            "open_settings" => {
                let sec = m["section"].as_str().unwrap_or("general").to_string();
                self.eval_ui(&format!("panel(true);section({})", Value::String(sec)));
            }
            // Dev hook (only with IMC_SELFTEST): run script in the active page.
            "selftest_eval" if std::env::var_os("IMC_SELFTEST").is_some() => self.eval_page(m["js"].as_str().unwrap_or("")),
            "site_info" => self.site_info(),
            "site_clear" => self.site_clear(m["kind"].as_str().unwrap_or("")),
            "dl_list" => self.push_downloads(true),
            "dl_open" | "dl_show" | "dl_cancel" | "dl_remove" => {
                let id = m["id"].as_u64().unwrap_or(0);
                let Some(i) = self.downloads.iter().position(|d| d.id == id) else { return false };
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
                    "dl_cancel" => cancel_download(id),
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
                return true;
            }
            "adblock" => {
                let on = !self.shield.enabled.load(Ordering::Relaxed);
                self.shield.enabled.store(on, Ordering::Relaxed);
                self.cfg.adblock = on;
                self.cfg.save();
                self.push();
            }
            "mute" => {
                if let Some(t) = m["id"].as_u64().and_then(|id| self.tabs.iter_mut().find(|t| t.id == id as usize)) {
                    t.muted = !t.muted;
                    let (id, muted) = (t.id, t.muted);
                    BROWSERS.with(|b| {
                        if let Some(h) = b.borrow().get(&id).and_then(|b| b.host()) {
                            h.set_audio_muted(muted as i32);
                        }
                    });
                }
                self.push();
            }
            "bm_delete" => {
                if let Some(i) = m["i"].as_u64().map(|i| i as usize).filter(|&i| i < self.bookmarks.len()) {
                    self.bookmarks.remove(i);
                    bookmarks::save(&self.bookmarks);
                    self.push();
                }
            }
            // Still on the WebView2 build: blocker, mute, proxy settings, imports, passwords.
            // Privacy and proxy settings need a restart (they are browser-wide switches).
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
                    return true;
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
                    self.eval_ui(&format!("window.ui.revealed({},{})", m["i"], Value::String(pw)));
                }
            }
            "pw_fill" => self.fill_password(m["i"].as_u64().unwrap_or(u64::MAX) as usize),
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
            other => self.toast(&format!("'{other}' is not in the CEF build yet.")),
        }
        false
    }
}

/// Close every page, give CEF a moment to finish, then shut it down.
fn finish() {
    BROWSERS.with(|m| {
        for b in m.borrow().values() {
            if let Some(h) = b.host() {
                h.close_browser(1);
            }
        }
    });
    let end = Instant::now() + Duration::from_millis(600);
    while Instant::now() < end {
        do_message_loop_work();
        std::thread::sleep(Duration::from_millis(10));
    }
    BROWSERS.with(|m| m.borrow_mut().clear());
    shutdown();
}

/// Command-line tools (run instead of starting the browser): `imc-browse --help` lists them.
fn cli(args: &[String]) -> bool {
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    let Some(first) = args.first() else { return false };
    if !["--update-lists", "--version", "--help"].contains(&first.as_str()) {
        return false; // anything else is a Chromium switch or a page to open
    }
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
    match first.as_str() {
        "--version" => println!("IMC Browse {} (build {})", updater::VERSION, updater::BUILD_ID),
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
                let ok = std::process::Command::new("curl.exe").args(["-fsSL", "-o"]).arg(&out).arg(url).status().is_ok_and(|s| s.success());
                println!("{} {name}", if ok { "updated" } else { "FAILED " });
            }
        }
        _ => println!("imc-browse [URL...]
  --update-lists   download the ad and tracker filter lists
  --version"),
    }
    true
}

fn main() {
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let args = args::Args::new();

    // Renderer/GPU/utility subprocesses are this same exe: they run here and return.
    let cfg = Config::load();
    let mut app = ImcApp::new(cfg.vpn.clone(), cfg.gestures.page_config(), cfg.hardware_acceleration);
    let code = execute_process(Some(args.as_main_args()), Some(&mut app), std::ptr::null_mut());
    if code >= 0 {
        std::process::exit(code);
    }

    // From here on this is the browser process (not a renderer/GPU/utility child).
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if cli(&argv) {
        return;
    }
    // Everything the browser spawns dies with it, and cannot break away.
    sandbox::contain_process_tree();
    updater::cleanup();
    // Secure DNS lives in the engine's Local State file; set it before the engine starts.
    config::write_secure_dns(&config::root().join("cef-profile"), &cfg.doh);
    vpn::start_watchdog(cfg.vpn.clone());
    let cache = cs(&config::root().join("cef-profile").to_string_lossy());
    let settings = Settings { no_sandbox: 1, root_cache_path: cache.clone(), cache_path: cache, ..Default::default() };
    assert_eq!(initialize(Some(args.as_main_args()), Some(&settings), Some(&mut app), std::ptr::null_mut()), 1);

    let event_loop = EventLoopBuilder::<Ev>::with_user_event().build();
    let tx = event_loop.create_proxy();
    let window = WindowBuilder::new()
        .with_title("IMC Browse (CEF)")
        .with_decorations(false)
        .with_inner_size(LogicalSize::new(1280.0, 800.0))
        .build(&event_loop)
        .expect("window");

    // The chrome UI is a local file so it can be a normal page.
    let ui_dir = config::root().join("ui");
    let _ = std::fs::create_dir_all(&ui_dir);
    let ui_file = ui_dir.join("chrome.html");
    let _ = std::fs::write(&ui_file, CHROME_HTML);
    let ui_url = format!("file:///{}", ui_file.to_string_lossy().replace('\\', "/"));

    let size = window.inner_size();
    let info = WindowInfo { runtime_style: RuntimeStyle::ALLOY, ..Default::default() }.set_as_child(
        sys::HWND(window.hwnd() as *mut _),
        &Rect { x: 0, y: 0, width: size.width as i32, height: size.height as i32 },
    );
    let mut ui_client = Cl::new(UI_ID, tx.clone(), None, false, Arc::new(AtomicU64::new(0)));
    let ui = browser_host_create_browser_sync(Some(&info), Some(&mut ui_client), Some(&cs(&ui_url)), Some(&BrowserSettings::default()), None, None)
        .expect("chrome ui");
    BROWSERS.with(|m| m.borrow_mut().insert(UI_ID, ui));

    let engine = load_engine();
    let css = engine.cosmetic_css();
    let css_js = if css.is_empty() {
        String::new()
    } else {
        format!(
            "(()=>{{const s=document.createElement('style');s.textContent={};(document.head||document.documentElement).appendChild(s)}})();",
            Value::String(css)
        )
    };
    let shield = Arc::new(Shield { engine, css_js, enabled: AtomicBool::new(cfg.adblock), blocked: AtomicUsize::new(0), kill_switch: cfg.vpn.enabled && cfg.vpn.kill_switch, gest: std::sync::RwLock::new(cfg.gestures.page_config()) });
    let internal = Arc::new(RwLock::new(internal::Internal { engine: String::new(), flags: String::new(), hw: true, proxy: String::new(), doh: String::new(), gestures: cfg.gestures.clone() }));
    {
        let mut factory = ImcSchemes::new(internal.clone(), shield.clone());
        register_scheme_handler_factory(Some(&cs("imc")), None, Some(&mut factory));
    }
    let downloads = downloads::load();
    let mut state = Shell {
        window,
        tx: tx.clone(),
        tabs: vec![],
        active: 0,
        next: 1,
        cfg: cfg.clone(),
        panel: false,
        bookmarks: bookmarks::load(),
        ui_ready: false,
        shield,
        ghost_ctx: None,
        closed: vec![],
        upd: updater::Ctl::new(),
        internal,
        dl_ids: Arc::new(AtomicU64::new(downloads.iter().map(|d| d.id).max().unwrap_or(0) + 1)),
        downloads,
        last_dl_push: Instant::now(),
    };
    let mut urls: Vec<String> = std::env::args().skip(1).filter(|a| !a.starts_with("--")).map(|a| normalize(&a, &cfg.search_engine)).collect();
    let session = config::root().join("session.json");
    if urls.is_empty() {
        if let Some(saved) = std::fs::read_to_string(&session).ok().and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok()) {
            urls = saved;
        }
    }
    let _ = std::fs::remove_file(&session);
    let urls = if urls.is_empty() { vec![cfg.home.clone()] } else { urls };
    let mut first = 0;
    for u in &urls {
        let id = state.new_tab(u, false);
        if first == 0 {
            first = id;
        }
    }
    state.sync_internal();
    state.activate(first);
    let p_tick = tx.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(3));
        if p_tick.send_event(Ev::Tick).is_err() {
            break;
        }
    });

    // Dev hook: scripted UI commands so behaviour can be checked without touching the screen.
    if std::env::var_os("IMC_SELFTEST").is_some() {
        let t = tx.clone();
        std::thread::spawn(move || {
            let step = |ms: u64, cmd: &str| {
                std::thread::sleep(Duration::from_millis(ms));
                let _ = t.send_event(Ev::Ipc(cmd.to_string()));
            };
            step(3000, r#"{"cmd":"new","private":true}"#);
            step(3000, r#"{"cmd":"new"}"#);
            step(16000, r#"{"cmd":"switch","id":1}"#);
            step(4000, r#"{"cmd":"close","id":3}"#);
            step(7000, r#"{"cmd":"close","id":2}"#);
        });
    }

    if std::env::var_os("IMC_SELFTEST_SITE").is_some() {
        let t3 = tx.clone();
        std::thread::spawn(move || {
            let send = |ms: u64, v: Value| {
                std::thread::sleep(Duration::from_millis(ms));
                let _ = t3.send_event(Ev::Ipc(v.to_string()));
            };
            send(6000, json!({"cmd": "selftest_eval", "js": "document.cookie='imc_test=1; path=/'; document.cookie='imc_test2=2; path=/'"}));
            send(1500, json!({"cmd": "site_info"}));
            let which = std::env::var("IMC_SELFTEST_SITE").unwrap_or_default();
            for kind in ["cookies", "data", "cache"] {
                if which == "all" || which == kind {
                    send(1500, json!({"cmd": "site_clear", "kind": kind}));
                }
            }
        });
    }
    if let Ok(what) = std::env::var("IMC_SELFTEST_CARD") {
        let t = tx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(6));
            let msg = if what == "add" {
                json!({"cmd": "card_add", "label": "", "name": "Test Person", "number": "4111 1111 1111 1111", "month": 12, "year": 2031})
            } else {
                json!({"cmd": "card_delete", "i": 0})
            };
            let _ = t.send_event(Ev::Ipc(msg.to_string()));
        });
    }
    event_loop.run(move |event, _, flow| {
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                state.save_session();
                finish();
                *flow = ControlFlow::Exit;
                return;
            }
            Event::WindowEvent { event: WindowEvent::Resized(_), .. } => {
                state.layout();
                state.push();
            }
            Event::UserEvent(ev) => match ev {
                Ev::Ipc(s) => {
                    if state.ipc(&s) {
                        finish();
                        *flow = ControlFlow::Exit;
                        return;
                    }
                }
                Ev::Url(id, u) => {
                    if let Some(t) = state.tabs.iter_mut().find(|t| t.id == id && !t.frozen) {
                        t.url = if u.starts_with("imc://") { u.trim_end_matches('/').to_string() } else { u };
                    }
                    state.apply_zoom(id);
                    state.push();
                }
                Ev::Title(id, title) => {
                    if let Some(t) = state.tabs.iter_mut().find(|t| t.id == id && !t.frozen) {
                        t.title = title;
                    }
                    state.push();
                }
                Ev::Load(id, loading) => {
                    if let Some(t) = state.tabs.iter_mut().find(|t| t.id == id) {
                        t.loading = loading;
                    }
                    state.push();
                }
                Ev::Audio(id, on) => {
                    if let Some(t) = state.tabs.iter_mut().find(|t| t.id == id) {
                        t.audible = on;
                    }
                    state.push();
                }
                Ev::Tick => {
                    state.tick();
                    if (state.cfg.update.auto || state.cfg.update.nightly) && state.upd.due(state.cfg.update.last_check) {
                        state.check_updates(false);
                    }
                }
                Ev::Zoom(d) => state.zoom_cmd(&d),
                Ev::Update(m) => state.on_update(m),
                Ev::Download(e) => state.on_download(e),
                Ev::Toast(m) => {
                    if std::env::var_os("IMC_SELFTEST").is_some() {
                        eprintln!("SELFTEST toast {m}");
                    }
                    state.toast(&m)
                }
                Ev::Gesture(path) => {
                    if state.gesture(&path) {
                        finish();
                        *flow = ControlFlow::Exit;
                        return;
                    }
                }
                // The Ghost profile can only go once its last page has really finished closing.
                Ev::Closed => {
                    if !state.tabs.iter().any(|t| t.private) {
                        state.ghost_ctx = None;
                    }
                }
                Ev::Open(u) => {
                    let private = state.tabs.iter().any(|t| t.id == state.active && t.private);
                    let n = state.new_tab(&u, private);
                    state.activate(n);
                }
            },
            _ => {}
        }
        // Pump CEF about 100 times a second (external message pump is a later optimisation).
        do_message_loop_work();
        *flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(10));
    });
}
