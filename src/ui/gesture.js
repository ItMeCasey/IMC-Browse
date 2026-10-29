// Mouse gestures: hold the right button, drag a path (U/D/L/R), release. Injected into every page
// by the host; the host turns the reported path into the user's configured action.
// Config arrives as window.__imcGset({enabled, min, trail, map:{PATH: "Action label"}}).
(() => {
  if (window.__imcG) return;
  const G = (window.__imcG = { cfg: { enabled: true, min: 28, trail: true, map: {} } });
  window.__imcGset = (c) => { Object.assign(G.cfg, c); };
  const send = (p) => {
    try {
      if (window.ipc && window.ipc.postMessage) window.ipc.postMessage('IMCG:' + p);
      else console.debug('IMCGESTURE:' + p);
    } catch (e) {}
  };
  const ARROW = { U: '↑', D: '↓', L: '←', R: '→' };
  let down = false, x0 = 0, y0 = 0, path = '', pts = [], cv = null, hud = null, suppress = false;

  const clear = () => {
    if (cv) { cv.remove(); cv = null; }
    if (hud) { hud.remove(); hud = null; }
  };
  const show = () => {
    const host = document.documentElement;
    if (G.cfg.trail) {
      if (!cv) {
        cv = document.createElement('canvas');
        cv.style.cssText = 'position:fixed;left:0;top:0;width:100vw;height:100vh;pointer-events:none;z-index:2147483647';
        cv.width = innerWidth; cv.height = innerHeight;
        host.appendChild(cv);
      }
      const c = cv.getContext('2d');
      c.clearRect(0, 0, cv.width, cv.height);
      c.lineWidth = 4; c.lineCap = 'round'; c.lineJoin = 'round'; c.strokeStyle = 'rgba(77,163,255,.9)';
      c.beginPath();
      pts.forEach((p, i) => (i ? c.lineTo(p[0], p[1]) : c.moveTo(p[0], p[1])));
      c.stroke();
    }
    if (!hud) {
      hud = document.createElement('div');
      hud.style.cssText = 'position:fixed;left:50%;bottom:36px;transform:translateX(-50%);padding:8px 16px;border-radius:10px;background:rgba(20,22,26,.92);color:#fff;font:600 15px system-ui,Segoe UI,sans-serif;z-index:2147483647;pointer-events:none;letter-spacing:2px';
      host.appendChild(hud);
    }
    const label = G.cfg.map[path];
    hud.textContent = [...path].map((d) => ARROW[d]).join(' ') + (label ? '   ' + label : '   (no action)');
    hud.style.opacity = label ? '1' : '.6';
  };

  addEventListener('mousedown', (e) => {
    if (e.button !== 2 || !G.cfg.enabled) return;
    down = true; x0 = e.clientX; y0 = e.clientY; path = ''; pts = [[x0, y0]];
  }, true);
  addEventListener('mousemove', (e) => {
    if (!down) return;
    if (!(e.buttons & 2)) { down = false; clear(); return; }
    const dx = e.clientX - x0, dy = e.clientY - y0;
    if (Math.hypot(dx, dy) >= G.cfg.min) {
      const d = Math.abs(dx) > Math.abs(dy) ? (dx > 0 ? 'R' : 'L') : (dy > 0 ? 'D' : 'U');
      if (path[path.length - 1] !== d && path.length < 8) path += d;
      x0 = e.clientX; y0 = e.clientY;
    }
    pts.push([e.clientX, e.clientY]);
    if (path) show();
  }, true);
  addEventListener('mouseup', (e) => {
    if (e.button !== 2 || !down) return;
    down = false; clear();
    if (path) { suppress = true; setTimeout(() => { suppress = false; }, 400); send(path); }
  }, true);
  // A real gesture must not also open the page's context menu; a plain right-click still does.
  addEventListener('contextmenu', (e) => { if (suppress) { e.preventDefault(); e.stopPropagation(); } }, true);
})();

