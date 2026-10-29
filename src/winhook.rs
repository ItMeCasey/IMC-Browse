//! Direct WebView2 (COM) hooks that wry does not expose:
//! per-request network filtering, tab suspension, and memory throttling.

use crate::adblock::{self, Engine, Request};
use crate::vpn::TUNNEL_DOWN;
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
};
use webview2_com::{Microsoft::Web::WebView2::Win32::*, *};
use windows::core::{HSTRING, Interface, PWSTR};
use wry::{WebView, WebViewExtWindows};

pub struct Shield {
    pub engine: Arc<RwLock<Arc<Engine>>>,
    pub enabled: AtomicBool,
    pub kill_switch: AtomicBool,
    pub blocked: AtomicUsize,
}

fn kind_of(ctx: COREWEBVIEW2_WEB_RESOURCE_CONTEXT) -> u16 {
    match ctx {
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT => adblock::T_SCRIPT,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE => adblock::T_IMAGE,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET => adblock::T_STYLE,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST
        | COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH
        | COREWEBVIEW2_WEB_RESOURCE_CONTEXT_EVENT_SOURCE
        | COREWEBVIEW2_WEB_RESOURCE_CONTEXT_WEBSOCKET => adblock::T_XHR,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA => adblock::T_MEDIA,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT => adblock::T_FONT,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING | COREWEBVIEW2_WEB_RESOURCE_CONTEXT_CSP_VIOLATION_REPORT => {
            adblock::T_PING
        }
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT => adblock::T_DOC,
        _ => adblock::T_OTHER,
    }
}

fn host_of(url: &str) -> &str {
    url.split("://").nth(1).and_then(|r| r.split(['/', '?', '#', ':']).next()).unwrap_or("")
}

fn is_internal(host: &str) -> bool {
    host == "localhost" || host.ends_with(".localhost") || host == "127.0.0.1" || host == "[::1]"
}

