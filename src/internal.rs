//! imc:// internal pages (version, GPU, extensions, bookmarks, blocker, shortcuts).
//!
//! Served by a CEF scheme handler. `chrome://`, `edge://` and `about:` for the same pages are mapped
//! here too, since the engine's own internal pages are not part of this browser. Everything is generated on request from
//! live data, styled with one shared stylesheet (dark and light follow the system).

use crate::{bookmarks, config, ext};

/// Snapshot the app keeps current for the pages.
/// Blocker numbers the pages show, read from the running blocker.
pub struct Stats {
    pub enabled: bool,
    pub blocked: usize,
    pub rules: usize,
    pub cosmetic: usize,
}

pub struct Internal {
    /// Chromium version of the engine.
    pub engine: String,
    pub flags: String,
    pub hw: bool,
    pub proxy: String,
    pub doh: String,
    pub gestures: config::Gestures,
    pub ai_site: String,
    pub control_on: bool,
    pub allow_js: bool,
}

const PAGES: &[(&str, &str, &str)] = &[
    ("version", "Version", "Browser, engine and where your data lives"),
    ("gpu", "Graphics", "Hardware acceleration and GPU flags"),
    ("extensions", "Extensions", "What is installed and how to open it"),
    ("history", "History", "Pages you visited, searchable"),
    ("bookmarks", "Bookmarks", "Everything you have saved, searchable"),
    ("blocker", "Blocker", "Ad and tracker filtering, rules and lists"),
    ("shortcuts", "Shortcuts", "Keyboard shortcuts and mouse gestures"),
    ("ai", "AI and control", "The AI panel and command-line control"),
];

const ICON_HOME: &str = include_str!("ui/glyphs/home.svg");
const ICON_INFO: &str = include_str!("ui/glyphs/info.svg");
const ICON_GPU: &str = include_str!("ui/glyphs/code.svg");
const ICON_EXT: &str = include_str!("ui/glyphs/extension.svg");
const ICON_HIST: &str = include_str!("ui/glyphs/history.svg");
const ICON_BM: &str = include_str!("ui/glyphs/bookmarks.svg");
const ICON_SHIELD: &str = include_str!("ui/glyphs/shield-ok.svg");
const ICON_KEYS: &str = include_str!("ui/glyphs/grip.svg");
const ICON_AI: &str = include_str!("ui/glyphs/ai.svg");
const ICON_SET: &str = include_str!("ui/glyphs/sliders.svg");

fn icon(name: &str) -> &'static str {
    match name {
        "" => ICON_HOME,
        "ai" => ICON_AI,
        "version" => ICON_INFO,
        "gpu" => ICON_GPU,
        "extensions" => ICON_EXT,
        "bookmarks" => ICON_BM,
        "history" => ICON_HIST,
        "blocker" => ICON_SHIELD,
        "shortcuts" => ICON_KEYS,
        _ => ICON_SET,
    }
}

pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

/// What the user typed -> the real URL, for `imc://x`, `chrome://x`, `edge://x`, `about:x`.
/// Only pages that exist here (plus settings) are mapped; anything else is left alone.
pub fn from_typed(text: &str) -> Option<String> {
    let t = text.trim().to_ascii_lowercase();
    let rest = ["imc://", "chrome://", "edge://", "about:"].iter().find_map(|p| t.strip_prefix(p))?;
    let rest = rest.trim_matches('/');
    let name = rest.split('/').next().unwrap_or("");
    let known = name.is_empty() || name == "settings" || name == "about" || PAGES.iter().any(|(n, _, _)| *n == name) || matches!(name, "extension" | "bookmark" | "flags" | "components" | "credits");
    if !known {
        return None;
    }
    let name = match name {
        "about" | "credits" => "",
        "extension" => "extensions",
        "bookmark" => "bookmarks",
        "flags" | "components" => "gpu",
        n => n,
    };
    let sub = rest.split_once('/').map(|(_, s)| s).unwrap_or("");
    Some(if sub.is_empty() { format!("imc://{name}") } else { format!("imc://{name}/{sub}") })
}

/// `imc://settings[/section]` -> the settings section to open (General when none or unknown).
pub fn settings_section(url: &str) -> Option<String> {
    let r = url.strip_prefix("imc://settings")?;
    let sec = r.trim_matches('/');
    let ok = ["general", "theme", "bookmarks", "passwords", "cards", "privacy", "toolbar", "gestures", "updates"];
    Some(if ok.contains(&sec) { sec.to_string() } else { "general".to_string() })
}

