//! Bookmarks store + importers (Netscape HTML export from any browser, and the
//! `Bookmarks` JSON file Chrome, Edge and Brave keep in their profile).

use crate::config::root;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs, path::PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Bookmark {
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub folder: String,
}

fn path() -> PathBuf {
    root().join("bookmarks.json")
}

pub fn load() -> Vec<Bookmark> {
    fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save(b: &[Bookmark]) {
    if let Ok(s) = serde_json::to_string_pretty(b) {
        let _ = fs::write(path(), s);
    }
}

/// Adds entries whose URL is new; returns how many were added.
pub fn merge(into: &mut Vec<Bookmark>, new: Vec<Bookmark>) -> usize {
    let mut n = 0;
    for b in new {
        if (b.url.starts_with("http://") || b.url.starts_with("https://")) && !into.iter().any(|x| x.url == b.url) {
            into.push(b);
            n += 1;
        }
    }
    n
}

pub fn parse_auto(text: &str) -> Vec<Bookmark> {
    let t = text.trim_start_matches('\u{feff}').trim_start();
    if t.starts_with('{') { parse_chrome_json(t) } else { parse_netscape_html(t) }
}

pub fn parse_chrome_json(text: &str) -> Vec<Bookmark> {
    fn walk(v: &Value, folder: &str, out: &mut Vec<Bookmark>) {
        match v["type"].as_str() {
            Some("url") => out.push(Bookmark {
                title: v["name"].as_str().unwrap_or("").into(),
                url: v["url"].as_str().unwrap_or("").into(),
                folder: folder.into(),
            }),
            Some("folder") => {
                let name = v["name"].as_str().unwrap_or("");
                let f = if folder.is_empty() { name.to_string() } else { format!("{folder}/{name}") };
                for c in v["children"].as_array().into_iter().flatten() {
                    walk(c, &f, out);
                }
            }
            _ => {}
        }
    }
    let mut out = vec![];
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        if let Some(roots) = v["roots"].as_object() {
            for r in roots.values() {
                if r.is_object() {
                    walk(r, "", &mut out);
                }
            }
        }
    }
    out
}

pub fn parse_netscape_html(html: &str) -> Vec<Bookmark> {
    let lower = html.to_ascii_lowercase();
    let mut out = vec![];
    let mut stack: Vec<String> = vec![];
    let mut pending: Option<String> = None;
    let mut i = 0;
    while let Some(p) = lower[i..].find('<') {
        let s = i + p;
        let Some(e) = lower[s..].find('>') else { break };
        let tag = &lower[s + 1..s + e];
        let after = s + e + 1;
        if tag.starts_with("h3") {
            let end = lower[after..].find("</h3").map_or(lower.len(), |x| after + x);
            pending = Some(decode(&html[after..end]));
            i = end;
        } else if tag.starts_with("dl") {
            stack.push(pending.take().unwrap_or_default());
            i = after;
        } else if tag.starts_with("/dl") {
            stack.pop();
            i = after;
        } else if tag.starts_with("a ") {
            let end = lower[after..].find("</a").map_or(lower.len(), |x| after + x);
            if let Some(url) = attr(&html[s + 1..s + e], "href") {
                let folder = stack.iter().filter(|f| !f.is_empty()).cloned().collect::<Vec<_>>().join("/");
                out.push(Bookmark { title: decode(&html[after..end]), url: decode(&url), folder });
            }
            i = end;
        } else {
            i = after;
        }
    }
    out
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let l = tag.to_ascii_lowercase();
    let k = l.find(&format!("{name}=\""))? + name.len() + 2;
    let end = tag[k..].find('"')?;
    Some(tag[k..k + end].to_string())
}

fn decode(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// Bookmark files of installed Chromium browsers (default profile).
pub fn browser_files() -> Vec<(String, PathBuf)> {
    let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) else { return vec![] };
    [
        ("Chrome", "Google/Chrome/User Data/Default/Bookmarks"),
        ("Edge", "Microsoft/Edge/User Data/Default/Bookmarks"),
        ("Brave", "BraveSoftware/Brave-Browser/User Data/Default/Bookmarks"),
        ("Vivaldi", "Vivaldi/User Data/Default/Bookmarks"),
    ]
    .iter()
    .map(|(n, p)| (n.to_string(), local.join(p)))
    .filter(|(_, p)| p.is_file())
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn netscape_nested_folders() {
        let h = r#"<DL><p><DT><H3>Dev</H3><DL><p><DT><A HREF="https://a.com/?x=1&amp;y=2" ADD_DATE="1">A &amp; B</A></DL><p>
        <DT><A HREF="https://top.org">Top</A></DL>"#;
        let b = parse_netscape_html(h);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0], Bookmark { title: "A & B".into(), url: "https://a.com/?x=1&y=2".into(), folder: "Dev".into() });
        assert_eq!(b[1].folder, "");
    }

    #[test]
    fn chrome_json() {
        let j = r#"{"roots":{"bookmark_bar":{"type":"folder","name":"Bar","children":[{"type":"url","name":"X","url":"https://x.com"},{"type":"folder","name":"Sub","children":[{"type":"url","name":"Y","url":"https://y.com"}]}]}}}"#;
        let b = parse_auto(j);
        assert_eq!(b.len(), 2);
        assert_eq!(b[1].folder, "Bar/Sub");
    }
}
