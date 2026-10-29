//! CLI control for AI tools: `imc-browse ctl <command>` talks to the running browser over a named
//! pipe, so an AI agent (Claude Code, a script, anything that can run a command) can open tabs, read
//! pages, click and type.
//!
//! Safety, because this is powerful:
//! - Off by default. Nothing listens until the user turns it on in Settings, AI control.
//! - The pipe can only be opened by the same Windows user (owner-only ACL), never remotely, and the
//!   name cannot be squatted (the first instance is created exclusively).
//! - Ghost (private) tabs are invisible to it. Running arbitrary JavaScript is a second switch.
//! - Passwords, cards and downloads have no commands. Every request is written to an audit log
//!   (shown at imc://ai).

use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    io::{Read, Write},
    sync::{
        Arc, Mutex, mpsc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use windows::{
    Win32::{
        Foundation::{CloseHandle, ERROR_PIPE_CONNECTED, GetLastError, HANDLE, HLOCAL, INVALID_HANDLE_VALUE, LocalFree},
        Security::{
            Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1},
            PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
        },
        Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, FlushFileBuffers, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile},
        System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT},
    },
    core::HSTRING,
};

const MAX_REQUEST: usize = 1 << 20;

pub struct Request {
    pub body: Value,
    pub reply: mpsc::Sender<Value>,
}

pub fn pipe_name() -> String {
    format!(r"\\.\pipe\imc-browse-control-{}", std::env::var("USERNAME").unwrap_or_default().to_ascii_lowercase())
}

static AUDIT: Mutex<VecDeque<(u64, String)>> = Mutex::new(VecDeque::new());

/// Records what an AI tool asked for (kept in memory, newest last, 200 entries).
pub fn audit(msg: String) {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut a = AUDIT.lock().unwrap();
    a.push_back((t, msg));
    while a.len() > 200 {
        a.pop_front();
    }
}

pub fn audit_list() -> Vec<(u64, String)> {
    AUDIT.lock().unwrap().iter().cloned().collect()
}

fn create_pipe(first: bool) -> Option<HANDLE> {
    unsafe {
        // Owner only: the Windows user running the browser, nobody else.
        let mut psd = PSECURITY_DESCRIPTOR::default();
        ConvertStringSecurityDescriptorToSecurityDescriptorW(&HSTRING::from("D:P(A;;GA;;;OW)"), SDDL_REVISION_1, &mut psd, None).ok()?;
        let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: psd.0, bInheritHandle: false.into() };
        let mode = PIPE_ACCESS_DUPLEX | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { Default::default() };
        let h = CreateNamedPipeW(&HSTRING::from(pipe_name()), mode, PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS, 4, 1 << 16, 1 << 16, 0, Some(&sa));
        let _ = LocalFree(Some(HLOCAL(psd.0)));
        (h != INVALID_HANDLE_VALUE).then_some(h)
    }
}

fn read_line(h: HANDLE) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let mut n = 0u32;
        unsafe { ReadFile(h, Some(&mut buf), Some(&mut n), None).ok()? };
        if n == 0 {
            return Some(out);
        }
        out.extend_from_slice(&buf[..n as usize]);
        if out.contains(&b'\n') || out.len() > MAX_REQUEST {
            return Some(out);
        }
    }
}

fn write_all(h: HANDLE, data: &[u8]) {
    let mut off = 0;
    while off < data.len() {
        let mut n = 0u32;
        if unsafe { WriteFile(h, Some(&data[off..]), Some(&mut n), None) }.is_err() || n == 0 {
            return;
        }
        off += n as usize;
    }
    unsafe {
        let _ = FlushFileBuffers(h);
    }
}

/// Serves requests while `enabled` is true. `deliver` hands each request to the browser's UI thread.
pub fn serve(enabled: Arc<AtomicBool>, deliver: impl Fn(Request) + Send + 'static) {
    std::thread::spawn(move || {
        let mut first = true;
        loop {
            if !enabled.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
            let Some(h) = create_pipe(first) else {
                if first {
                    // someone else already owns the name: do not serve
                    audit("control pipe could not be created exclusively; AI control is unavailable".into());
                    return;
                }
                std::thread::sleep(Duration::from_secs(1));
                continue;
            };
            first = false;
            let connected = unsafe { ConnectNamedPipe(h, None).is_ok() || GetLastError() == ERROR_PIPE_CONNECTED };
            if connected {
                let reply = if !enabled.load(Ordering::Relaxed) {
                    json!({"error": "AI control is turned off in the browser's settings"})
                } else {
                    match read_line(h).and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
                        None => json!({"error": "bad request"}),
                        Some(body) => {
                            let (tx, rx) = mpsc::channel();
                            deliver(Request { body, reply: tx });
                            rx.recv_timeout(Duration::from_secs(30)).unwrap_or_else(|_| json!({"error": "the browser did not answer in time"}))
                        }
                    }
                };
                let mut line = reply.to_string().into_bytes();
                line.push(b'\n');
                write_all(h, &line);
                unsafe {
                    let _ = DisconnectNamedPipe(h);
                }
            }
            unsafe {
                let _ = CloseHandle(h);
            }
        }
    });
}

// ---------------------------------------------------------------------------------------------
// client: `imc-browse ctl ...`