// Zoom: Ctrl+wheel and Ctrl +/-/0 are reported to the host, which owns the zoom level
// (remembered per site and shown in the address bar).
(() => {
  if (window.__imcZ) return;
  window.__imcZ = 1;
  const send = (d) => {
    try {
      if (window.ipc && window.ipc.postMessage) window.ipc.postMessage('IMCZ:' + d);
      else console.debug('IMCZOOM:' + d);
    } catch (e) {}
  };
  let acc = 0;
  addEventListener('wheel', (e) => {
    if (!e.ctrlKey) return;
    e.preventDefault(); e.stopPropagation();
    acc += e.deltaY;
    // one mouse-wheel notch is about 100; trackpad pinch sends many small deltas
    if (Math.abs(acc) >= 40) { send(acc < 0 ? 'in' : 'out'); acc = 0; }
  }, { passive: false, capture: true });
  addEventListener('keydown', (e) => {
    if (!e.ctrlKey || e.altKey) return;
    const k = e.key;
    if (k === '=' || k === '+') send('in');
    else if (k === '-' || k === '_') send('out');
    else if (k === '0') send('reset');
    else return;
    e.preventDefault(); e.stopPropagation();
  }, true);
})();

// Link hover bubble (bottom-left, full URL). WebView2 draws its own status bubble, so this only
// runs where the engine has none (the CEF build).
(() => {
  if (window.__imcB || (window.chrome && window.chrome.webview)) return;
  window.__imcB = 1;
  let el = null;
  const hide = () => { if (el) { el.remove(); el = null; } };
  const show = (url) => {
    if (!el) {
      el = document.createElement('div');
      el.style.cssText = 'position:fixed;left:0;bottom:0;max-width:62vw;padding:4px 12px 5px 10px;border-radius:0 8px 0 0;background:rgba(28,31,37,.96);color:#e6e8ec;border:1px solid rgba(255,255,255,.12);border-left:0;border-bottom:0;font:12px/1.35 system-ui,Segoe UI,sans-serif;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;z-index:2147483647;pointer-events:none';
      document.documentElement.appendChild(el);
    }
    // long tracking links: keep the start (the site) and the end, drop the middle
    el.textContent = url.length > 110 ? url.slice(0, 75) + ' … ' + url.slice(-25) : url;
  };
  const link = (t) => (t && t.closest ? t.closest('a[href]') : null);
  addEventListener('mouseover', (e) => {
    const a = link(e.target);
    if (a && a.href && !/^javascript:/i.test(a.href)) show(a.href); else hide();
  }, true);
  addEventListener('mouseout', (e) => { if (!link(e.relatedTarget)) hide(); }, true);
  // like Chrome: step aside when the pointer heads for the bubble
  addEventListener('mousemove', (e) => {
    if (!el) return;
    const near = innerHeight - e.clientY < el.offsetHeight + 24 && e.clientX < el.offsetWidth + 30;
    el.style.left = near ? 'auto' : '0'; el.style.right = near ? '0' : 'auto';
    el.style.borderRadius = near ? '8px 0 0 0' : '0 8px 0 0';
  }, true);
})();

// Page icon: the engine does not announce it for these windows, so find <link rel="icon"> here and
// report the url (the host downloads it through the tab's own network context).
(() => {
  if (window.__imcF || window.top !== window) return;
  window.__imcF = 1;
  let last = '';
  const find = () => {
    const links = [...document.querySelectorAll('link[rel~="icon"], link[rel="shortcut icon"]')];
    const pick = links.find((l) => /(^|\s)(32|48|64)x/.test((l.sizes && l.sizes.value) || '')) || links.find((l) => !l.sizes || !l.sizes.value) || links[0];
    const u = pick ? pick.href : (/^https?:$/.test(location.protocol) ? location.origin + '/favicon.ico' : '');
    if (u && u !== last) { last = u; console.debug('IMCFAV:' + u); }
  };
  addEventListener('DOMContentLoaded', () => {
    find();
    if (document.head) new MutationObserver(find).observe(document.head, { childList: true, subtree: true, attributes: true, attributeFilter: ['href', 'rel'] });
  });
  addEventListener('load', find);
})();

// "Send page to the AI assistant": the host asks for this and gets the selection (or the visible page
// text), capped in size. Nothing is read or sent unless the user clicked the button.
window.__imcPageText = () => {
  const sel = String(getSelection() || '').trim();
  const text = (sel || (document.body && document.body.innerText) || '').slice(0, 30000);
  console.debug('IMCTXT:' + JSON.stringify({ selected: !!sel, title: document.title, url: location.href, text }));
};
