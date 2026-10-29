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

// Extension popups: the engine's tabs API does not know this browser's tabs (tabs.query never answers), so
// a popup opened from the toolbar is given the page it was opened on and answers tabs.query with it.
(() => {
  if (location.protocol !== 'chrome-extension:' || window.top !== window) return;
  const m = /[?&]__imc=([^&#]+)/.exec(location.search);
  if (!m) return;
  let t;
  try { t = JSON.parse(decodeURIComponent(m[1])); } catch (e) { return; }
  const tab = { id: t.id, index: 0, windowId: 1, active: true, highlighted: true, pinned: false, incognito: false, selected: true,
    url: t.url, title: t.title, favIconUrl: t.fav || '', status: 'complete', discarded: false, autoDiscardable: true, groupId: -1, width: 1280, height: 800 };
  const win = { id: 1, focused: true, incognito: false, type: 'normal', state: 'normal', alwaysOnTop: false, left: 0, top: 0, width: 1280, height: 800, tabs: [tab] };
  const reply = (v) => (cb) => { if (typeof cb === 'function') { setTimeout(() => cb(v), 0); return undefined; } return Promise.resolve(v); };
  const set = (o, k, f) => { try { o[k] = f; } catch (e) { try { Object.defineProperty(o, k, { value: f, configurable: true, writable: true }); } catch (e2) {} } };
  // the engine adds chrome.* after this script first runs, so keep trying until it is there
  let done = false;
  const install = () => {
    const c = window.chrome;
    if (done || !c || !c.tabs) return;
    done = true;
    set(c.tabs, 'query', (info, cb) => (typeof info === 'function' ? reply([tab])(info) : reply([tab])(cb)));
    set(c.tabs, 'get', (id, cb) => reply(tab)(cb));
    set(c.tabs, 'getCurrent', (cb) => reply(undefined)(cb));
    if (c.windows) {
      set(c.windows, 'getCurrent', (a, b) => reply(win)(typeof a === 'function' ? a : b));
      set(c.windows, 'getLastFocused', (a, b) => reply(win)(typeof a === 'function' ? a : b));
    }
  };
  install();
  if (!done) {
    const id = setInterval(() => { install(); if (done) clearInterval(id); }, 2);
    setTimeout(() => clearInterval(id), 4000);
    document.addEventListener('readystatechange', install);
  }
})();

// Extension popups: report the size of the content so the panel can fit it, and close on Escape.
(() => {
  if (location.protocol !== 'chrome-extension:' || window.top !== window) return;
  let last = '';
  const hideX = () => { if (document.documentElement) document.documentElement.style.overflowX = 'hidden'; };
  hideX(); document.addEventListener('DOMContentLoaded', hideX);
  const rep = () => {
    try { rep2(); } catch (e) {}
  };
  const rep2 = () => {
    const d = document.documentElement, b = document.body;
    const w = Math.ceil(Math.max(d ? d.scrollWidth : 0, b ? b.scrollWidth : 0));
    let h = Math.ceil(Math.max(d ? d.scrollHeight : 0, b ? b.scrollHeight : 0));
    // pages that scroll inside their own container (many popups do): count what is hidden in there too
    let extra = 0, bottom = 0;
    if (b) {
      let n = 0;
      for (const el of b.querySelectorAll('*')) {
        if (++n > 3000) break;
        const r = el.getBoundingClientRect();
        if (r.height > 0 && r.bottom + scrollY > bottom) bottom = r.bottom + scrollY;
        if (el.scrollHeight > el.clientHeight + 1 && el.clientHeight > 0) {
          const o = getComputedStyle(el).overflowY;
          if (o === 'auto' || o === 'scroll') extra = Math.max(extra, el.scrollHeight - el.clientHeight);
        }
      }
    }
    h = Math.max(h + extra, Math.ceil(bottom));
    const k = w + ',' + h;
    if (k !== last) { last = k; console.debug('IMCPOPSIZE:' + k); }
  };
  window.__imcRep = rep; // the browser calls this a few times a second while the popup is open (timers in this page do not run reliably)
  addEventListener('load', rep);
  if (window.ResizeObserver) new ResizeObserver(rep).observe(document.documentElement);
  setInterval(rep, 400);
  addEventListener('keydown', (e) => { if (e.key === 'Escape') console.debug('IMCPOPCLOSE'); }, true);
})();

// Extension stores: their own "Add to Chrome / Firefox" buttons need the real Chrome or Firefox, so on a
// store page we turn those buttons into "Add to IMC Browse", which asks the browser to install that extension.
(() => {
  const h = location.hostname;
  if (window.top !== window || (h !== 'chromewebstore.google.com' && h !== 'addons.mozilla.org')) return;
  const isDetail = () => (h === 'chromewebstore.google.com' ? /\/detail\//.test(location.pathname) : /\/addon\//.test(location.pathname));
  const WANT = /^(add to (chrome|firefox|brave|edge)|switch to chrome|download firefox and get the extension|get firefox)$/i;
  const GONE = /^remove from chrome$/i;
  let fallback = null;
  const ask = (e) => {
    e.preventDefault(); e.stopImmediatePropagation(); e.stopPropagation();
    console.debug('IMCEXT:' + location.href);
    const t = e.currentTarget; const old = t.dataset.imcText;
    setLabel(t, 'Installing...');
    setTimeout(() => setLabel(t, old || 'Add to IMC Browse'), 3500);
  };
  const setLabel = (el, text) => {
    // keep the button's own icons: change only the deepest text
    const w = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    let n, last = null;
    while ((n = w.nextNode())) if (n.nodeValue.trim()) last = n;
    if (last) last.nodeValue = text; else el.textContent = text;
  };
  const scan = () => {
    if (!isDetail()) { if (fallback) { fallback.remove(); fallback = null; } return; }
    let found = false;
    for (const el of document.querySelectorAll('button, a, [role="button"]')) {
      const txt = (el.innerText || '').trim().replace(/\s+/g, ' ');
      if (el.dataset.imc) { found = true; continue; }
      if (GONE.test(txt)) { el.style.display = 'none'; continue; }
      if (!WANT.test(txt)) continue;
      el.dataset.imc = '1'; el.dataset.imcText = 'Add to IMC Browse';
      el.removeAttribute('href'); el.removeAttribute('download');
      setLabel(el, 'Add to IMC Browse');
      el.addEventListener('click', ask, true);
      found = true;
    }
    // the store changed its markup: still offer a button
    if (!found && !fallback && document.readyState === 'complete' && performance.now() > 6000) {
      fallback = document.createElement('button');
      fallback.textContent = 'Add to IMC Browse';
      fallback.style.cssText = 'position:fixed;top:76px;right:20px;z-index:2147483647;padding:10px 18px;border:0;border-radius:10px;background:#4da3ff;color:#06121f;font:600 14px system-ui,Segoe UI,sans-serif;cursor:pointer;box-shadow:0 6px 20px rgba(0,0,0,.35)';
      fallback.onclick = () => { console.debug('IMCEXT:' + location.href); };
      document.documentElement.appendChild(fallback);
    } else if (found && fallback) { fallback.remove(); fallback = null; }
  };
  setInterval(scan, 500);
  scan();
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
