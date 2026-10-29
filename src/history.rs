//! Browsing history: every page you finish loading (never in a Ghost tab, and only when the switch in
//! Settings, Privacy is on). Kept in memory, saved to history.json in the profile folder every few
//! seconds and when the browser closes.

use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};

const MAX: usize = 20_000;

#[derive(Serialize, Deserialize, Clone)]
pub struct Entry {
    pub url: String,
    pub title: String,
    /// Last visit, unix seconds.
    pub ts: u64,
    pub visits: u32,
}

#[derive(Default)]
struct Store {
    items: Vec<Entry>, // oldest first
    dirty: bool,
}

fn path() -> std::path::PathBuf {
    crate::config::root().join("history.json")
}

fn store() -> &'static Mutex<Store> {
    static S: OnceLock<Mutex<Store>> = OnceLock::new();
    S.get_or_init(|| {
        let items = std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Mutex::new(Store { items, dirty: false })
    })
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn recordable(url: &str) -> bool {
    (url.starts_with("http://") || url.starts_with("https://")) && url.len() < 2000
}

/// A page finished loading: counts a visit and moves it to the top.
pub fn visit(url: &str, title: &str) {
    if !recordable(url) {
        return;
    }
    let mut s = store().lock().unwrap();
    let mut e = match s.items.iter().position(|e| e.url == url) {
        Some(i) => s.items.remove(i),
        None => Entry { url: url.into(), title: String::new(), ts: 0, visits: 0 },
    };
    e.ts = now();
    e.visits += 1;
    if !title.is_empty() {
        e.title = title.into();
    }
    s.items.push(e);
    if s.items.len() > MAX {
        s.items.remove(0);
    }
    s.dirty = true;
}

/// The page's title arrived after the load finished.
pub fn retitle(url: &str, title: &str) {
    if title.is_empty() {
        return;
    }
    let mut s = store().lock().unwrap();
    if let Some(e) = s.items.iter_mut().rev().take(50).find(|e| e.url == url) {
        if e.title != title {
            e.title = title.into();
            s.dirty = true;
        }
    }
}

/// Newest first.
pub fn all() -> Vec<Entry> {
    store().lock().unwrap().items.iter().rev().cloned().collect()
}

pub fn count() -> usize {
    store().lock().unwrap().items.len()
}

pub fn remove(url: &str) {
    let mut s = store().lock().unwrap();
    s.items.retain(|e| e.url != url);
    s.dirty = true;
}

/// Clears everything (`since` = 0) or only visits newer than `since` (unix seconds).
pub fn clear_since(since: u64) {
    let mut s = store().lock().unwrap();
    if since == 0 {
        s.items.clear();
    } else {
        s.items.retain(|e| e.ts < since);
    }
    s.dirty = true;
    drop(s);
    flush();
}

/// Best matches for what is being typed: url or title contains every word; most-visited, most recent first.
pub fn suggest(q: &str, n: usize) -> Vec<Entry> {
    let words: Vec<String> = q.to_lowercase().split_whitespace().map(String::from).collect();
    if words.is_empty() {
        return vec![];
    }
    let s = store().lock().unwrap();
    let mut hits: Vec<&Entry> = s
        .items
        .iter()
        .filter(|e| {
            let hay = format!("{} {}", e.url.to_lowercase(), e.title.to_lowercase());
            words.iter().all(|w| hay.contains(w))
        })
        .collect();
    hits.sort_by(|a, b| b.visits.cmp(&a.visits).then(b.ts.cmp(&a.ts)));
    hits.into_iter().take(n).cloned().collect()
}

pub fn flush() {
    let mut s = store().lock().unwrap();
    if s.dirty {
        if let Ok(j) = serde_json::to_string(&s.items) {
            let _ = std::fs::write(path(), j);
        }
        s.dirty = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visits_merge_and_suggest() {
        visit("https://a.example/x", "Alpha page");
        visit("https://b.example/", "Beta");
        visit("https://a.example/x", "");
        let a = all();
        assert_eq!(a[0].url, "https://a.example/x");
        assert_eq!(a[0].visits, 2);
        assert_eq!(a[0].title, "Alpha page");
        assert!(suggest("alpha", 5).iter().any(|e| e.url == "https://a.example/x"));
        assert!(suggest("zzz", 5).is_empty());
        visit("imc://history", "no");
        assert!(!all().iter().any(|e| e.url.starts_with("imc")));
        remove("https://b.example/");
        assert!(!all().iter().any(|e| e.url == "https://b.example/"));
    }
}
