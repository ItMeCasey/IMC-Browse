"""IMC Browse glyph pack: source of truth.

Run `python docs/glyphs/build.py` to regenerate:
  src/ui/glyphs.svg     sprite (<symbol id="g-NAME">)
  src/ui/glyphs.js      GLYPH map + gl(name, size) helper for chrome.html's innerHTML strings
  docs/glyphs/index.html  preview sheet

House style: 24x24 grid, centre-lines kept inside 3.5..20.5, one stroke weight (default 2.75,
about 1.7px at 15px and 2.1px at 18px; the old pack was 2 => 1.25px), round caps and joins,
square-ish geometry with 1-3 unit corner radii, solid dots for "ink" accents (eyes, keyholes, alerts).
Stroke weight comes from CSS (--gw), so the whole pack can be thickened or thinned in one place.
"""
import json, pathlib

ROOT = pathlib.Path(__file__).resolve().parents[2]
GW = 2.75
F = 'fill="currentColor" stroke="none"'      # solid ink
FS = 'fill="currentColor"'                    # solid body that keeps the outline weight

def dot(x, y, r=1.4): return f'<circle cx="{x}" cy="{y}" r="{r}" {F}/>'
def p(d, extra=""): return f'<path d="{d}"{(" " + extra) if extra else ""}/>'

STAR = "M12 3.5l2.6 5.5 6 .8-4.4 4.2 1.1 6-5.3-2.9-5.3 2.9 1.1-6L3.4 9.8l6-.8z"
SHIELD = "M12 3.5 19.5 6.5v5.5c0 4.5-3.2 7.5-7.5 9-4.3-1.5-7.5-4.5-7.5-9V6.5z"
SPEAKER = "M4 9.5v5h3.5l5 4v-13l-5 4z"
GLOBE = '<circle cx="12" cy="12" r="8.5"/><path d="M3.5 12h17"/><ellipse cx="12" cy="12" rx="3.6" ry="8.5"/>'
EYE = ('<path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z"/>'
       '<circle cx="12" cy="12" r="2.6"/>')
MAG = '<circle cx="10.5" cy="10.5" r="6.5"/><path d="M15.5 15.5l5 5"/>'

