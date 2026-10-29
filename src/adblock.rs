//! Network filter engine: Adblock-Plus / uBlock-style network rules.
//!
//! Lookup is O(tokens in URL): every rule is indexed under one rare token (or
//! a hostname for `||host^` rules), so a request only evaluates the handful of
//! rules that share a token with its URL instead of the whole list.

use std::collections::HashMap;

pub const T_SCRIPT: u16 = 1;
pub const T_IMAGE: u16 = 2;
pub const T_STYLE: u16 = 4;
pub const T_XHR: u16 = 8;
pub const T_SUBDOC: u16 = 16;
pub const T_MEDIA: u16 = 32;
pub const T_FONT: u16 = 64;
pub const T_PING: u16 = 128;
pub const T_DOC: u16 = 256;
pub const T_OTHER: u16 = 512;
const T_ALL_BUT_DOC: u16 = 0x3FF & !T_DOC;

pub struct Request<'a> {
    pub url: &'a str,
    /// Hostname of the page that issued the request (empty for top-level).
    pub source_host: &'a str,
    pub kind: u16,
}

#[derive(Default)]
struct Rule {
    pat: Vec<u8>,
    host_anchor: bool,
    start_anchor: bool,
    end_anchor: bool,
    /// Resource types this rule applies to.
    types: u16,
    /// Some(true) = `$third-party`, Some(false) = `$~third-party`.
    third_party: Option<bool>,
    dom_in: Vec<String>,
    dom_out: Vec<String>,
}

#[derive(Default)]
pub struct Engine {
    block_tok: HashMap<u32, Vec<Rule>>,
    block_host: HashMap<String, Vec<Rule>>,
    block_any: Vec<Rule>,
    allow_tok: HashMap<u32, Vec<Rule>>,
    allow_host: HashMap<String, Vec<Rule>>,
    allow_any: Vec<Rule>,
    /// Generic (`##selector`) cosmetic filters.
    pub cosmetic: Vec<String>,
    pub rule_count: usize,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load_list(&mut self, text: &str) {
        for line in text.lines() {
            self.add_line(line.trim());
        }
    }

    pub fn add_line(&mut self, line: &str) {
        if line.is_empty() || line.starts_with('!') || line.starts_with('[') {
            return;
        }
        if let Some(i) = line.find("##") {
            if i == 0 && self.cosmetic.len() < 3000 {
                let sel = &line[2..];
                if !sel.starts_with("+js") && !sel.contains(":-abp") && !sel.contains(":has-text") && !sel.contains('\\') {
                    self.cosmetic.push(sel.to_string());
                }
            }
            return;
        }
        if line.contains("#@#") || line.contains("#?#") || line.contains("#$#") || line.contains("#%#") {
            return;
        }
        let (allow, body) = match line.strip_prefix("@@") {
            Some(b) => (true, b),
            None => (false, line),
        };
        let (pat_str, opts) = match body.rfind('$') {
            Some(i) if !body[i..].contains('/') => (&body[..i], Some(&body[i + 1..])),
            _ => (body, None),
        };
        let mut rule = Rule { types: T_ALL_BUT_DOC, ..Default::default() };
        if let Some(opts) = opts {
            if !parse_options(opts, &mut rule) {
                return;
            }
        }
        let mut p = pat_str;
        if let Some(r) = p.strip_prefix("||") {
            rule.host_anchor = true;
            p = r;
        } else if let Some(r) = p.strip_prefix('|') {
            rule.start_anchor = true;
            p = r;
        }
        if let Some(r) = p.strip_suffix('|') {
            rule.end_anchor = true;
            p = r;
        }
        if p.len() < 2 && !rule.host_anchor {
            return; // too generic to be useful
        }
        rule.pat = p.to_ascii_lowercase().into_bytes();
        self.rule_count += 1;
        self.insert(rule, allow);
    }

    fn insert(&mut self, rule: Rule, allow: bool) {
        let (tok, host, any) = if allow {
            (&mut self.allow_tok, &mut self.allow_host, &mut self.allow_any)
        } else {
            (&mut self.block_tok, &mut self.block_host, &mut self.block_any)
        };
        if rule.host_anchor {
            let end = rule.pat.iter().position(|c| matches!(c, b'^' | b'/' | b':' | b'*' | b'?')).unwrap_or(rule.pat.len());
            let next = rule.pat.get(end).copied();
            if end > 0 && matches!(next, None | Some(b'^') | Some(b'/') | Some(b':') | Some(b'?')) {
                let h = String::from_utf8_lossy(&rule.pat[..end]).into_owned();
                host.entry(h).or_default().push(rule);
                return;
            }
        } else if let Some(t) = best_token(&rule) {
            tok.entry(t).or_default().push(rule);
            return;
        }
        any.push(rule);
    }