/// Intercept every request of this webview on the UI thread (microsecond-scale lookups).
pub fn install_shield(wv: &WebView, shield: Arc<Shield>) -> windows::core::Result<()> {
    unsafe {
        let core = wv.controller().CoreWebView2()?;
        let env = core.cast::<ICoreWebView2_2>()?.Environment()?;
        core.AddWebResourceRequestedFilter(&HSTRING::from("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)?;
        let mut token = Default::default();
        core.add_WebResourceRequested(
            &WebResourceRequestedEventHandler::create(Box::new(move |sender, args| {
                let (Some(sender), Some(args)) = (sender, args) else { return Ok(()) };
                let req = args.Request()?;
                let mut p = PWSTR::null();
                req.Uri(&mut p)?;
                let uri = take_pwstr(p);
                let host = host_of(&uri);
                // imc:// pages are private to the browser: other sites may not load or read them.
                if host == crate::internal::HOST {
                    let mut ctx = COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL;
                    args.ResourceContext(&mut ctx)?;
                    if ctx != COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT {
                        let mut s = PWSTR::null();
                        sender.Source(&mut s)?;
                        if host_of(&take_pwstr(s)) != crate::internal::HOST {
                            let resp = env.CreateWebResourceResponse(None, 403, &HSTRING::from("Blocked"), &HSTRING::from(""))?;
                            args.SetResponse(&resp)?;
                        }
                    }
                    return Ok(());
                }
                if is_internal(host) || !(uri.starts_with("http://") || uri.starts_with("https://") || uri.starts_with("ws")) {
                    return Ok(());
                }

                let mut block = shield.kill_switch.load(Ordering::Relaxed) && TUNNEL_DOWN.load(Ordering::Relaxed);
                if !block && shield.enabled.load(Ordering::Relaxed) {
                    let mut ctx = COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL;
                    args.ResourceContext(&mut ctx)?;
                    let kind = kind_of(ctx);
                    if kind != adblock::T_DOC {
                        let mut s = PWSTR::null();
                        sender.Source(&mut s)?;
                        let page = take_pwstr(s);
                        let engine = shield.engine.read().unwrap().clone();
                        block = engine.should_block(&Request { url: &uri, source_host: host_of(&page), kind });
                        if block {
                            shield.blocked.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
                if block {
                    let resp = env.CreateWebResourceResponse(None, 403, &HSTRING::from("Blocked"), &HSTRING::from(""))?;
                    args.SetResponse(&resp)?;
                }
                Ok(())
            })),
            &mut token,
        )?;
    }
    Ok(())
}

/// Calls `on(is_playing)` whenever the page starts or stops producing sound.
pub fn watch_audio(wv: &WebView, on: impl Fn(bool) + 'static) -> windows::core::Result<()> {
    unsafe {
        let c8 = wv.controller().CoreWebView2()?.cast::<ICoreWebView2_8>()?;
        let mut token = Default::default();
        c8.add_IsDocumentPlayingAudioChanged(
            &IsDocumentPlayingAudioChangedEventHandler::create(Box::new(move |sender, _| {
                if let Some(Ok(c8)) = sender.map(|s| s.cast::<ICoreWebView2_8>()) {
                    let mut playing = windows::core::BOOL(0);
                    c8.IsDocumentPlayingAudio(&mut playing)?;
                    on(playing.as_bool());
                }
                Ok(())
            })),
            &mut token,
        )?;
    }
    Ok(())
}

pub fn set_muted(wv: &WebView, muted: bool) {
    unsafe {
        if let Ok(c8) = wv.controller().CoreWebView2().and_then(|c| c.cast::<ICoreWebView2_8>()) {
            let _ = c8.SetIsMuted(muted);
        }
    }
}

/// Hibernate: stops script timers/rendering and lets Chromium discard the renderer's working set.
pub fn suspend(wv: &WebView) {
    unsafe {
        let _ = wv.set_visible(false);
        let _ = wv.set_memory_usage_level(wry::MemoryUsageLevel::Low);
        if let Ok(core) = wv.controller().CoreWebView2() {
            if let Ok(c3) = core.cast::<ICoreWebView2_3>() {
                let done = TrySuspendCompletedHandler::create(Box::new(|_, _| Ok(())));
                let _ = c3.TrySuspend(&done);
            }
        }
    }
}

pub fn resume(wv: &WebView) {
    unsafe {
        if let Ok(core) = wv.controller().CoreWebView2() {
            if let Ok(c3) = core.cast::<ICoreWebView2_3>() {
                let _ = c3.Resume();
            }
        }
    }
    let _ = wv.set_memory_usage_level(wry::MemoryUsageLevel::Normal);
    let _ = wv.set_visible(true);
}

/// Hidden but not suspended (cheap tab switch); drops memory target.
pub fn background(wv: &WebView) {
    let _ = wv.set_visible(false);
    let _ = wv.set_memory_usage_level(wry::MemoryUsageLevel::Low);
}

/// Asks WebView2 for the ids it gave the installed extensions (needed to open chrome-extension:// pages).
pub fn extension_ids(wv: &WebView, done: impl Fn(Vec<(String, String)>) + 'static) -> windows::core::Result<()> {
    unsafe {
        let core = wv.controller().CoreWebView2()?;
        let profile = core.cast::<ICoreWebView2_13>()?.Profile()?;
        let p7 = profile.cast::<ICoreWebView2Profile7>()?;
        let handler = ProfileGetBrowserExtensionsCompletedHandler::create(Box::new(move |hr, list| {
            let mut out = vec![];
            if hr.is_ok() {
                if let Some(list) = list {
                    let mut n = 0u32;
                    list.Count(&mut n)?;
                    for i in 0..n {
                        let e = list.GetValueAtIndex(i)?;
                        let (mut id, mut name) = (PWSTR::null(), PWSTR::null());
                        e.Id(&mut id)?;
                        e.Name(&mut name)?;
                        out.push((take_pwstr(id), take_pwstr(name)));
                    }
                }
            }
            done(out);
            Ok(())
        }));
        p7.GetBrowserExtensions(&handler)?;
    }
    Ok(())
}

/// Runs one DevTools-protocol command on the page; `done` gets the result JSON ("" on failure).
pub fn cdp(wv: &WebView, method: &str, params: &str, done: impl Fn(String) + 'static) -> windows::core::Result<()> {
    unsafe { cdp_core(&wv.controller().CoreWebView2()?, method, params, done) }
}

unsafe fn cdp_core(core: &ICoreWebView2, method: &str, params: &str, done: impl Fn(String) + 'static) -> windows::core::Result<()> {
    let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |hr: windows::core::Result<()>, json: String| {
        done(if hr.is_ok() { json } else { String::new() });
        Ok(())
    }));
    unsafe { core.CallDevToolsProtocolMethod(&HSTRING::from(method), &HSTRING::from(params), &handler) }
}

/// Reports the page's certificate and connection details (Security.visibleSecurityStateChanged) as JSON.
pub fn watch_security(wv: &WebView, on: impl Fn(String) + 'static) -> windows::core::Result<()> {
    unsafe {
        let core = wv.controller().CoreWebView2()?;
        let recv = core.GetDevToolsProtocolEventReceiver(&HSTRING::from("Security.visibleSecurityStateChanged"))?;
        let mut token = Default::default();
        recv.add_DevToolsProtocolEventReceived(
            &DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_, args| {
                if let Some(a) = args {
                    let mut p = PWSTR::null();
                    a.ParameterObjectAsJson(&mut p)?;
                    on(take_pwstr(p));
                }
                Ok(())
            })),
            &mut token,
        )?;
        cdp_core(&core, "Security.enable", "{}", |_| {})
    }
}

/// Deletes every cookie whose domain belongs to `site` (the registrable domain, so subdomains go too).
pub fn clear_cookies(wv: &WebView, site: String, done: impl Fn(usize) + 'static) -> windows::core::Result<()> {
    unsafe {
        let core = wv.controller().CoreWebView2()?;
        let core2 = core.clone();
        cdp_core(&core, "Network.getAllCookies", "{}", move |json| {
            let v: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
            let mut n = 0;
            for c in v["cookies"].as_array().into_iter().flatten() {
                let d = c["domain"].as_str().unwrap_or("").trim_start_matches('.');
                if d == site || d.ends_with(&format!(".{site}")) {
                    let p = serde_json::json!({"name": c["name"], "domain": c["domain"], "path": c["path"]}).to_string();
                    let _ = cdp_core(&core2, "Network.deleteCookies", &p, |_| {});
                    n += 1;
                }
            }
            done(n);
        })
    }
}

/// The host owns zoom (per-site memory, indicator), so WebView2's own Ctrl+wheel zoom must not also fire.
pub fn disable_native_zoom(wv: &WebView) {
    unsafe {
        if let Ok(settings) = wv.controller().CoreWebView2().and_then(|c| c.Settings()) {
            let _ = settings.SetIsZoomControlEnabled(false);
        }
    }
}

/// Download progress, as reported to the app.
pub enum DlEvent {
    Started { id: u64, name: String, path: String, url: String, total: i64, private: bool },
    Progress { id: u64, received: i64, total: i64 },
    State { id: u64, state: &'static str },
}

thread_local! {
    /// Running downloads, kept so they can be cancelled.
    static DL_OPS: RefCell<HashMap<u64, ICoreWebView2DownloadOperation>> = RefCell::new(HashMap::new());
}

pub fn cancel_download(id: u64) {
    DL_OPS.with(|m| {
        if let Some(op) = m.borrow().get(&id) {
            let _ = unsafe { op.Cancel() };
        }
    });
}

/// Takes over this page's downloads: they go to `dir` (the system Downloads folder) without
/// the engine's own dialog, and progress is reported through `on`.
pub fn watch_downloads(wv: &WebView, ids: Arc<AtomicU64>, dir: std::path::PathBuf, private: bool, on: impl Fn(DlEvent) + Clone + 'static) -> windows::core::Result<()> {
    unsafe {
        let c4 = wv.controller().CoreWebView2()?.cast::<ICoreWebView2_4>()?;
        let mut token = Default::default();
        c4.add_DownloadStarting(
            &DownloadStartingEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let op = args.DownloadOperation()?;
                let mut p = PWSTR::null();
                args.ResultFilePath(&mut p)?;
                let name = crate::downloads::sanitize_name(&take_pwstr(p));
                let path = crate::downloads::unique_path(&dir, &name);
                args.SetResultFilePath(&HSTRING::from(path.as_os_str()))?;
                args.SetHandled(true)?; // no engine dialog: our download list takes over
                let id = ids.fetch_add(1, Ordering::Relaxed);
                let mut u = PWSTR::null();
                op.Uri(&mut u)?;
                let mut total = 0i64;
                op.TotalBytesToReceive(&mut total)?;
                on(DlEvent::Started {
                    id,
                    name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(name),
                    path: path.display().to_string(),
                    url: take_pwstr(u),
                    total,
                    private,
                });
                DL_OPS.with(|m| m.borrow_mut().insert(id, op.clone()));

                let (on1, mut t1) = (on.clone(), Default::default());
                op.add_BytesReceivedChanged(
                    &BytesReceivedChangedEventHandler::create(Box::new(move |o, _| {
                        if let Some(o) = o {
                            let (mut r, mut t) = (0i64, 0i64);
                            o.BytesReceived(&mut r)?;
                            o.TotalBytesToReceive(&mut t)?;
                            on1(DlEvent::Progress { id, received: r, total: t });
                        }
                        Ok(())
                    })),
                    &mut t1,
                )?;
                let (on2, mut t2) = (on.clone(), Default::default());
                op.add_StateChanged(
                    &StateChangedEventHandler::create(Box::new(move |o, _| {
                        let Some(o) = o else { return Ok(()) };
                        let mut st = COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS;
                        o.State(&mut st)?;
                        let state = if st == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED {
                            "done"
                        } else if st == COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED {
                            let mut why = COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE;
                            o.InterruptReason(&mut why)?;
                            if why == COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED { "canceled" } else { "failed" }
                        } else {
                            return Ok(());
                        };
                        DL_OPS.with(|m| m.borrow_mut().remove(&id));
                        on2(DlEvent::State { id, state });
                        Ok(())
                    })),
                    &mut t2,
                )?;
                Ok(())
            })),
            &mut token,
        )?;
    }
    Ok(())
}

/// Status bubble with the link URL while hovering a link (WebView2's built-in one).
pub fn enable_status_bar(wv: &WebView) {
    unsafe {
        if let Ok(settings) = wv.controller().CoreWebView2().and_then(|c| c.Settings()) {
            let _ = settings.SetIsStatusBarEnabled(true);
        }
    }
}
