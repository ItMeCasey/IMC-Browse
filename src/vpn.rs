//! Proxy/"VPN" client: routes all browser traffic through a SOCKS5 or HTTP
//! proxy, fails closed (kill switch) and supports split tunnelling.
//!
//! This is a *proxy* client, not a WireGuard tunnel: it needs an endpoint you
//! provide (your own server, or a provider's SOCKS5/HTTPS proxy). No servers
//! are bundled and no "zero-log" claim can be made for third-party nodes.

use crate::config::Vpn;
use std::{
    net::{TcpStream, ToSocketAddrs},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

/// Set by the probe thread; checked by the request hook to fail closed.
pub static TUNNEL_DOWN: AtomicBool = AtomicBool::new(false);

/// Chromium switches: proxy, split-tunnel bypass list, no WebRTC IP leak.
pub fn browser_args(v: &Vpn) -> String {
    if !v.enabled {
        return String::new();
    }
    let scheme = if v.kind == "http" { "http" } else { "socks5" };
    let mut s = format!(" --proxy-server={scheme}://{}:{} --force-webrtc-ip-handling-policy=disable_non_proxied_udp", v.host, v.port);
    if !v.bypass.is_empty() {
        // Chromium splits the list on ';'; quote it so the argument stays one token.
        s.push_str(&format!(" --proxy-bypass-list=\"{}\"", v.bypass.join(";")));
    }
    s
}

pub fn reachable(v: &Vpn) -> bool {
    (v.host.as_str(), v.port)
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
        .is_some_and(|a| TcpStream::connect_timeout(&a, Duration::from_secs(3)).is_ok())
}

/// Probes the proxy every few seconds; flips [`TUNNEL_DOWN`] when the kill switch applies.
pub fn start_watchdog(v: Vpn) {
    if !v.enabled || !v.kill_switch {
        return;
    }
    TUNNEL_DOWN.store(!reachable(&v), Ordering::SeqCst);
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(4));
        TUNNEL_DOWN.store(!reachable(&v), Ordering::SeqCst);
    });
}