    /// True if the request should be blocked.
    pub fn should_block(&self, req: &Request) -> bool {
        let url = req.url.to_ascii_lowercase();
        let Some((host, _)) = host_range(&url) else { return false };
        let host = host.to_string();
        let third = is_third_party(&host, req.source_host);
        let ctx = Ctx { url: url.as_bytes(), host: &host, source: req.source_host, third, kind: req.kind };
        if self.any_match(&self.allow_tok, &self.allow_host, &self.allow_any, &ctx) {
            return false;
        }
        self.any_match(&self.block_tok, &self.block_host, &self.block_any, &ctx)
    }

    fn any_match(
        &self,
        tok: &HashMap<u32, Vec<Rule>>,
        hosts: &HashMap<String, Vec<Rule>>,
        any: &[Rule],
        c: &Ctx,
    ) -> bool {
        // host-keyed rules: walk label suffixes (a.b.c.com -> b.c.com -> c.com ...)
        let mut h = c.host;
        loop {
            if let Some(rs) = hosts.get(h) {
                if rs.iter().any(|r| rule_matches(r, c)) {
                    return true;
                }
            }
            match h.find('.') {
                Some(i) => h = &h[i + 1..],
                None => break,
            }
        }
        // token-keyed rules
        let u = c.url;
        let mut i = 0;
        while i < u.len() {
            if u[i].is_ascii_alphanumeric() {
                let s = i;
                while i < u.len() && u[i].is_ascii_alphanumeric() {
                    i += 1;
                }
                if i - s >= 3 {
                    if let Some(rs) = tok.get(&fnv(&u[s..i])) {
                        if rs.iter().any(|r| rule_matches(r, c)) {
                            return true;
                        }
                    }
                }
            } else {
                i += 1;
            }
        }
        any.iter().any(|r| rule_matches(r, c))
    }

    /// CSS hiding generic cosmetic selectors, ready to inject.
    pub fn cosmetic_css(&self) -> String {
        let mut css = String::new();
        for chunk in self.cosmetic.chunks(200) {
            css.push_str(&chunk.join(","));
            css.push_str("{display:none!important}\n");
        }
        css
    }
}

struct Ctx<'a> {
    url: &'a [u8],
    host: &'a str,
    source: &'a str,
    third: bool,
    kind: u16,
}

fn parse_options(opts: &str, r: &mut Rule) -> bool {
    let mut pos = 0u16;
    let mut neg = 0u16;
    for o in opts.split(',') {
        let (not, name) = match o.strip_prefix('~') {
            Some(n) => (true, n),
            None => (false, o),
        };
        let ty = match name {
            "script" => T_SCRIPT,
            "image" => T_IMAGE,
            "stylesheet" => T_STYLE,
            "xmlhttprequest" | "xhr" => T_XHR,
            "subdocument" => T_SUBDOC,
            "media" => T_MEDIA,
            "font" => T_FONT,
            "ping" => T_PING,
            "document" | "doc" => T_DOC,
            "other" => T_OTHER,
            _ => 0,
        };
        if ty != 0 {
            if not { neg |= ty } else { pos |= ty }
            continue;
        }
        match name {
            "third-party" | "3p" => r.third_party = Some(!not),
            "first-party" | "1p" => r.third_party = Some(not),
            "important" | "match-case" | "popup" => {}
            n if n.starts_with("domain=") => {
                for d in n[7..].split('|') {
                    match d.strip_prefix('~') {
                        Some(x) => r.dom_out.push(x.to_ascii_lowercase()),
                        None => r.dom_in.push(d.to_ascii_lowercase()),
                    }
                }
            }
            // redirect=, removeparam=, csp=, replace=, badfilter ... need more than allow/deny.
            _ => return false,
        }
    }
    if pos != 0 {
        r.types = pos;
    }
    r.types &= !neg;
    true
}

fn rule_matches(r: &Rule, c: &Ctx) -> bool {
    if r.types & c.kind == 0 {
        return false;
    }
    if let Some(t) = r.third_party {
        if t != c.third {
            return false;
        }
    }
    if !r.dom_in.is_empty() && !r.dom_in.iter().any(|d| host_in_domain(c.source, d)) {
        return false;
    }
    if r.dom_out.iter().any(|d| host_in_domain(c.source, d)) {
        return false;
    }
    let u = c.url;
    let p = &r.pat[..];
    if r.host_anchor {
        let Some((_, start)) = host_range(std::str::from_utf8(u).unwrap_or("")) else { return false };
        let host_end = start + c.host.len();
        let mut pos = start;
        loop {
            if match_here(p, &u[pos..], r.end_anchor) {
                return true;
            }
            match u[pos..host_end].iter().position(|&b| b == b'.') {
                Some(i) => pos += i + 1,
                None => return false,
            }
        }
    }
    if r.start_anchor {
        return match_here(p, u, r.end_anchor);
    }
    (0..=u.len()).any(|i| match_here(p, &u[i..], r.end_anchor))
}