GROUPS = [
 ("Navigation", [
  ("back",       p("M19.5 12H5M11 5.5 4.5 12l6.5 6.5")),
  ("forward",    p("M4.5 12H19M13 5.5l6.5 6.5-6.5 6.5")),
  ("up",         p("M12 19.5V5M5.5 11 12 4.5 18.5 11")),
  ("reload",     p("M20 12a8 8 0 1 1-2.35-5.65L20 8.5M20 3.5v5h-5")),
  ("stop",       p("M6.5 6.5l11 11M17.5 6.5l-11 11")),
  ("home",       p("M3.5 11.5 12 4l8.5 7.5M6 10v9.5h12V10M10 19.5v-5h4v5")),
  ("loading",    p("M12 3.5a8.5 8.5 0 1 0 8.5 8.5")),
  ("search",     MAG),
  ("history",    '<circle cx="12" cy="12" r="8.5"/>' + p("M12 7v5l3.5 2")),
  ("external",   p("M13.5 4.5h6v6M19.5 4.5 11 13M17 14v4.5a1 1 0 0 1-1 1H5.5a1 1 0 0 1-1-1V8a1 1 0 0 1 1-1H10")),
 ]),
 ("Toolbar and tabs", [
  ("menu",       p("M4.5 6.5h15M4.5 12h15M4.5 17.5h15")),
  ("more",       dot(5, 12, 1.9) + dot(12, 12, 1.9) + dot(19, 12, 1.9)),
  ("plus",       p("M12 5v14M5 12h14")),
  ("minus",      p("M5 12h14")),
  ("close",      p("M6.5 6.5l11 11M17.5 6.5l-11 11")),
  ("check",      p("M4.5 12.5l5 5 10-11")),
  ("chev-down",  p("M6 9l6 6 6-6")),
  ("chev-up",    p("M6 15l6-6 6 6")),
  ("chev-left",  p("M15 6l-6 6 6 6")),
  ("chev-right", p("M9 6l6 6-6 6")),
  ("star",       p(STAR)),
  ("star-fill",  p(STAR, FS)),
  ("bookmarks",  p("M6.5 4.5a1 1 0 0 1 1-1h9a1 1 0 0 1 1 1v16l-5.5-4-5.5 4z")),
  ("sidebar",    '<rect x="3.5" y="4.5" width="17" height="15" rx="3.5"/>' + p("M9.5 4.5v15")),
  ("engine",     '<rect x="3.5" y="3.5" width="17" height="17" rx="5"/>' + '<circle cx="11" cy="11" r="3.6"/>' + p("M14 14l3 3")),
  ("sliders",    p("M4 7h8.5M17.5 7H20M4 17h2.5M11.5 17H20") + '<circle cx="15" cy="7" r="2.5"/><circle cx="9" cy="17" r="2.5"/>'),
  ("theme",      '<circle cx="12" cy="12" r="8.5"/>' + p("M12 3.5a8.5 8.5 0 0 1 0 17z", FS)),
  ("extension",  p("M4 8.5a1 1 0 0 1 1-1h3.5v-1a2.25 2.25 0 0 1 4.5 0v1h3.5a1 1 0 0 1 1 1V11h1a2.25 2.25 0 0 1 0 4.5h-1v4a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1z")),
 ]),
 ("Security and privacy", [
  ("shield",     p(SHIELD)),
  ("shield-ok",  p(SHIELD) + p("M8.8 12l2.4 2.4 4-4.4")),
  ("shield-off", p(SHIELD) + p("M4.5 4.5l15 15")),
  ("lock",       '<rect x="5" y="10.5" width="14" height="10" rx="3"/>' + p("M8 10.5V8a4 4 0 0 1 8 0v2.5") + dot(12, 15.5, 1.5)),
  ("unlock",     '<rect x="5" y="10.5" width="14" height="10" rx="3"/>' + p("M8 10.5V8a4 4 0 0 1 7.6-1.7") + dot(12, 15.5, 1.5)),
  ("ghost",      p("M12 3.5a6.5 6.5 0 0 0-6.5 6.5v10.5l3.25-2.2L12 20.5l3.25-2.2 3.25 2.2V10A6.5 6.5 0 0 0 12 3.5z") + dot(9.5, 10.5, 1.5) + dot(14.5, 10.5, 1.5)),
  ("globe",      GLOBE),
  ("globe-off",  GLOBE + p("M4.5 4.5l15 15")),
  ("power",      p("M12 3.5v8.5M7 6.5a7.5 7.5 0 1 0 10 0")),
  ("key",        '<circle cx="7.5" cy="16.5" r="3.5"/>' + p("M10 14 20 4M16 8l3 3")),
  ("eye",        EYE),
  ("eye-off",    EYE + p("M4.5 4.5l15 15")),
  ("block",      '<circle cx="12" cy="12" r="8.5"/>' + p("M6 6l12 12")),
 ]),
 ("Media and tab state", [
  ("sound",      p(SPEAKER, FS) + p("M16.5 9a4 4 0 0 1 0 6M19.5 6.5a8 8 0 0 1 0 11")),
  ("mute",       p(SPEAKER, FS) + p("M16.5 9.5l5 5M21.5 9.5l-5 5")),
  ("sleep",      p("M5 11h8l-8 9h8M15 3.5h5l-5 6h5")),
  ("tab-new",    '<rect x="3.5" y="4.5" width="17" height="15" rx="3.5"/>' + p("M12 9v6M9 12h6")),
  ("camera",     p("M3.5 8.5a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2h-8a2 2 0 0 1-2-2zM15.5 11l5-3v8l-5-3z")),
  ("mic",        '<rect x="9" y="3.5" width="6" height="11" rx="3"/>' + p("M5.5 11.5a6.5 6.5 0 0 0 13 0M12 18v2.5")),
  ("pin",        p("M12 21s-6.5-5.8-6.5-11a6.5 6.5 0 0 1 13 0c0 5.2-6.5 11-6.5 11z") + '<circle cx="12" cy="10" r="2.3"/>'),
  ("bell",       p("M6 17.5V11a6 6 0 0 1 12 0v6.5M4.5 17.5h15M10 21h4")),
 ]),
 ("Panel and data", [
  ("folder",     p("M3.5 18.5v-11A1.5 1.5 0 0 1 5 6h4.2l2 2.5H19a1.5 1.5 0 0 1 1.5 1.5v8.5A1.5 1.5 0 0 1 19 20H5a1.5 1.5 0 0 1-1.5-1.5z")),
  ("download",   p("M12 4v11M7 10.5l5 5 5-5M5 20h14")),
  ("upload",     p("M12 15V4M7 8.5l5-5 5 5M5 20h14")),
  ("enter",      p("M14 4.5h4.5A1.5 1.5 0 0 1 20 6v12a1.5 1.5 0 0 1-1.5 1.5H14M4 12h11M10.5 7.5 15 12l-4.5 4.5")),
  ("copy",       '<rect x="8.5" y="8.5" width="11" height="11" rx="2.5"/>' + p("M15.5 8.5V6.5a2 2 0 0 0-2-2h-7a2 2 0 0 0-2 2v7a2 2 0 0 0 2 2h2")),
  ("edit",       p("M4.5 19.5l1-4.5L16 4.5a2.1 2.1 0 0 1 3 3L8.5 18z")),
  ("trash",      p("M4.5 7h15M9.5 7V4.5h5V7M6.5 7l1 13h9l1-13M10 11v5M14 11v5")),
  ("zoom-in",    MAG + p("M8 10.5h5M10.5 8v5")),
  ("zoom-out",   MAG + p("M8 10.5h5")),
 ]),
 ("Tabs and window", [
  ("pin-tab",    p("M9 4.5h6M10 4.5v5L7 13h10l-3-3.5v-5M12 13v7")),
  ("undo",       p("M4.5 9h10a5 5 0 0 1 0 10H9M8.5 4.5 4 9l4.5 4.5")),
  ("tabs",       '<rect x="3.5" y="7.5" width="17" height="13" rx="3"/>' + p("M7.5 4h9")),
  ("window",     '<rect x="3.5" y="4.5" width="17" height="15" rx="3.5"/>' + p("M3.5 9.5h17")),
  ("user",       '<circle cx="12" cy="8.5" r="3.5"/>' + p("M4.5 20.5a7.5 7 0 0 1 15 0")),
  ("grip",       "".join(dot(x, y, 1.5) for x in (9, 15) for y in (6, 12, 18))),
  ("win-min",    p("M5.5 18h13")),
  ("win-max",    '<rect x="5" y="5" width="14" height="14" rx="2.5"/>'),
  ("win-restore", p("M9 8.5V7a2 2 0 0 1 2-2h6a2 2 0 0 1 2 2v6a2 2 0 0 1-2 2h-1.5") + '<rect x="5" y="9" width="10.5" height="10.5" rx="2"/>'),
 ]),
 ("Page actions", [
  ("fullscreen", p("M4.5 9V5.5a1 1 0 0 1 1-1H9M15 4.5h3.5a1 1 0 0 1 1 1V9M19.5 15v3.5a1 1 0 0 1-1 1H15M9 19.5H5.5a1 1 0 0 1-1-1V15")),
  ("print",      p("M7 8.5V4.5h10v4M7 17H5.5a1 1 0 0 1-1-1v-6a1 1 0 0 1 1-1h13a1 1 0 0 1 1 1v6a1 1 0 0 1-1 1H17M7 14h10v6H7z")),
  ("share",      '<circle cx="6" cy="12" r="2.5"/><circle cx="18" cy="6" r="2.5"/><circle cx="18" cy="18" r="2.5"/>' + p("M8.2 10.8l7.6-3.6M8.2 13.2l7.6 3.6")),
  ("link",       p("M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1")),
  ("reader",     '<rect x="5" y="3.5" width="14" height="17" rx="3"/>' + p("M9 9h6M9 13h6M9 17h3")),
  ("screenshot", p("M4.5 8V6.5a2 2 0 0 1 2-2H8M16 4.5h1.5a2 2 0 0 1 2 2V8M19.5 16v1.5a2 2 0 0 1-2 2H16M8 19.5H6.5a2 2 0 0 1-2-2V16") + '<circle cx="12" cy="12" r="2.5"/>'),
  ("pip",        '<rect x="3.5" y="5.5" width="17" height="13" rx="3"/><rect x="12" y="11.5" width="6" height="4.5" rx="1" fill="currentColor"/>'),
  ("code",       p("M8.5 7 3.5 12l5 5M15.5 7l5 5-5 5")),
  ("image",      '<rect x="3.5" y="4.5" width="17" height="15" rx="3"/>' + p("M4 17l5-5 4 4 2.5-2.5 4.5 4.5") + dot(15.5, 9.5, 1.4)),
 ]),
 ("Network and errors", [
  ("cookie",     p("M20.5 12.5A8.5 8.5 0 1 1 11.5 3.5a3.5 3.5 0 0 0 4 4 3 3 0 0 0 5 5z") + dot(9, 10, 1.2) + dot(10, 15.5, 1.2) + dot(15, 16, 1.2)),
  ("wifi",       p("M3.5 9.5a12.5 12.5 0 0 1 17 0M6.5 13a8 8 0 0 1 11 0M9.5 16.2a3.6 3.6 0 0 1 5 0") + dot(12, 19.5, 1.5)),
  ("wifi-off",   p("M3.5 9.5a12.5 12.5 0 0 1 17 0M6.5 13a8 8 0 0 1 11 0M9.5 16.2a3.6 3.6 0 0 1 5 0") + dot(12, 19.5, 1.5) + p("M4.5 4.5l15 15")),
  ("page-error", '<rect x="5" y="3.5" width="14" height="17" rx="3"/>' + p("M9.5 9.5l5 5M14.5 9.5l-5 5")),
 ]),
 ("Status", [
  ("info",       '<circle cx="12" cy="12" r="8.5"/>' + p("M12 11v5.5") + dot(12, 7.8, 1.4)),
  ("warning",    p("M12 3.8 21 19.5H3z") + p("M12 10v3.5") + dot(12, 16.6, 1.2)),
  ("ok",         '<circle cx="12" cy="12" r="8.5"/>' + p("M8 12.3l2.8 2.8 5.2-5.6")),
  ("error",      '<circle cx="12" cy="12" r="8.5"/>' + p("M9 9l6 6M15 9l-6 6")),
 ]),
]