fn b64_decode(s: &str) -> Vec<u8> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let (mut out, mut acc, mut bits) = (Vec::new(), 0u32, 0);
    for c in s.bytes().filter_map(val) {
        acc = acc << 6 | c as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

pub const HELP: &str = "imc-browse ctl <command>   (AI control must be on in Settings, AI control)

  status                      version, open tabs, what is allowed
  tabs                        list tabs: id, title, url, active, loading
  open <url>                  open a new tab
  navigate <url> [--tab N]    load a URL in a tab (default: the active one)
  switch <id>                 make a tab active
  close [id]                  close a tab
  back | forward | reload [--tab N]
  text [--tab N]              the page's visible text
  html [--tab N]              the page's HTML source
  screenshot <file.png> [--tab N]
  click <css-selector> [--tab N]
  type <css-selector> <text...> [--tab N]
  scroll <up|down|top|bottom> [--tab N]
  eval <javascript...> [--tab N]   (needs 'Allow running JavaScript' in settings)
  wait [seconds]              wait until the tab has finished loading
  help

Output is JSON (text and html print as plain text). Exit code 1 on any error.";

fn call(req: &Value) -> Result<Value, String> {
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(pipe_name()).map_err(|_| "cannot reach the browser: is IMC Browse running with AI control turned on?".to_string())?;
    let mut line = req.to_string().into_bytes();
    line.push(b'\n');
    f.write_all(&line).map_err(|e| e.to_string())?;
    // The answer is one line. The server closes the pipe right after it, which can surface as an
    // error on the next read, so stop at the newline instead of waiting for end-of-file.
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                out.extend_from_slice(&buf[..n]);
                if out.contains(&b'\n') {
                    break;
                }
            }
            Err(e) => {
                if out.is_empty() {
                    return Err(e.to_string());
                }
                break;
            }
        }
    }
    serde_json::from_slice(&out).map_err(|_| "unreadable answer from the browser".to_string())
}

/// Runs a `ctl` command; returns the process exit code.
pub fn client(args: &[String]) -> i32 {
    let mut tab: Option<u64> = None;
    let mut rest: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--tab" {
            tab = it.next().and_then(|v| v.parse().ok());
        } else {
            rest.push(a.clone());
        }
    }
    let Some(cmd) = rest.first().cloned() else {
        println!("{HELP}");
        return 1;
    };
    let arg = |i: usize| rest.get(i).cloned().unwrap_or_default();
    let tail = |i: usize| rest.iter().skip(i).cloned().collect::<Vec<_>>().join(" ");
    let mut req = json!({"cmd": cmd, "tab": tab});
    match cmd.as_str() {
        "help" | "--help" => {
            println!("{HELP}");
            return 0;
        }
        "open" | "navigate" => req["url"] = json!(arg(1)),
        "switch" | "close" => req["id"] = json!(arg(1).parse::<u64>().ok()),
        "click" => req["selector"] = json!(arg(1)),
        "type" => {
            req["selector"] = json!(arg(1));
            req["text"] = json!(tail(2));
        }
        "scroll" => req["to"] = json!(arg(1)),
        "eval" => req["expression"] = json!(tail(1)),
        "wait" => return wait(arg(1).parse().unwrap_or(30), tab),
        "status" | "tabs" | "back" | "forward" | "reload" | "text" | "html" | "screenshot" => {}
        _ => {
            eprintln!("unknown command '{cmd}'. Try: imc-browse ctl help");
            return 1;
        }
    }
    match call(&req) {
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
        Ok(v) if v["error"].is_string() => {
            eprintln!("error: {}", v["error"].as_str().unwrap_or(""));
            1
        }
        Ok(v) => {
            if cmd == "screenshot" {
                let path = arg(1);
                if path.is_empty() {
                    eprintln!("usage: imc-browse ctl screenshot <file.png>");
                    return 1;
                }
                let png = b64_decode(v["png_base64"].as_str().unwrap_or(""));
                if png.is_empty() || std::fs::write(&path, &png).is_err() {
                    eprintln!("error: could not save the screenshot");
                    return 1;
                }
                println!("{}", json!({"ok": true, "file": path, "bytes": png.len()}));
            } else if cmd == "text" || cmd == "html" {
                print!("{}", v[cmd.as_str()].as_str().unwrap_or(""));
            } else {
                println!("{v}");
            }
            0
        }
    }
}

fn wait(secs: u64, tab: Option<u64>) -> i32 {
    let end = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < end {
        match call(&json!({"cmd": "tabs"})) {
            Ok(v) if v["tabs"].is_array() => {
                let loading = v["tabs"].as_array().unwrap().iter().any(|t| t["loading"] == true && (tab.is_none() && t["active"] == true || tab == t["id"].as_u64()));
                if !loading {
                    println!("{}", json!({"ok": true}));
                    return 0;
                }
            }
            Ok(v) => {
                eprintln!("error: {}", v["error"].as_str().unwrap_or("unexpected answer"));
                return 1;
            }
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    eprintln!("error: still loading after {secs} s");
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_decodes() {
        assert_eq!(b64_decode("aGVsbG8gd29ybGQ="), b"hello world");
        assert_eq!(b64_decode("YQ=="), b"a");
        assert!(b64_decode("!!!").is_empty());
    }

    #[test]
    fn audit_keeps_the_newest_200() {
        for i in 0..250 {
            audit(format!("x{i}"));
        }
        let l = audit_list();
        assert_eq!(l.len(), 200);
        assert_eq!(l.last().unwrap().1, "x249");
    }

    #[test]
    fn pipe_name_is_per_user() {
        assert!(pipe_name().starts_with(r"\\.\pipe\imc-browse-control-"));
    }
}