/// Match `p` against the *start* of `s`. `*` = any run, `^` = separator or end.
fn match_here(p: &[u8], s: &[u8], end_anchor: bool) -> bool {
    match p.first() {
        None => !end_anchor || s.is_empty(),
        Some(b'*') => (0..=s.len()).any(|i| match_here(&p[1..], &s[i..], end_anchor)),
        Some(b'^') => match s.first() {
            None => match_here(&p[1..], s, end_anchor),
            Some(&c) if !(c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b'%')) => {
                match_here(&p[1..], &s[1..], end_anchor)
            }
            _ => false,
        },
        Some(&c) => matches!(s.first(), Some(&x) if x == c) && match_here(&p[1..], &s[1..], end_anchor),
    }
}

fn best_token(r: &Rule) -> Option<u32> {
    let p = &r.pat;
    let mut best: Option<&[u8]> = None;
    let mut i = 0;
    while i < p.len() {
        if p[i].is_ascii_alphanumeric() {
            let s = i;
            while i < p.len() && p[i].is_ascii_alphanumeric() {
                i += 1;
            }
            let left_ok = if s == 0 { r.start_anchor } else { p[s - 1] != b'*' };
            let right_ok = if i == p.len() { r.end_anchor } else { p[i] != b'*' };
            if left_ok && right_ok && i - s >= 3 && best.is_none_or(|b| i - s > b.len()) {
                best = Some(&p[s..i]);
            }
        } else {
            i += 1;
        }
    }
    best.map(fnv)
}

fn fnv(b: &[u8]) -> u32 {
    b.iter().fold(0x811c9dc5u32, |h, &c| (h ^ c as u32).wrapping_mul(16777619))
}

/// (host, byte offset of host within url)
fn host_range(url: &str) -> Option<(&str, usize)> {
    let a = url.find("://")? + 3;
    let rest = &url[a..];
    let end = rest.find(['/', '?', '#', ':']).unwrap_or(rest.len());
    let auth = rest[..end].rsplit('@').next()?;
    let off = a + (end - auth.len());
    Some((auth, off))
}

fn host_in_domain(host: &str, dom: &str) -> bool {
    host == dom || host.strip_suffix(dom).is_some_and(|p| p.ends_with('.'))
}

const MULTI_TLD: &[&str] = &["co.uk", "org.uk", "ac.uk", "com.au", "net.au", "co.jp", "co.nz", "com.br", "co.in", "co.za", "com.cn", "com.mx"];

pub fn registrable(host: &str) -> &str {
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() <= 2 {
        return host;
    }
    let n = if MULTI_TLD.iter().any(|t| host.ends_with(&format!(".{t}"))) { 3 } else { 2 };
    let skip: usize = labels[..labels.len() - n].iter().map(|l| l.len() + 1).sum();
    &host[skip..]
}

fn is_third_party(host: &str, source: &str) -> bool {
    !source.is_empty() && registrable(host) != registrable(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eng(rules: &str) -> Engine {
        let mut e = Engine::new();
        e.load_list(rules);
        e
    }
    fn blocked(e: &Engine, url: &str, src: &str, kind: u16) -> bool {
        e.should_block(&Request { url, source_host: src, kind })
    }

    #[test]
    fn host_anchor_and_exception() {
        let e = eng("||ads.example.com^\n@@||ads.example.com/ok.js");
        assert!(blocked(&e, "https://ads.example.com/x.js", "site.org", T_SCRIPT));
        assert!(blocked(&e, "https://sub.ads.example.com/x", "site.org", T_IMAGE));
        assert!(!blocked(&e, "https://ads.example.com/ok.js", "site.org", T_SCRIPT));
        assert!(!blocked(&e, "https://notads.example.com/x.js", "site.org", T_SCRIPT));
    }

    #[test]
    fn wildcard_and_options() {
        let e = eng("/banner/*/img.$image,third-party\n||track.io^$script,domain=a.com|~b.a.com");
        assert!(blocked(&e, "https://cdn.x.com/banner/300/img.png", "site.org", T_IMAGE));
        assert!(!blocked(&e, "https://site.org/banner/300/img.png", "site.org", T_IMAGE));
        assert!(!blocked(&e, "https://cdn.x.com/banner/300/img.png", "site.org", T_SCRIPT));
        assert!(blocked(&e, "https://track.io/t.js", "a.com", T_SCRIPT));
        assert!(!blocked(&e, "https://track.io/t.js", "b.a.com", T_SCRIPT));
        assert!(!blocked(&e, "https://track.io/t.js", "other.com", T_SCRIPT));
    }

    #[test]
    fn registrable_domains() {
        assert_eq!(registrable("a.b.example.co.uk"), "example.co.uk");
        assert_eq!(registrable("www.example.com"), "example.com");
    }
}