ALL = [(n, b) for _, g in GROUPS for n, b in g]
assert len({n for n, _ in ALL}) == len(ALL), "duplicate glyph name"

# ---- sprite ----
sym = "\n".join(f'  <symbol id="g-{n}" viewBox="0 0 24 24">{b}</symbol>' for n, b in ALL)
sprite = (f'<svg xmlns="http://www.w3.org/2000/svg" style="display:none">\n'
          f'<!-- IMC Browse glyphs. Use: <svg class="g"><use href="#g-back"/></svg> -->\n{sym}\n</svg>\n')
(ROOT / "src/ui/glyphs.svg").write_text(sprite, encoding="utf-8")

# ---- js ----
js = ("// IMC Browse glyph pack (generated by docs/glyphs/build.py; do not edit).\n"
      "// gl('back', 18) -> inline <svg>. Colour = currentColor, weight = CSS var --gw (default 2.75).\n"
      f"const GLYPH={json.dumps(dict(ALL), separators=(',', ':'))};\n"
      "const gl=(n,s=18)=>'<svg class=\"g\" viewBox=\"0 0 24 24\" width=\"'+s+'\" height=\"'+s+'\">'+GLYPH[n]+'</svg>';\n"
      f"const GLYPH_CSS='.g{{display:block;fill:none;stroke:currentColor;stroke-width:var(--gw,{GW});stroke-linecap:round;stroke-linejoin:round}}';\n")
