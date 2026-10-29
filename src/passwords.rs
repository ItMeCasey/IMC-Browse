//! Saved logins. Passwords are encrypted with Windows DPAPI (bound to this
//! Windows user account) before touching disk; plaintext exists only in memory
//! while importing, revealing, or filling. Import is from CSV exports (Chrome,
//! Edge, Brave, Firefox, Bitwarden, 1Password). Chrome's own encrypted login
//! database cannot be read directly.

use crate::config::root;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};
use windows::Win32::{
    Foundation::{HLOCAL, LocalFree},
    Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData},
};

const ENTROPY: &[u8] = b"IMC-Browse-v1";

#[derive(Serialize, Deserialize, Clone)]
pub struct Cred {
    pub url: String,
    pub username: String,
    /// hex(DPAPI(password))
    pub enc: String,
}

fn path() -> PathBuf {
    root().join("passwords.json")
}

pub fn load() -> Vec<Cred> {
    fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save(c: &[Cred]) {
    if let Ok(s) = serde_json::to_string(c) {
        let _ = fs::write(path(), s);
    }
}

fn blob(b: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB { cbData: b.len() as u32, pbData: b.as_ptr() as *mut u8 }
}

fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    unsafe {
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData as *mut _)));
        v
    }
}

pub fn encrypt(plain: &str) -> Option<String> {
    protect(plain, ENTROPY)
}

pub fn decrypt(enc: &str) -> Option<String> {
    unprotect(enc, ENTROPY)
}

/// DPAPI-encrypts `plain` for this Windows user; `entropy` separates what each store can decrypt.
pub fn protect(plain: &str, entropy: &[u8]) -> Option<String> {
    unsafe {
        let mut out = CRYPT_INTEGER_BLOB::default();
        CryptProtectData(&blob(plain.as_bytes()), None, Some(&blob(entropy)), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out).ok()?;
        Some(hex(&take(out)))
    }
}

pub fn unprotect(enc: &str, entropy: &[u8]) -> Option<String> {
    unsafe {
        let data = unhex(enc)?;
        let mut out = CRYPT_INTEGER_BLOB::default();
        CryptUnprotectData(&blob(&data), None, Some(&blob(entropy)), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out).ok()?;
        String::from_utf8(take(out)).ok()
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

pub fn host(url: &str) -> String {
    url.split("://").nth(1).unwrap_or(url).split(['/', '?', '#', ':']).next().unwrap_or("").to_ascii_lowercase()
}

/// RFC 4180 CSV (quoted fields, doubled quotes, embedded newlines).
pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let (mut rows, mut row, mut f) = (vec![], vec![], String::new());
    let (mut q, mut any) = (false, false);
    let mut it = text.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(c) = it.next() {
        any = true;
        match (q, c) {
            (true, '"') if it.peek() == Some(&'"') => {
                f.push('"');
                it.next();
            }
            (true, '"') => q = false,
            (false, '"') => q = true,
            (false, ',') => row.push(std::mem::take(&mut f)),
            (false, '\r') => {}
            (false, '\n') => {
                row.push(std::mem::take(&mut f));
                rows.push(std::mem::take(&mut row));
                any = false;
            }
            _ => f.push(c),
        }
    }
    if any {
        row.push(f);
        rows.push(row);
    }
    rows
}

/// Imports a password-manager CSV; returns (added_or_updated, skipped).
pub fn import_csv(into: &mut Vec<Cred>, text: &str) -> Result<(usize, usize), String> {
    let rows = parse_csv(text);
    let head: Vec<String> = rows.first().ok_or("empty file")?.iter().map(|h| h.trim().to_ascii_lowercase()).collect();
    let col = |names: &[&str]| head.iter().position(|h| names.contains(&h.as_str()));
    let (iu, il, ip) = (
        col(&["url", "login_uri", "website", "origin", "web site"]),
        col(&["username", "login_username", "login", "user", "email"]),
        col(&["password", "login_password"]),
    );
    let (Some(iu), Some(il), Some(ip)) = (iu, il, ip) else {
        return Err("CSV needs url, username and password columns".into());
    };
    let (mut ok, mut skip) = (0, 0);
    for r in rows.iter().skip(1) {
        let (Some(u), Some(l), Some(p)) = (r.get(iu), r.get(il), r.get(ip)) else { skip += 1; continue };
        if u.is_empty() || p.is_empty() || host(u).is_empty() {
            skip += 1;
            continue;
        }
        let Some(enc) = encrypt(p) else { skip += 1; continue };
        let h = host(u);
        into.retain(|c| !(host(&c.url) == h && c.username == *l));
        into.push(Cred { url: u.clone(), username: l.clone(), enc });
        ok += 1;
    }
    Ok((ok, skip))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_quotes_and_newlines() {
        let r = parse_csv("a,b\r\n\"x,1\",\"he said \"\"hi\"\"\nthere\"\r\n");
        assert_eq!(r[1], vec!["x,1".to_string(), "he said \"hi\"\nthere".to_string()]);
    }

    #[test]
    fn dpapi_roundtrip_and_import() {
        let e = encrypt("s3cret!").unwrap();
        assert!(!e.contains("s3cret"));
        assert_eq!(decrypt(&e).unwrap(), "s3cret!");
        let mut v = vec![];
        let (n, _) = import_csv(&mut v, "name,url,username,password\nA,https://a.com/login,me,pw1\nA,https://a.com/x,me,pw2\n").unwrap();
        assert_eq!((n, v.len()), (2, 1));
        assert_eq!(decrypt(&v[0].enc).unwrap(), "pw2");
    }
}