/// The page name in an `imc://name/...` URL ("" for the index).
pub fn page_name(url: &str) -> String {
    url.strip_prefix("imc://").unwrap_or("").split(['/', '?', '#']).next().unwrap_or("").to_ascii_lowercase()
}

const CSS: &str = r#"
:root{color-scheme:dark light;--bg:#101216;--panel:#171a20;--panel2:#1e222a;--line:#2a2f39;--fg:#e8eaf0;--dim:#9aa3b2;--acc:#4da3ff;--acc2:#8b7bff;--ok:#3ecf8e;--bad:#ff6b6b;--warn:#f5b84b;--shadow:0 10px 30px #0006}
@media (prefers-color-scheme:light){:root{--bg:#f3f5f9;--panel:#fff;--panel2:#f0f2f7;--line:#dde1ea;--fg:#1b1e25;--dim:#5d6676;--acc:#1a73e8;--acc2:#6b4de6;--ok:#12a46a;--bad:#d93f3f;--warn:#b8780c;--shadow:0 10px 30px #1b1e2514}}
*{box-sizing:border-box}
html,body{margin:0;background:var(--bg);color:var(--fg);font:14px/1.5 system-ui,"Segoe UI",sans-serif}
a{color:var(--acc);text-decoration:none}a:hover{text-decoration:underline}
svg{width:20px;height:20px;display:block;flex:0 0 auto}
.app{display:flex;min-height:100vh}
nav{width:236px;flex:0 0 auto;padding:22px 14px;border-right:1px solid var(--line);background:var(--panel);position:sticky;top:0;height:100vh;display:flex;flex-direction:column;gap:2px}
nav .brand{display:flex;align-items:center;gap:10px;font-weight:700;font-size:16px;padding:4px 10px 18px}
nav .brand i{width:30px;height:30px;border-radius:9px;background:linear-gradient(135deg,var(--acc),var(--acc2));display:flex;align-items:center;justify-content:center;color:#fff;font-style:normal;font-weight:800;font-size:13px}
nav a{display:flex;align-items:center;gap:12px;padding:9px 12px;border-radius:10px;color:var(--dim);font-weight:500}
nav a:hover{background:var(--panel2);color:var(--fg);text-decoration:none}
nav a.on{background:color-mix(in srgb,var(--acc),transparent 86%);color:var(--acc)}
nav .sp{flex:1}
main{flex:1;min-width:0;padding:34px 44px 60px;max-width:1080px}
h1{margin:0 0 4px;font-size:28px;letter-spacing:-.4px}
.sub{color:var(--dim);margin:0 0 26px}
h2{font-size:12px;text-transform:uppercase;letter-spacing:1.2px;color:var(--dim);margin:30px 0 10px;font-weight:700}
.hero{border-radius:20px;padding:34px 34px;margin-bottom:26px;background:linear-gradient(135deg,color-mix(in srgb,var(--acc),transparent 78%),color-mix(in srgb,var(--acc2),transparent 82%)),var(--panel);border:1px solid var(--line);box-shadow:var(--shadow)}
.hero h1{font-size:38px}
.hero .grad{background:linear-gradient(90deg,var(--acc),var(--acc2));-webkit-background-clip:text;background-clip:text;color:transparent}
.grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(250px,1fr));gap:14px}
.card{background:var(--panel);border:1px solid var(--line);border-radius:16px;padding:18px;display:flex;flex-direction:column;gap:8px;color:inherit;transition:transform .12s,border-color .12s,box-shadow .12s}
a.card:hover{transform:translateY(-2px);border-color:var(--acc);box-shadow:var(--shadow);text-decoration:none}
.card .ic{width:38px;height:38px;border-radius:11px;background:color-mix(in srgb,var(--acc),transparent 84%);color:var(--acc);display:flex;align-items:center;justify-content:center}
.card b{font-size:15px}.card span.d{color:var(--dim);font-size:13px}
.card .stat{margin-top:auto;font-size:12px;color:var(--acc);font-weight:600}
.stats{display:grid;grid-template-columns:repeat(auto-fill,minmax(190px,1fr));gap:14px;margin-bottom:8px}
.stat-card{background:var(--panel);border:1px solid var(--line);border-radius:16px;padding:16px 18px}
.stat-card .n{font-size:28px;font-weight:700;letter-spacing:-.5px}.stat-card .l{color:var(--dim);font-size:12px;text-transform:uppercase;letter-spacing:.8px}
table{width:100%;border-collapse:separate;border-spacing:0;background:var(--panel);border:1px solid var(--line);border-radius:14px;overflow:hidden}
th,td{text-align:left;padding:11px 16px;border-bottom:1px solid var(--line);vertical-align:top}
tr:last-child td{border-bottom:0}th{color:var(--dim);font-size:12px;font-weight:600;text-transform:uppercase;letter-spacing:.7px;background:var(--panel2)}
td.k{color:var(--dim);width:210px;white-space:nowrap}
.mono,code,kbd{font-family:Consolas,"Cascadia Mono",monospace}
code{background:var(--panel2);padding:2px 7px;border-radius:6px;font-size:12.5px;word-break:break-all}
pre{background:var(--panel);border:1px solid var(--line);border-radius:14px;padding:16px;white-space:pre-wrap;word-break:break-all;color:var(--dim);font-size:12.5px;margin:0}
kbd{background:var(--panel2);border:1px solid var(--line);border-bottom-width:2px;border-radius:7px;padding:2px 8px;font-size:12px}
.pill{display:inline-flex;align-items:center;gap:6px;padding:3px 11px;border-radius:999px;font-weight:600;font-size:12px;background:color-mix(in srgb,var(--dim),transparent 82%);color:var(--dim)}
.pill.ok{background:color-mix(in srgb,var(--ok),transparent 82%);color:var(--ok)}.pill.bad{background:color-mix(in srgb,var(--bad),transparent 82%);color:var(--bad)}
.note{display:flex;gap:12px;align-items:flex-start;background:color-mix(in srgb,var(--acc),transparent 92%);border:1px solid color-mix(in srgb,var(--acc),transparent 70%);border-radius:14px;padding:14px 16px;color:var(--fg);margin:14px 0}
.note svg{color:var(--acc);margin-top:1px}
.empty{background:var(--panel);border:1px dashed var(--line);border-radius:16px;padding:30px;text-align:center;color:var(--dim)}
.search{width:100%;max-width:420px;background:var(--panel);border:1px solid var(--line);border-radius:12px;padding:10px 14px;color:var(--fg);font:inherit;margin-bottom:14px;outline:none}
.search:focus{border-color:var(--acc)}
.row-ic{width:32px;height:32px;border-radius:8px;background:var(--panel2);object-fit:cover;display:flex;align-items:center;justify-content:center;color:var(--dim);font-weight:700}
.arrows{font-size:17px;letter-spacing:4px;color:var(--acc);white-space:nowrap}
@media (max-width:820px){nav{display:none}main{padding:22px 18px}}
"#;

fn shell(title: &str, active: &str, sub: &str, body: &str) -> String {
    let mut nav = String::new();
    nav.push_str(&format!("<a class=\"{}\" href=\"imc://\">{}Home</a>", if active.is_empty() { "on" } else { "" }, icon("")));
    for (n, label, _) in PAGES {
        nav.push_str(&format!("<a class=\"{}\" href=\"imc://{n}\">{}{}</a>", if *n == active { "on" } else { "" }, icon(n), label));
    }
    nav.push_str(&format!("<div class=\"sp\"></div><a href=\"imc://settings\">{}Settings</a>", icon("settings")));
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{} - IMC Browse</title><style>{CSS}</style></head><body><div class=\"app\"><nav><div class=\"brand\"><i>IMC</i>IMC Browse</div>{nav}</nav><main><h1>{}</h1><p class=\"sub\">{}</p>{body}</main></div></body></html>",
        esc(title),
        esc(title),
        esc(sub)
    )
}


fn kv(rows: &[(&str, String)]) -> String {
    let mut s = String::from("<table>");
    for (k, v) in rows {
        s.push_str(&format!("<tr><td class=\"k\">{}</td><td>{v}</td></tr>", esc(k)));
    }
    s.push_str("</table>");
    s
}

fn note(svg: &str, html: &str) -> String {
    format!("<div class=\"note\">{svg}<div>{html}</div></div>")
}

fn arrows(path: &str) -> String {
    path.chars().map(|c| match c { 'U' => '\u{2191}', 'D' => '\u{2193}', 'L' => '\u{2190}', _ => '\u{2192}' }).collect()
}

/// Dark/light page for `imc://<name>`; None for unknown names.
pub fn page(name: &str, d: &Internal, stats: &Stats) -> Option<String> {
    let bms = bookmarks::load();
    match name {
        "" => {
            let stats = [
                ("version", format!("IMC Browse {}", env!("CARGO_PKG_VERSION"))),
                ("gpu", if d.hw { "Hardware acceleration on".into() } else { "Hardware acceleration off".into() }),
                ("extensions", format!("{} installed", ext::items().len())),
                ("history", format!("{} pages", crate::history::count())),
                ("bookmarks", format!("{} saved", bms.len())),
                ("blocker", format!("{} blocked this session", stats.blocked)),
                ("shortcuts", format!("{} gestures set", d.gestures.binds.len())),
            ];
            let mut cards = String::new();
            for (n, label, desc) in PAGES {
                let stat = stats.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone()).unwrap_or_default();
                cards.push_str(&format!("<a class=\"card\" href=\"imc://{n}\"><div class=\"ic\">{}</div><b>{label}</b><span class=\"d\">{desc}</span><span class=\"stat\">{}</span></a>", icon(n), esc(&stat)));
            }
            cards.push_str(&format!("<a class=\"card\" href=\"imc://settings\"><div class=\"ic\">{}</div><b>Settings</b><span class=\"d\">Themes, toolbar, gestures, privacy, proxy</span><span class=\"stat\">Open the settings panel</span></a>", icon("settings")));
            let hero = "<div class=\"hero\"><h1><span class=\"grad\">IMC Browse</span> internal pages</h1><p class=\"sub\" style=\"margin:0\">Your browser's own pages: built here, so they work even where the engine's chrome:// and edge:// pages are blocked. Type <code>imc://version</code> (or <code>chrome://version</code>) in the address bar to open any of them.</p></div>";
            Some(shell("Home", "", "Internal pages", &format!("{hero}<div class=\"grid\">{cards}</div>")).replacen("<h1>Home</h1><p class=\"sub\">Internal pages</p>", "", 1))
        }
        "version" => {
            let b = vec![
                ("IMC Browse", env!("CARGO_PKG_VERSION").to_string()),
                ("Engine", format!("Chromium {} (embedded)", esc(&d.engine))),
                ("Operating system", format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)),
                ("Hardware acceleration", if d.hw { "<span class=\"pill ok\">On</span>".into() } else { "<span class=\"pill\">Off</span>".into() }),
                ("Proxy", esc(&d.proxy)),
                ("Secure DNS", esc(&d.doh)),
            ];
            let paths = vec![
                ("User data", config::root().display().to_string()),
                ("Settings", config::root().join("config.json").display().to_string()),
                ("Browser profile", config::profile_dir().display().to_string()),
                ("Filter lists", config::lists_dir().display().to_string()),
                ("Extensions", config::extensions_dir().display().to_string()),
                ("Bookmarks", config::root().join("bookmarks.json").display().to_string()),
            ]
            .into_iter()
            .map(|(k, v)| (k, format!("<code>{}</code>", esc(&v))))
            .collect::<Vec<_>>();
            let body = format!(
                "<h2>Browser</h2>{}<h2>Where your data lives</h2>{}<h2>Command-line switches in effect</h2><pre>{}</pre>",
                kv(&b),
                kv(&paths),
                esc(d.flags.trim())
            );
            Some(shell("Version", "version", "What you are running and where it keeps things", &body))
        }
        "gpu" => {
            let status = if d.hw { "<span class=\"pill ok\">Hardware acceleration is on</span>" } else { "<span class=\"pill bad\">Hardware acceleration is off</span>" };
            let flags: Vec<&str> = d.flags.split_whitespace().filter(|f| f.contains("gpu") || f.contains("zero-copy")).collect();
            let body = format!(
                "<p>{status}</p>{}<h2>GPU switches in effect</h2><pre>{}</pre><h2>Full engine report</h2>{}",
                note(ICON_INFO, "Change this under <a href=\"imc://settings/general\">Settings, General</a>. It needs a restart. Turn it off if you see flicker, black video or graphics-driver crashes."),
                esc(&flags.join("\n")),
                kv(&[("Driver and feature status", "<a href=\"edge://gpu\">edge://gpu</a> (opened by the engine itself)".to_string()), ("Developer tools targets", "<a href=\"edge://inspect\">edge://inspect</a>".to_string())])
            );
            Some(shell("Graphics", "gpu", "Hardware acceleration and GPU flags", &body))
        }
        "extensions" => {
            let items = ext::items();
            let body = if items.is_empty() {
                format!(
                    "<div class=\"empty\"><b>No extensions installed</b><p>Install a Chrome or Firefox extension from the command line, then restart the browser:</p><p><code>imc-browse --install-extension &lt;Chrome Web Store id | Firefox add-on URL | .crx | .xpi | .zip | folder&gt;</code></p></div>{}",
                    note(ICON_INFO, "Chrome extensions run on the embedded Chromium's own extension system. Older (Manifest V2) Chrome extensions and most Firefox add-ons are converted to the current format when installed, which works for many but not all of them.")
                )
            } else {
                let mut rows = String::from("<table><tr><th></th><th>Name</th><th>Made for</th><th>Status</th><th>Open</th></tr>");
                for it in &items {
                    let img = match &it.icon {
                        Some(u) => format!("<img class=\"row-ic\" alt=\"\" src=\"{}\">", esc(u)),
                        None => format!("<div class=\"row-ic\">{}</div>", esc(&it.name.chars().next().unwrap_or('?').to_string())),
                    };
                    let mut links = String::new();
                    if it.error.is_none() {
                        if let Some(p) = &it.popup {
                            links.push_str(&format!("<a href=\"chrome-extension://{}/{}\">Popup</a> ", esc(&it.id), esc(p)));
                        }
                        if let Some(o) = &it.options {
                            links.push_str(&format!("<a href=\"chrome-extension://{}/{}\">Options</a>", esc(&it.id), esc(o)));
                        }
                    }
                    if links.is_empty() {
                        links = "<span class=\"pill\">no page</span>".into();
                    }
                    let status = match &it.error {
                        None => "<span class=\"pill ok\">Loaded</span>".to_string(),
                        Some(e) => format!("<span class=\"pill bad\">Not loaded</span><br><span style=\"color:var(--dim);font-size:12px\">{}</span>", esc(e)),
                    };
                    rows.push_str(&format!(
                        "<tr><td>{img}</td><td><b>{}</b><br><span class=\"mono\" style=\"color:var(--dim);font-size:12px\">{}</span></td><td>{}</td><td>{status}</td><td>{links}</td></tr>",
                        esc(&it.name),
                        esc(&it.id),
                        if it.firefox { "Firefox" } else { "Chrome" }
                    ));
                }
                rows.push_str("</table>");
                format!("{rows}{}", note(ICON_INFO, "Remove one with <code>imc-browse --remove-extension &lt;folder&gt;</code> and restart. Pin them to the toolbar in <a href=\"imc://settings/toolbar\">Settings, Toolbar</a>. Extensions that need a persistent background page may not work."))
            };
            Some(shell("Extensions", "extensions", &format!("{} installed", items.len()), &body))
        }
        "ai" => {
            let state = |on: bool| if on { "<span class=\"pill ok\">On</span>" } else { "<span class=\"pill\">Off</span>" };
            let st = kv(&[
                ("AI side panel", format!("{} {}", state(true), esc(&d.ai_site))),
                ("Command-line control", state(d.control_on).to_string()),
                ("Running JavaScript", if d.allow_js { "<span class=\"pill bad\">Allowed</span>".to_string() } else { "<span class=\"pill\">Not allowed</span>".to_string() }),
            ]);
            let usage = "<pre>imc-browse ctl status\nimc-browse ctl tabs\nimc-browse ctl open https://example.com\nimc-browse ctl text            # the page's visible text\nimc-browse ctl click \"a.more\"\nimc-browse ctl type \"#search\" hello world\nimc-browse ctl screenshot page.png\nimc-browse ctl help</pre>";
            let mut log = String::from("<table><tr><th>When</th><th>Request</th></tr>");
            let now = crate::updater::now_unix();
            let entries = crate::control::audit_list();
            for (t, m) in entries.iter().rev().take(60) {
                log.push_str(&format!("<tr><td class=\"k\">{} s ago</td><td><code>{}</code></td></tr>", now.saturating_sub(*t), esc(m)));
            }
            if entries.is_empty() {
                log.push_str("<tr><td colspan=\"2\" style=\"color:var(--dim)\">Nothing yet.</td></tr>");
            }
            log.push_str("</table>");
            let body = format!(
                "<h2>Status</h2>{st}{}<h2>Commands</h2>{usage}<h2>What AI tools asked for (newest first)</h2>{log}",
                note(ICON_SHIELD, "Control is off until you turn it on in <a href=\"imc://settings/ai\">Settings, AI</a>. It only works for programs running as your Windows user. Ghost tabs are never visible to it, there are no commands for passwords, cards or downloads, and every request is listed here.")
            );
            Some(shell("AI and control", "ai", "The assistant panel and command-line control", &body))
        }
        "bookmarks" => {
            let body = if bms.is_empty() {
                format!("<div class=\"empty\"><b>No bookmarks yet</b><p>Click the bookmark button in the toolbar, or import from Chrome, Edge, Brave or an HTML file in <a href=\"imc://settings/bookmarks\">Settings, Bookmarks</a>.</p></div>")
            } else {
                let mut rows = String::from("<input class=\"search\" id=\"q\" placeholder=\"Filter bookmarks\" autofocus><table id=\"t\"><tr><th></th><th>Title</th><th>Folder</th></tr>");
                for b in &bms {
                    let t = if b.title.is_empty() { &b.url } else { &b.title };
                    rows.push_str(&format!(
                        "<tr><td><div class=\"row-ic\">{}</div></td><td><a href=\"{}\"><b>{}</b></a><br><span class=\"mono\" style=\"color:var(--dim);font-size:12px\">{}</span></td><td>{}</td></tr>",
                        esc(&t.chars().next().unwrap_or('?').to_uppercase().to_string()),
                        esc(&b.url),
                        esc(t),
                        esc(&b.url),
                        esc(&b.folder)
                    ));
                }
                rows.push_str("</table><script>const q=document.getElementById('q');q.oninput=()=>{const v=q.value.toLowerCase();for(const r of document.querySelectorAll('#t tr:not(:first-child)'))r.style.display=r.textContent.toLowerCase().includes(v)?'':'none'}</script>");
                rows
            };
            Some(shell("Bookmarks", "bookmarks", &format!("{} saved", bms.len()), &body))
        }
        "history" => {
            let all = crate::history::all();
            let body = if all.is_empty() {
                "<div class=\"empty\"><b>No history yet</b><p>Pages you visit show up here. Ghost tabs are never recorded, and you can turn history off in <a href=\"imc://settings/privacy\">Settings, Privacy</a>.</p></div>".to_string()
            } else {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                let mut rows = String::from("<div style=\"display:flex;gap:10px;align-items:center\"><input class=\"search\" id=\"q\" placeholder=\"Search history\" autofocus style=\"flex:1\"><button class=\"act\" id=\"clr\">Clear all history</button></div><table id=\"t\"><tr><th>Visited</th><th>Page</th><th>Visits</th><th></th></tr>");
                for e in all.iter().take(1500) {
                    let t = if e.title.is_empty() { &e.url } else { &e.title };
                    let ago = now.saturating_sub(e.ts);
                    let when = match ago {
                        0..=59 => "just now".to_string(),
                        60..=3599 => format!("{} min ago", ago / 60),
                        3600..=86399 => format!("{} h ago", ago / 3600),
                        _ => format!("{} days ago", ago / 86400),
                    };
                    rows.push_str(&format!(
                        "<tr data-u=\"{u}\"><td style=\"white-space:nowrap;color:var(--dim)\">{when}</td><td><a href=\"{u}\"><b>{}</b></a><br><span class=\"mono\" style=\"color:var(--dim);font-size:12px\">{u}</span></td><td>{}</td><td><a href=\"#\" class=\"del\">Remove</a></td></tr>",
                        esc(t),
                        e.visits,
                        u = esc(&e.url)
                    ));
                }
                rows.push_str("</table><script>const q=document.getElementById('q');q.oninput=()=>{const v=q.value.toLowerCase();for(const r of document.querySelectorAll('#t tr:not(:first-child)'))r.style.display=r.textContent.toLowerCase().includes(v)?'':'none'};for(const a of document.querySelectorAll('.del'))a.onclick=e=>{e.preventDefault();const r=a.closest('tr');console.debug('IMCHIST:del:'+r.dataset.u);r.remove()};document.getElementById('clr').onclick=()=>{if(confirm('Delete all browsing history?')){console.debug('IMCHIST:clear');setTimeout(()=>location.reload(),300)}}</script>");
                rows
            };
            Some(shell("History", "history", &format!("{} pages", all.len()), &body))
        }
        "blocker" => {
                        let stats = format!(
                "<div class=\"stats\"><div class=\"stat-card\"><div class=\"l\">Status</div><div class=\"n\">{}</div></div><div class=\"stat-card\"><div class=\"l\">Blocked this session</div><div class=\"n\">{}</div></div><div class=\"stat-card\"><div class=\"l\">Network rules</div><div class=\"n\">{}</div></div><div class=\"stat-card\"><div class=\"l\">Cosmetic filters</div><div class=\"n\">{}</div></div></div>",
                if stats.enabled { "<span class=\"pill ok\">On</span>" } else { "<span class=\"pill bad\">Off</span>" },
                stats.blocked,
                stats.rules,
                stats.cosmetic
            );
            let mut lists = String::from("<table><tr><th>List</th><th>Size</th><th>Updated</th></tr><tr><td><b>Built-in baseline</b></td><td>embedded</td><td>ships with the browser</td></tr>");
            if let Ok(rd) = std::fs::read_dir(config::lists_dir()) {
                for f in rd.flatten() {
                    if f.path().extension().is_some_and(|x| x == "txt") {
                        let meta = f.metadata().ok();
                        let size = meta.as_ref().map(|m| m.len() / 1024).unwrap_or(0);
                        let age = meta.and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).map(|e| format!("{} days ago", e.as_secs() / 86_400)).unwrap_or_default();
                        lists.push_str(&format!("<tr><td>{}</td><td>{size} KB</td><td>{age}</td></tr>", esc(&f.file_name().to_string_lossy())));
                    }
                }
            }
            lists.push_str("</table>");
            let body = format!(
                "{stats}{}<h2>Filter lists</h2>{lists}",
                note(ICON_SHIELD, "Requests are checked before they are fetched, by the browser's own engine. Update lists with <code>imc-browse.exe --update-lists</code>. Toggle blocking with the shield button in the toolbar.")
            );
            Some(shell("Blocker", "blocker", "Ad and tracker filtering", &body))
        }
        "shortcuts" => {
            let keys = [
                ("Ctrl + T", "New tab"),
                ("Ctrl + Shift + N", "New Ghost (private) tab"),
                ("Ctrl + W", "Close tab"),
                ("Ctrl + Shift + T", "Reopen closed tab"),
                ("Ctrl + L", "Focus the address bar"),
                ("Ctrl + D", "Bookmark this page"),
                ("Ctrl + Mouse wheel", "Zoom in / out"),
                ("Ctrl + Plus / Minus / 0", "Zoom in / out / reset"),
                ("F11", "Full screen"),
                ("Esc", "Close menus"),
                ("Middle-click a tab", "Close it"),
            ];
            let mut k = String::from("<table><tr><th>Shortcut</th><th>Action</th></tr>");
            for (a, b) in keys {
                let chips: String = a.split(" + ").map(|p| format!("<kbd>{}</kbd>", esc(p))).collect::<Vec<_>>().join(" + ");
                k.push_str(&format!("<tr><td>{chips}</td><td>{}</td></tr>", esc(b)));
            }
            k.push_str("</table>");
            let mut g = String::from("<table><tr><th>Gesture (hold right mouse button)</th><th>Action</th></tr>");
            for b in &d.gestures.binds {
                let label = config::GESTURE_ACTIONS.iter().find(|(id, _)| *id == b.action).map(|(_, l)| *l).unwrap_or(&b.action);
                g.push_str(&format!("<tr><td class=\"arrows\">{}</td><td>{}</td></tr>", arrows(&b.path), esc(label)));
            }
            g.push_str("</table>");
            let state = if d.gestures.enabled { "<span class=\"pill ok\">Gestures on</span>" } else { "<span class=\"pill\">Gestures off</span>" };
            let body = format!("<h2>Keyboard</h2>{k}<h2>Mouse gestures {state}</h2>{g}{}", note(ICON_INFO, "Change gestures in <a href=\"imc://settings/gestures\">Settings, Mouse gestures</a>."));
            Some(shell("Shortcuts", "shortcuts", "Keyboard and mouse", &body))
        }
        _ => None,
    }
}

pub fn not_found(name: &str) -> String {
    shell("Page not found", "", "There is no internal page with that name", &format!("<div class=\"empty\"><b>imc://{}</b><p>Try one of the pages in the sidebar.</p></div>", esc(name)))
}