(ROOT / "src/ui/glyphs.js").write_text(js, encoding="utf-8")

# ---- sheet ----
cards = ""
for title, items in GROUPS:
    cards += f'<h2>{title}</h2><div class="grid">'
    for n, _ in items:
        cards += (f'<div class="card{" spin" if n == "loading" else ""}"><svg class="g big"><use href="#g-{n}"/></svg>'
                  f'<span>{n}</span></div>')
    cards += "</div>"

def row(names, size, extra=""):
    return "".join(f'<button class="b {extra}" title="{n}"><svg class="g" style="width:{size}px;height:{size}px"><use href="#g-{n}"/></svg></button>' for n in names)

sheet = f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>IMC Browse Glyphs</title>
<style>
:root{{--bg:#14161a;--bar:#1c1f25;--fg:#e6e8ec;--acc:#4da3ff;--ok:#3ecf8e;--bad:#ff5d5d;--gw:{GW};
 --dim:color-mix(in srgb,var(--fg),var(--bg) 42%);--line:color-mix(in srgb,var(--bg),var(--fg) 14%);
 --tab:color-mix(in srgb,var(--bar),var(--fg) 8%)}}
:root[data-theme=light]{{--bg:#e9ebef;--bar:#f6f7f9;--fg:#1b1d22;--acc:#1a73e8}}
*{{box-sizing:border-box}}
body{{margin:0;background:var(--bg);color:var(--fg);font:14px system-ui,Segoe UI,sans-serif;padding:24px 20px 60px}}
main{{max-width:980px;margin:0 auto}}
h1{{margin:0 0 4px;font-size:22px}} h2{{margin:30px 0 10px;font-size:13px;color:var(--dim);font-weight:600;letter-spacing:.04em;text-transform:uppercase}}
p.s{{margin:0;color:var(--dim)}}
.g{{width:1em;height:1em;display:block;fill:none;stroke:currentColor;stroke-width:var(--gw);stroke-linecap:round;stroke-linejoin:round}}
.ctl{{display:flex;gap:18px;flex-wrap:wrap;align-items:center;margin:18px 0;padding:10px 14px;background:var(--bar);border:1px solid var(--line);border-radius:10px}}
.ctl label{{display:flex;gap:8px;align-items:center;color:var(--dim)}} .ctl b{{color:var(--fg);min-width:2.6em;font-variant-numeric:tabular-nums}}
button.t{{border:1px solid var(--line);background:var(--tab);color:var(--fg);border-radius:6px;padding:4px 10px;cursor:pointer}}
.grid{{display:grid;grid-template-columns:repeat(auto-fill,minmax(112px,1fr));gap:8px}}
.card{{display:flex;flex-direction:column;align-items:center;gap:10px;padding:16px 6px 10px;background:var(--bar);border:1px solid var(--line);border-radius:10px}}
.card span{{font-size:11.5px;color:var(--dim)}}
.big{{width:32px;height:32px}}
.spin .big{{animation:sp 1s linear infinite}} @keyframes sp{{to{{transform:rotate(360deg)}}}}
.bar{{display:flex;align-items:center;gap:4px;padding:8px 10px;background:var(--bar);border-radius:10px;border:1px solid var(--line);flex-wrap:wrap}}
.b{{width:32px;height:32px;display:inline-flex;align-items:center;justify-content:center;border:0;border-radius:6px;background:none;color:var(--dim);cursor:pointer}}
.b:hover{{background:color-mix(in srgb,var(--fg),transparent 85%);color:var(--fg)}}
.b.acc{{color:var(--acc)}} .b.ok{{color:var(--ok)}} .b.bad{{color:var(--bad)}}
.url{{flex:1;min-width:160px;height:32px;border-radius:16px;background:var(--tab);display:flex;align-items:center;gap:8px;padding:0 12px;color:var(--dim)}}
.pill{{height:28px;padding:0 10px;border-radius:14px;background:var(--tab);display:inline-flex;align-items:center;gap:6px}}
.sizes{{display:flex;gap:22px;align-items:flex-end;flex-wrap:wrap;padding:14px;background:var(--bar);border:1px solid var(--line);border-radius:10px}}
.sizes div{{display:flex;flex-direction:column;align-items:center;gap:6px;font-size:11px;color:var(--dim)}}
.cmp{{display:grid;grid-template-columns:1fr 1fr;gap:10px}} .cmp .bar{{flex-direction:column;align-items:flex-start}}
.cmp small{{color:var(--dim)}}
</style></head><body>
{sprite}
<main>
<h1>IMC Browse Glyphs</h1>
<p class="s">{len(ALL)} glyphs, 24 grid, round caps, one weight. Weight is the CSS variable <code>--gw</code>.</p>

<div class="ctl">
 <label>Weight <input id="w" type="range" min="1.5" max="3.75" step="0.05" value="{GW}"><b id="wv">{GW}</b></label>
 <label>Size <input id="z" type="range" min="12" max="48" step="1" value="32"><b id="zv">32</b></label>
 <button class="t" id="th">Light / dark</button>
</div>

<h2>In the toolbar (actual size: 18px)</h2>
<div class="bar">
 {row(["back","forward","reload"], 18)}
 <div class="url"><svg class="g" style="width:15px;height:15px;color:var(--ok)"><use href="#g-lock"/></svg>example.com/page</div>
 {row(["star"], 18, "acc")}{row(["search"], 18)}
 <span class="pill" style="color:var(--ok)"><svg class="g" style="width:16px;height:16px"><use href="#g-shield-ok"/></svg>142</span>
 <span class="pill" style="color:var(--dim)"><svg class="g" style="width:16px;height:16px"><use href="#g-globe-off"/></svg>VPN off</span>
 <span class="pill" style="color:var(--bad)"><svg class="g" style="width:16px;height:16px"><use href="#g-globe-off"/></svg>VPN down</span>
 {row(["menu"], 18)}
</div>
<div class="bar" style="margin-top:8px">
 <span class="pill"><svg class="g" style="width:15px;height:15px;color:#9b7bff"><use href="#g-ghost"/></svg>Ghost tab<svg class="g" style="width:14px;height:14px"><use href="#g-close"/></svg></span>
 <span class="pill"><svg class="g" style="width:15px;height:15px;color:var(--acc)"><use href="#g-sound"/></svg>Playing<svg class="g" style="width:14px;height:14px"><use href="#g-close"/></svg></span>
 <span class="pill"><svg class="g" style="width:15px;height:15px"><use href="#g-mute"/></svg>Muted<svg class="g" style="width:14px;height:14px"><use href="#g-close"/></svg></span>
 <span class="pill"><svg class="g" style="width:15px;height:15px;color:var(--acc)"><use href="#g-sleep"/></svg>Hibernating</span>
 {row(["plus"], 18)}
</div>

<h2>Old pack (2.0) vs this pack ({GW}) at 15px and 18px</h2>
<div class="cmp">
 <div class="bar"><small>2.0</small><div style="display:flex;gap:6px">
  {"".join(f'<svg class="g" style="--gw:2;width:{s}px;height:{s}px"><use href="#g-{n}"/></svg>' for s in (15,18) for n in ("sound","mute","star","back","close"))}</div></div>
 <div class="bar"><small>{GW}</small><div style="display:flex;gap:6px">
  {"".join(f'<svg class="g" style="width:{s}px;height:{s}px"><use href="#g-{n}"/></svg>' for s in (15,18) for n in ("sound","mute","star","back","close"))}</div></div>
</div>

<h2>Sizes</h2>
<div class="sizes">{"".join(f'<div><svg class="g" style="width:{s}px;height:{s}px"><use href="#g-ghost"/></svg>{s}</div>' for s in (12,14,16,18,20,24,32,48))}</div>

{cards}
</main>
<script>
const r=document.documentElement,$=id=>document.getElementById(id);
$('w').oninput=e=>{{r.style.setProperty('--gw',e.target.value);$('wv').textContent=e.target.value}};
$('z').oninput=e=>{{document.querySelectorAll('.big').forEach(g=>g.style.cssText='width:'+e.target.value+'px;height:'+e.target.value+'px');$('zv').textContent=e.target.value}};
$('th').onclick=()=>{{r.dataset.theme=r.dataset.theme==='light'?'dark':'light'}};
</script></body></html>
"""
(ROOT / "docs/glyphs/index.html").write_text(sheet, encoding="utf-8")
print(f"{len(ALL)} glyphs")

# ---- one file per glyph + index.json (src/ui/glyphs/) ----
USE = {
 "back": "Back button", "forward": "Forward button", "up": "Scroll/step up, parent folder", "reload": "Reload button",
 "stop": "Stop loading (reload swaps to this while a page loads)", "home": "Home page", "loading": "Spinner; rotate with CSS",
 "search": "Search / find", "history": "History", "external": "Open in new window / external link",
 "menu": "Main menu (hamburger)", "more": "Overflow menu", "plus": "New tab (+)", "minus": "Zoom out / collapse",
 "close": "Tab close x, dialog close", "check": "Selected item (engine picker tick)",
 "chev-down": "Dropdown open", "chev-up": "Collapse", "chev-left": "Previous", "chev-right": "Next / expand",
 "star": "Bookmark, not saved (outline)", "star-fill": "Bookmark, saved (filled)", "bookmarks": "Bookmarks panel",
 "sidebar": "Vertical tabs toggle", "engine": "Search engine placeholder (use until a real engine badge is shown)",
 "sliders": "Settings", "theme": "Theme / appearance", "extension": "Extensions",
 "shield": "Tracker/ad blocker on (neutral)", "shield-ok": "Blocker on, active", "shield-off": "Blocker off",
 "lock": "Secure HTTPS page", "unlock": "Not secure page", "ghost": "Ghost (private) tab", "globe": "Proxy/VPN on (also network)",
 "globe-off": "VPN off or down", "power": "Kill switch", "key": "Passwords", "eye": "Show password", "eye-off": "Hide password",
 "block": "Blocked / denied permission",
 "sound": "Tab playing audio (speaker)", "mute": "Tab muted", "sleep": "Hibernated tab (Zzz)", "tab-new": "New tab (boxed variant)",
 "camera": "Camera permission", "mic": "Microphone permission", "pin": "Location permission", "bell": "Notifications permission",
 "folder": "Bookmark folder", "download": "Downloads / import", "upload": "Export", "enter": "Autofill / sign in",
 "copy": "Copy", "edit": "Edit / rename", "trash": "Delete", "zoom-in": "Zoom in", "zoom-out": "Zoom out",
 "pin-tab": "Pin tab", "undo": "Reopen closed tab / undo", "tabs": "Tab list / tab search", "window": "New window",
 "user": "Profile", "grip": "Drag handle (reorder tabs/bookmarks)", "win-min": "Minimize window", "win-max": "Maximize window",
 "win-restore": "Restore window", "fullscreen": "Fullscreen", "print": "Print", "share": "Share page", "link": "Copy link",
 "reader": "Reader mode", "screenshot": "Screenshot", "pip": "Picture-in-picture", "code": "Developer tools / view source",
 "image": "Image / save image", "cookie": "Cookies", "wifi": "Online", "wifi-off": "Offline / no connection",
 "page-error": "Page failed to load",
 "info": "Information", "warning": "Warning", "ok": "Success", "error": "Error",
}
ALIASES = {"tab-close": "close", "settings": "sliders", "vertical-tabs": "sidebar", "downloads": "download",
           "passwords": "key", "vpn": "globe", "speaker": "sound", "speaker-muted": "mute", "sleeping": "sleep",
           "new-tab": "plus", "search-engine": "engine", "shield-on": "shield-ok", "bookmark": "star", "duplicate-tab": "copy", "new-window": "window", "reopen-closed-tab": "undo",
           "clear-data": "trash", "find": "search", "devtools": "code", "offline": "wifi-off", "load-error": "page-error",
           "certificate": "shield-ok", "https-upgraded": "lock", "minimize": "win-min", "maximize": "win-max",
           "restore": "win-restore", "profile": "user","bookmark-saved": "star-fill"}
gd = ROOT / "src/ui/glyphs"
gd.mkdir(exist_ok=True)
for old in gd.glob("*.svg"): old.unlink()
head = ('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" '
        f'stroke-width="{GW}" stroke-linecap="round" stroke-linejoin="round">')
body = dict(ALL)
index = {"viewBox": "24 24", "strokeWidth": GW, "color": "currentColor", "glyphs": {}, "aliases": ALIASES}
for n, _ in ALL:
    assert n in USE, n
    (gd / f"{n}.svg").write_text(head + body[n] + "</svg>\n", encoding="utf-8")
    index["glyphs"][n] = USE[n]
for a, t in ALIASES.items(): assert t in body, a
(gd / "index.json").write_text(json.dumps(index, indent=1), encoding="utf-8")
print("files:", len(list(gd.glob("*.svg"))))
