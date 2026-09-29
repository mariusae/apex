//! Web windows on the client (WEB.md §2): a native web view (`wry`,
//! WebKit) as a child view of the gpui window, placed over the body
//! rectangle the layout gives a `Body::Web` window, hidden while a gpui
//! overlay (the tools menu, the finder, the picker) would be painted
//! under it. The page's navigations come back as events the app turns
//! into `WebNavigate` proposals, so the window's name follows the page.
//!
//! The page's traffic goes through the session's host (§2.3): a
//! localhost `CONNECT` proxy whose tunnels are streams on the I/O
//! plane, which the view's data store is pointed at. `apexfile:///path`
//! is a file on the host (§2.4), fetched on the plane and watched, so
//! the page reloads when it changes. Without a link (an in-process
//! server) the view uses its own network and reads files directly.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{Bounds, Pixels, Point, Window};

use apex_core::WindowId;
use apex_server::plane::{alias_loopback_url, mime_for, start_connect_proxy, unalias_url, IoPlane};
use apex_server::proto::{file_url_path, FileFrame, IoFrame};
use apex_server::remote::{file_url, Wake};

pub enum WebEvent {
    /// The page went to `url` (a link, a redirect, a form).
    Navigated(String),
    /// The CSS cursor under the pointer changed (`pointer` over a link,
    /// `text`, `default`...): the page's script says, since WebKit's own
    /// cursor never reaches the screen inside this window.
    Cursor(String),
    /// The document's title changed (not kept yet: WEB.md §2.1).
    #[allow(dead_code)]
    Title(String),
    /// A host file the page uses changed: load it again.
    Reload,
    /// A link followed in a page rendered from a buffer: open it.
    Link(String),
    /// A page went for the host's loopback by its bare name, which the
    /// view would take for the client's: load it under the alias instead.
    Reroute(String),
    /// A `file://` link with a line (`?line=N`): the file in a text
    /// window, at that line.
    Open(String, Option<usize>),
    /// The page started (true) or finished loading.
    Loading(bool),
    /// A button went down in the page (a click there, which gpui does not
    /// see: the toasts go, as for a click anywhere off them).
    Down,
    /// A code block's copy handle was clicked: its text, for the snarf
    /// buffer and the clipboard.
    Copy(String),
    /// A page from a buffer has mermaid blocks to draw and no mermaid:
    /// the client's copy is given to it.
    Mermaid,
    /// Where the page is scrolled, as the scrollbar beside it shows it:
    /// how far down (`top`), how long the page is, and how much shows,
    /// in CSS pixels.
    Scroll { top: f64, height: f64, view: f64 },
}

/// The theme, as a page rendered from a buffer sees it: the editor's
/// colours as CSS variables, and the copy handle on code blocks.
fn theme_css() -> String {
    let t = crate::theme::theme();
    let hex = |c: u32| format!("#{c:06X}");
    let (code_bg, rule, dim) = if crate::theme::is_dark() { (0x2A2A2C, 0x3A3A3C, t.text_dim) } else { (0xF0F0EE, 0xDCDCD9, t.text_dim) };
    let link = if crate::theme::is_dark() { t.panel_accent } else { t.dirty };
    // a diff's added and removed lines (apex diff): pale, one tint each.
    // Removed is orange rather than red, which stays apart from the green
    // for a reader with deuteranopia where red does not (under a
    // simulation, 14 apart in CIELAB on light and 16 on dark), and each
    // still clears the paper for that reader (8 and 12 on light, 11 and
    // 18 on dark)
    // on the modern-mac branch, GitHub Colorblind's diff lines: added in
    // its blue (its green scale is blue), removed in its orange
    let (add, del) = (t.diff_add, t.diff_del);
    let diff = format!(":root{{--apex-add:{};--apex-del:{}}}", hex(add), hex(del));
    // the font set's faces and families (View ▸ Font), for a page's
    // stylesheet to set itself in
    let accent = format!(":root{{--apex-accent:{}}}", hex(t.accent));
    diff + &accent + &crate::fonts::page_css() + &format!(
        ":root{{--apex-bg:{};--apex-fg:{};--apex-code-bg:{};--apex-rule:{};--apex-border:{};--apex-link:{};--apex-sel:{};--apex-dim:{};--apex-tag-bg:{}}}\
         html{{background:{}}}\
         .apex-copy{{position:absolute;top:4px;right:4px;font:11px var(--apex-font);color:{};background:{};border:1px solid {};border-radius:4px;padding:1px 6px;cursor:pointer;opacity:0;transition:opacity .15s}}\
         pre:hover .apex-copy,.apex-copy:focus{{opacity:1}}",
        hex(t.body_bg), hex(t.text), hex(code_bg), hex(rule), hex(t.body_border), hex(link), hex(t.body_sel), hex(dim), hex(t.tag_bg),
        hex(t.body_bg), hex(dim), hex(t.body_bg), hex(rule)
    )
}

/// Look in a page (`Webs::find`): the next place, found as the browser
/// finds (whole words or not, any case), and every place the text is,
/// marked by CSS custom highlights -- which leave the page's own DOM
/// alone, so a preview's morph neither loses them nor trips on them. The
/// page's selection, which the found place is, goes clear while they are
/// up so that it does not paint over the mark; a click or a key takes
/// them down.
const LOOK_SCRIPT: &str = r#"(function () {
  let t = __TEXT__;
  if (!t) t = String(window.getSelection());
  if (!t) return;
  window.find(t, false, __REVERSE__, true, false, true, false);
  if (!(window.CSS && CSS.highlights && window.Highlight)) return;
  if (!window.__apexLook) {
    window.__apexLook = { sheet: new CSSStyleSheet() };
    document.adoptedStyleSheets = [...document.adoptedStyleSheets, window.__apexLook.sheet];
    const down = function () {
      CSS.highlights.delete('apex-look');
      CSS.highlights.delete('apex-look-here');
      window.__apexLook.sheet.replaceSync('');
    };
    addEventListener('mousedown', down, true);
    addEventListener('keydown', down, true);
  }
  window.__apexLook.sheet.replaceSync(
    '::highlight(apex-look){background-color:__ELSE__}' +
    '::highlight(apex-look-here){background-color:__HERE__}' +
    '::selection{background-color:transparent}');
  const all = new Highlight(), here = new Highlight();
  here.priority = 1;
  const sel = window.getSelection();
  if (sel.rangeCount && !sel.isCollapsed) here.add(sel.getRangeAt(0).cloneRange());
  // the page's text in order, each node where it starts in the whole, so
  // that a place split by markup (a bold half) is found as one; a block's
  // edge is a break, so that none runs on into the next
  const nodes = [], starts = [];
  let whole = '', block = null;
  const blockOf = function (e) {
    for (; e; e = e.parentElement) {
      const d = getComputedStyle(e).display;
      if (d !== 'inline' && d !== 'contents') return e;
    }
    return null;
  };
  const walk = document.createTreeWalker(document.body || document.documentElement, NodeFilter.SHOW_TEXT, {
    acceptNode: function (x) {
      const p = x.parentElement;
      return p && p.closest('script,style,noscript,textarea,.apex-copy') ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT;
    }
  });
  for (let x = walk.nextNode(); x; x = walk.nextNode()) {
    const b = blockOf(x.parentElement);
    if (b !== block) { whole += '\n'; block = b; }
    nodes.push(x); starts.push(whole.length);
    whole += x.data;
  }
  const at = function (i) {
    let lo = 0, hi = nodes.length - 1;
    while (lo < hi) { const m = (lo + hi + 1) >> 1; if (starts[m] <= i) lo = m; else hi = m - 1; }
    return lo;
  };
  const want = t.toLowerCase(), n = want.length, text = whole.toLowerCase();
  let count = 0;
  for (let i = text.indexOf(want); i >= 0 && count < 5000; i = text.indexOf(want, i + n)) {
    const a = at(i), b = at(i + n - 1);
    const r = document.createRange();
    r.setStart(nodes[a], i - starts[a]);
    r.setEnd(nodes[b], i + n - starts[b]);
    all.add(r);
    count++;
  }
  CSS.highlights.set('apex-look', all);
  CSS.highlights.set('apex-look-here', here);
})();"#;

/// The copy handle: every code block gets a button that sends the
/// block's text over (`copy:`), those made later too (a re-render).
const COPY_SCRIPT: &str = r#"(function () {
  function dress(pre) {
    if (pre.querySelector(':scope > .apex-copy')) return;
    const b = document.createElement('button');
    b.className = 'apex-copy'; b.type = 'button'; b.title = 'Copy to the snarf buffer'; b.textContent = 'copy';
    b.addEventListener('click', function (ev) {
      ev.preventDefault(); ev.stopPropagation();
      const code = pre.querySelector('code');
      const text = code ? code.textContent : Array.from(pre.childNodes).filter(n => n !== b).map(n => n.textContent).join('');
      try { window.ipc.postMessage('copy:' + text); } catch (e) {}
      b.textContent = 'copied'; setTimeout(function () { b.textContent = 'copy'; }, 1200);
    });
    pre.appendChild(b);
  }
  function all() { document.querySelectorAll('pre:not(.mermaid)').forEach(dress); }
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', all); else all();
  new MutationObserver(all).observe(document.documentElement, { childList: true, subtree: true });
})();"#;

/// A page from a buffer's contents, as a scrubber down its left edge (its
/// scrollbar is at the right):
/// each heading (h1 to h4) a tick where it stands in the page, longer the
/// higher it is, the part of the page in view a faint band over them, and
/// the heading of the part being read -- the last one above a quarter of
/// the way down the view -- in the accent (a scrollspy). A press on the
/// rail goes to that place in the page, and a drag scrubs through it; the
/// pointer on it brings out the contents, the headings as a list beside
/// it, the one being read in the accent, and a click on one goes there.
/// At most half the view's height, in the middle of it; the list may take
/// all of it. Only with two headings or more, and room for it: the page moved over as
/// much as it lacks, or no rail on a view too narrow to spare it. The
/// pointer on the rail marks the nearest tick's heading in the list. It
/// hangs off the document's root, not
/// its body, so a preview's morph (which redoes the body) leaves it be,
/// and it is laid out again whenever the page changes (a morph, an image
/// or a diagram coming in, the view resized).
const TOC_SCRIPT: &str = r#"(function () {
  if (window.__apexToc) return;
  window.__apexToc = true;
  function start() {
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(
      '#apex-toc{position:fixed;top:50%;height:min(50vh,calc(100vh - 28px));transform:translateY(-50%);left:10px;width:22px;z-index:2147483000;font:12.5px/1.35 var(--apex-font,-apple-system,sans-serif);user-select:none;-webkit-user-select:none}' +
      '#apex-toc[hidden]{display:none}' +
      '#apex-toc .rail{position:absolute;inset:0;cursor:pointer}' +
      '#apex-toc .tick{position:absolute;left:5px;height:2px;margin-top:-1px;border-radius:1px;background:var(--apex-dim);opacity:.5;transition:opacity .12s,background-color .12s}' +
      '#apex-toc .rail:hover .tick{opacity:.8}' +
      '#apex-toc .tick.on{background:var(--apex-accent);opacity:1}' +
      '#apex-toc .band{position:absolute;left:2px;width:18px;border-radius:4px;background:var(--apex-fg);opacity:.06;pointer-events:none}' +
      '#apex-toc .rail:hover .band{opacity:.1}' +
      '#apex-toc .panel{position:absolute;left:28px;top:50%;max-height:calc(100vh - 28px);overflow-y:auto;min-width:170px;max-width:300px;padding:6px;box-sizing:border-box;border-radius:10px;background:var(--apex-bg);border:1px solid var(--apex-border);box-shadow:0 8px 28px rgba(0,0,0,.16);opacity:0;transform:translate(-6px,-50%);pointer-events:none;transition:opacity .12s,transform .12s}' +
      '#apex-toc.open .panel{opacity:1;transform:translate(0,-50%);pointer-events:auto}' +
      '#apex-toc .item{padding:3px 8px;border-radius:6px;color:var(--apex-fg);white-space:nowrap;overflow:hidden;text-overflow:ellipsis;cursor:pointer;opacity:.72}' +
      '#apex-toc .item:hover,#apex-toc .item.near{background:var(--apex-tag-bg);opacity:1}' +
      '#apex-toc .item.on{color:var(--apex-accent);opacity:1;font-weight:600}' +
      '#apex-toc .item.path{direction:rtl;text-align:left}' +
      '#apex-toc .l1{font-weight:600;opacity:.9}#apex-toc .l2{padding-left:18px}#apex-toc .l3{padding-left:30px}#apex-toc .l4{padding-left:42px}');
    document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
    const root = document.createElement('div');
    root.id = 'apex-toc';
    root.hidden = true;
    const rail = document.createElement('div');
    rail.className = 'rail';
    const band = document.createElement('div');
    band.className = 'band';
    const panel = document.createElement('div');
    panel.className = 'panel';
    rail.appendChild(band);
    root.append(rail, panel);
    document.documentElement.appendChild(root);
    const view = () => document.scrollingElement || document.documentElement;
    const at = (h) => h.getBoundingClientRect().top + view().scrollTop;
    let heads = [], ticks = [], items = [], current = -1;
    // room for the rail left of the page's content: the page moved over
    // as much as it lacks (on the root, which a morph leaves be), and on
    // a view too narrow to spare it, no rail
    // the rail 10 in from the edge, 22 wide, as much air after it
    const RAIL = 44, NARROW = 420;
    let pad = 0;
    function room(show) {
      const el = document.querySelector('article') || document.body;
      let want = 0;
      if (show && el) {
        const lead = el.getBoundingClientRect().left + parseFloat(getComputedStyle(el).paddingLeft || '0') - pad;
        want = Math.max(0, Math.ceil(RAIL - lead));
      }
      if (want !== pad) {
        pad = want;
        document.documentElement.style.paddingLeft = pad ? pad + 'px' : '';
      }
    }
    function build() {
      heads = Array.from(document.body ? document.body.querySelectorAll('h1,h2,h3,h4') : []).filter((h) => h.textContent.trim());
      root.hidden = heads.length < 2 || innerWidth < NARROW;
      room(!root.hidden);
      ticks.forEach((t) => t.remove());
      panel.textContent = '';
      const H = rail.clientHeight, total = Math.max(1, view().scrollHeight);
      ticks = heads.map(function (h) {
        const t = document.createElement('div');
        t.className = 'tick';
        t.style.width = [0, 14, 10, 7, 5][+h.tagName[1]] + 'px';
        t.style.top = (at(h) / total) * H + 'px';
        rail.appendChild(t);
        return t;
      });
      items = heads.map(function (h) {
        const a = document.createElement('div');
        a.className = 'item l' + h.tagName[1];
        // a diff's file: its name alone, cut from the left when it is too
        // long (the end of a path says most), the whole of it on hover
        const name = h.querySelector('.name');
        const label = (name || h).textContent.trim();
        a.title = label;
        if (h.closest('section.file')) {
          a.classList.add('path');
          const b = document.createElement('bdi');
          b.dir = 'ltr';
          b.textContent = label;
          a.appendChild(b);
        } else {
          a.textContent = label;
        }
        a.addEventListener('mousedown', function (e) {
          e.preventDefault();
          e.stopPropagation();
          view().scrollTo({ top: Math.max(0, at(h) - 12), behavior: 'smooth' });
        });
        panel.appendChild(a);
        return a;
      });
      current = -1;
      spy();
    }
    function spy() {
      const s = view(), H = rail.clientHeight, total = Math.max(1, s.scrollHeight);
      band.style.top = (s.scrollTop / total) * H + 'px';
      band.style.height = Math.max(8, (s.clientHeight / total) * H) + 'px';
      let c = -1;
      const line = s.scrollTop + s.clientHeight * 0.25;
      for (let i = 0; i < heads.length; i++) {
        if (at(heads[i]) <= line) c = i;
        else break;
      }
      if (heads.length && s.scrollTop + s.clientHeight >= total - 2) c = heads.length - 1;
      if (c === current) return;
      if (current >= 0 && ticks[current]) { ticks[current].classList.remove('on'); items[current].classList.remove('on'); }
      current = c;
      if (c >= 0) {
        ticks[c].classList.add('on');
        items[c].classList.add('on');
        if (root.classList.contains('open')) items[c].scrollIntoView({ block: 'nearest' });
      }
    }
    // laid out again when the page changes, once a frame at most
    let queued = false;
    function later() {
      if (queued) return;
      queued = true;
      requestAnimationFrame(function () { queued = false; build(); });
    }
    let spying = false;
    addEventListener('scroll', function () {
      if (spying) return;
      spying = true;
      requestAnimationFrame(function () { spying = false; spy(); });
    }, { passive: true });
    addEventListener('resize', later);
    addEventListener('load', later, true);
    new MutationObserver(function (records) {
      if (records.some((r) => !root.contains(r.target))) later();
    }).observe(document.documentElement, { childList: true, subtree: true, characterData: true });
    // a press on the rail goes there; a drag scrubs
    let scrubbing = false;
    function scrub(e) {
      const r = rail.getBoundingClientRect(), s = view();
      const f = Math.min(1, Math.max(0, (e.clientY - r.top) / r.height));
      s.scrollTop = f * s.scrollHeight - s.clientHeight / 2;
    }
    rail.addEventListener('mousedown', function (e) {
      if (e.button !== 0) return;
      e.preventDefault();
      e.stopPropagation();
      scrubbing = true;
      scrub(e);
    });
    addEventListener('mousemove', function (e) { if (scrubbing) scrub(e); }, true);
    addEventListener('mouseup', function () { scrubbing = false; }, true);
    // the contents beside the rail while the pointer is on either
    let closing = null;
    root.addEventListener('mouseenter', function () {
      clearTimeout(closing);
      root.classList.add('open');
      if (current >= 0 && items[current]) items[current].scrollIntoView({ block: 'nearest' });
    });
    root.addEventListener('mouseleave', function () {
      closing = setTimeout(function () { if (!scrubbing) root.classList.remove('open'); }, 200);
      near(-1);
    });
    // the pointer on the rail: the heading of the tick nearest it marked
    // in the list, and brought into its view
    let marked = -1;
    function near(i) {
      if (i === marked) return;
      if (marked >= 0 && items[marked]) items[marked].classList.remove('near');
      marked = i;
      if (i >= 0 && items[i]) {
        items[i].classList.add('near');
        items[i].scrollIntoView({ block: 'nearest' });
      }
    }
    rail.addEventListener('mousemove', function (e) {
      const r = rail.getBoundingClientRect();
      const y = e.clientY - r.top;
      let best = -1, dist = Infinity;
      ticks.forEach(function (t, i) {
        const d = Math.abs(parseFloat(t.style.top) - y);
        if (d < dist) { dist = d; best = i; }
      });
      near(best);
    });
    build();
  }
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start);
  else start();
})();"#;

/// Mermaid in a page from a buffer (WEB.md §3): each `pre.mermaid` block
/// (what `apex md` makes of a ```mermaid fence) drawn as its diagram, in
/// the page's theme. Mermaid itself is large and most pages have no
/// diagram, so a page asks for it (`mermaid:`) only when it has a block,
/// and the client gives it its copy. A block keeps the source it was drawn
/// from, so re-rendering the page leaves a diagram whose source did not
/// change alone (`morph_script`), and a drawing that finishes after its
/// source changed is dropped; a theme change draws them all again.
const MERMAID_SCRIPT: &str = r#"(function () {
  if (window.__apexMermaid) return;
  let asked = false, n = 0;
  function dark() {
    const v = getComputedStyle(document.documentElement).getPropertyValue('--apex-bg').trim();
    const m = /^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})/i.exec(v);
    if (!m) return false;
    const [r, g, b] = [m[1], m[2], m[3]].map(function (x) { return parseInt(x, 16) / 255; });
    return 0.2126 * r + 0.7152 * g + 0.0722 * b < 0.5;
  }
  window.__apexMermaid = function () {
    const blocks = Array.from(document.querySelectorAll('pre.mermaid'));
    if (!blocks.length) return;
    if (!window.mermaid) {
      if (!asked) { asked = true; try { window.ipc.postMessage('mermaid:'); } catch (e) {} }
      return;
    }
    const d = dark();
    if (window.__apexMermaidDark !== d) {
      window.mermaid.initialize({ startOnLoad: false, theme: d ? 'dark' : 'neutral', securityLevel: 'strict', suppressErrorRendering: true });
      window.__apexMermaidDark = d;
      blocks.forEach(function (b) { delete b.dataset.apexDrawn; });
    }
    blocks.forEach(function (b) {
      if (b.dataset.apexDrawn) return;
      if (b.dataset.apexSrc === undefined) b.dataset.apexSrc = b.textContent;
      const src = b.dataset.apexSrc;
      window.mermaid.render('apex-mermaid-' + (++n), src).then(function (r) {
        if (b.isConnected && b.dataset.apexSrc === src) { b.innerHTML = r.svg; b.dataset.apexDrawn = '1'; b.removeAttribute('title'); }
      }).catch(function (e) {
        if (b.isConnected && b.dataset.apexSrc === src) { b.textContent = src; b.dataset.apexDrawn = '1'; b.title = String((e && e.message) || e); }
      });
    });
  };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', function () { window.__apexMermaid(); });
  else window.__apexMermaid();
})();"#;

/// Mermaid, the browser build, vendored gzipped (`assets/`): unpacked the
/// first time a page asks for it.
fn mermaid_js() -> &'static str {
    static JS: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    JS.get_or_init(|| {
        use std::io::Read;
        let gz: &[u8] = include_bytes!("../assets/mermaid.min.js.gz");
        let mut js = String::new();
        if flate2::read::GzDecoder::new(gz).read_to_string(&mut js).is_err() {
            js.clear();
        }
        js
    })
}

/// One window's view.
pub struct WebHost {
    view: wry::WebView,
    /// The URL we loaded or were told the page went to: when the window's
    /// name differs, the state moved the page and we load it.
    url: String,
    bounds: Option<Bounds<Pixels>>,
    shown: bool,
    /// The holes cut in the view (`set_holes`), in the view's own
    /// coordinates, as last applied.
    holes: Vec<(f64, f64, f64, f64)>,
    /// A page from a buffer: the buffer version shown, and the directory
    /// its relative links resolve in.
    html: Option<(u64, String)>,
    /// The source line the page was last scrolled to follow.
    followed: Option<usize>,
    /// Loading since: the handle pulses. Set the moment a load is asked
    /// for (WebKit says "started" only once content arrives), cleared
    /// when the page finishes, or after a while when it never says so.
    loading: Option<std::time::Instant>,
    /// Watch streams on the host files this page fetched, by path.
    watches: Arc<Mutex<HashMap<String, u32>>>,
    plane: Option<IoPlane>,
    /// Where the page last said it was scrolled: top, length, and how much
    /// shows, for the scrollbar beside it.
    scroll: Option<(f64, f64, f64)>,
    /// The veil laid over the page while a dialog has the window (`set_veil`):
    /// its colour, and the layer that draws it.
    veil: Option<(u32, VeilLayer)>,
}

/// A CALayer over a page, retained while it is there.
pub struct VeilLayer(*mut objc::runtime::Object);

impl Drop for VeilLayer {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        // SAFETY: a layer we made and retained; removed from its parent
        // before the last reference goes.
        unsafe {
            use objc::{msg_send, sel, sel_impl};
            let _: () = msg_send![self.0, removeFromSuperlayer];
            let _: () = msg_send![self.0, release];
        }
    }
}

impl Drop for WebHost {
    fn drop(&mut self) {
        // the watches end with the page
        if let Some(plane) = &self.plane {
            for (_, stream) in self.watches.lock().unwrap().drain() {
                plane.end(stream);
                plane.close(stream);
            }
        }
    }
}

/// Run in every page: the CSS cursor of the element under the pointer,
/// reported when it changes. `auto` is read as WebKit would: a hand
/// within a link, a beam in a text field, the arrow elsewhere.
/// The thumb for a page scrolled `top` down a page `height` long with
/// `view` of it showing: the part of the bar, 0 to 1, the view covers.
/// All of the bar before the page has said, or when it all shows.
fn thumb_of(scroll: Option<(f64, f64, f64)>) -> (f32, f32) {
    match scroll {
        Some((top, height, view)) if height > view && height > 0. => {
            let t0 = (top / height).clamp(0., 1.) as f32;
            let t1 = ((top + view) / height).clamp(0., 1.) as f32;
            (t0, t1)
        }
        _ => (0., 1.),
    }
}

/// acme's scrollbar for a page (WEB.md §2.2): the page's own is hidden,
/// in a constructed stylesheet a page's morph cannot take out, and where
/// the page is scrolled is said whenever that or its length changes, once
/// a frame, for the scrollbar drawn beside the view.
const SCROLL_SCRIPT: &str = r#"(function () {
  if (window.__apexScroll) return;
  window.__apexScroll = true;
  try {
    const sheet = new CSSStyleSheet();
    sheet.replaceSync('html, body { scrollbar-width: none !important; } ::-webkit-scrollbar { display: none !important; width: 0 !important; height: 0 !important; }');
    document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
  } catch (e) {}
  let asked = false, last = '';
  function say() {
    asked = false;
    const e = document.scrollingElement || document.documentElement;
    const now = Math.round(e.scrollTop) + ',' + Math.round(e.scrollHeight) + ',' + Math.round(e.clientHeight);
    if (now !== last) { last = now; try { window.ipc.postMessage('scroll:' + now); } catch (x) {} }
  }
  function soon() { if (!asked) { asked = true; requestAnimationFrame(say); } }
  addEventListener('scroll', soon, { passive: true, capture: true });
  addEventListener('resize', soon);
  addEventListener('load', soon);
  function watch() {
    try { new ResizeObserver(soon).observe(document.documentElement); } catch (x) {}
    soon();
  }
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', watch); else watch();
})();"#;

/// A page's selection kept while the page has not the keyboard. WebKit
/// clears a page's selection when its view stops being the window's
/// first responder, and in apex the keyboard follows the pointer, so
/// the pointer going off a page lost what was selected there. The last
/// selection the page had is kept (let go of only when a click or a key
/// in the page empties it), painted as the selection is while the page
/// is away (`__apexAway`), and put back when the keyboard comes back to
/// it (`__apexBack`): `Webs::focus_tick` says which.
const KEEP_SCRIPT: &str = r#"(function () {
  let kept = null, input = 0;
  const marks = function () { return window.CSS && CSS.highlights && window.Highlight; };
  const touched = function () { input = Date.now(); };
  addEventListener('mousedown', touched, true);
  addEventListener('keydown', touched, true);
  document.addEventListener('selectionchange', function () {
    const s = getSelection();
    if (s.rangeCount && !s.isCollapsed) kept = s.getRangeAt(0).cloneRange();
    else if (Date.now() - input < 1000) kept = null;
  });
  let sheet = null;
  window.__apexAway = function () {
    if (!kept || !marks()) return;
    if (!sheet) {
      sheet = new CSSStyleSheet();
      sheet.replaceSync('::highlight(apex-kept){background-color:var(--apex-sel, Highlight)}');
      document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
    }
    CSS.highlights.set('apex-kept', new Highlight(kept));
  };
  window.__apexBack = function () {
    if (marks()) CSS.highlights.delete('apex-kept');
    const s = getSelection();
    if (kept && kept.startContainer.isConnected && (!s.rangeCount || s.isCollapsed)) {
      try { s.removeAllRanges(); s.addRange(kept); } catch (e) {}
    }
  };
})();"#;

const CURSOR_SCRIPT: &str = r#"(function () {
  let last = '';
  function say(c) { if (c !== last) { last = c; try { window.ipc.postMessage('cursor:' + c); } catch (e) {} } }
  function at(e) {
    const el = document.elementFromPoint(e.clientX, e.clientY);
    if (!el) { say('default'); return; }
    let c = getComputedStyle(el).cursor || 'auto';
    if (c === 'auto') {
      if (el.closest && el.closest('a[href], button, summary, [role=button], [role=link]')) c = 'pointer';
      else if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable) c = 'text';
      else c = 'default';
    }
    say(c);
  }
  document.addEventListener('mousemove', at, { capture: true, passive: true });
  document.addEventListener('mouseleave', function () { say('default'); }, { capture: true, passive: true });
  // a button down in the page, which the app does not hear otherwise
  document.addEventListener('mousedown', function () { try { window.ipc.postMessage('down:'); } catch (e) {} }, { capture: true, passive: true });
})();"#;

pub struct Webs {
    hosts: HashMap<WindowId, WebHost>,
    tx: Sender<(WindowId, WebEvent)>,
    rx: Receiver<(WindowId, WebEvent)>,
    /// The session's I/O plane, and the proxy's port on it.
    plane: Option<IoPlane>,
    proxy: Option<u16>,
    /// Wakes the UI when a page reports something.
    wake: Option<Wake>,
    /// The page the keyboard was given to, the pointer being over it.
    focused: Option<WindowId>,
    /// The cursor each page last asked for.
    cursors: HashMap<WindowId, gpui::CursorStyle>,
    /// The overlays over the pages this frame (the window's coordinates):
    /// the pointer there is not on a page.
    cut: Vec<Bounds<Pixels>>,
}

/// A page that has the keyboard when its view goes hands it back first.
/// AppKit leaves the window itself as the first responder when a view
/// that is one is taken out of it, and a window responder answers no
/// key: every keystroke beeps then, and the keys that still arrive come
/// the roundabout way, twice. The views go together when a session is
/// parked or adopted (`Webs` replaced whole), where nothing else is
/// watching, so the handing back belongs here.
impl Drop for Webs {
    fn drop(&mut self) {
        for h in self.hosts.values() {
            let _ = h.view.focus_parent();
        }
    }
}

/// What Back, Fwd and Get do in a web window's tag.
#[derive(Clone, Copy)]
pub enum Nav {
    Back,
    Fwd,
    Reload,
}

impl Webs {
    /// Views over `plane` (the host's network and files) when there is
    /// one; on their own otherwise.
    pub fn new(plane: Option<IoPlane>, wake: Option<Wake>) -> Webs {
        let (tx, rx) = channel();
        let proxy = plane.as_ref().and_then(|p| match start_connect_proxy(p.clone()) {
            Ok(port) => Some(port),
            Err(e) => {
                eprintln!("web: no proxy: {e}");
                None
            }
        });
        if std::env::var_os("APEX_WEB_DEBUG").is_some() {
            eprintln!("web: views over a plane: {}, proxy port {proxy:?}", plane.is_some());
        }
        Webs { hosts: HashMap::new(), tx, rx, plane, proxy, wake, focused: None, cursors: HashMap::new(), cut: Vec::new() }
    }

    /// The shown page under `pos`, if any.
    pub fn window_at(&self, pos: Point<Pixels>) -> Option<WindowId> {
        // an overlay over a page (a toast, a menu) is not the page
        if self.cut.iter().any(|b| b.contains(&pos)) {
            return None;
        }
        self.hosts.iter().find(|(_, h)| h.shown && h.bounds.is_some_and(|b| b.contains(&pos))).map(|(w, _)| *w)
    }

    /// An Edit menu command (`copy`, `cut`, `paste`, `select-all`) for
    /// the page in window `w`: WebKit does it on the page's selection,
    /// as the menu would were it its own. False when the page cannot.
    #[cfg(target_os = "macos")]
    pub fn edit(&self, w: WindowId, what: &str) -> bool {
        use objc::runtime::{Object, Sel};
        use objc::{msg_send, sel, sel_impl};
        use wry::WebViewExtMacOS;
        let Some(h) = self.hosts.get(&w) else { return false };
        let sel: Sel = match what {
            "copy" => sel!(copy:),
            "cut" => sel!(cut:),
            "paste" => sel!(paste:),
            "select-all" => sel!(selectAll:),
            _ => return false,
        };
        let wk = h.view.webview();
        let view = &*wk as *const _ as *mut Object;
        // SAFETY: the WKWebView is alive while its host is; the editing
        // selectors are WebKit's own on the main thread.
        unsafe {
            let can: bool = msg_send![view, respondsToSelector: sel];
            if !can {
                return false;
            }
            let nil: *mut Object = std::ptr::null_mut();
            let _: () = msg_send![view, performSelector: sel withObject: nil];
        }
        true
    }

    #[cfg(not(target_os = "macos"))]
    pub fn edit(&self, _w: WindowId, _what: &str) -> bool {
        false
    }

    /// Holes cut in the views where the overlays are: a mask on each
    /// view's layer, the view's rectangle less the overlays' (even-odd),
    /// so gpui's overlay shows through and the page stays live around
    /// it. No overlay over a view, no mask.
    #[cfg(target_os = "macos")]
    pub fn set_holes(&mut self, holes: &[(Bounds<Pixels>, Pixels)]) {
        use objc::runtime::Object;
        use objc::{class, msg_send, sel, sel_impl};
        use wry::WebViewExtMacOS;
        #[repr(C)]
        struct CGPoint { x: f64, y: f64 }
        #[repr(C)]
        struct CGSize { w: f64, h: f64 }
        #[repr(C)]
        struct CGRect { origin: CGPoint, size: CGSize }
        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" {
            fn CGPathCreateMutable() -> *mut std::ffi::c_void;
            fn CGPathAddRect(path: *mut std::ffi::c_void, m: *const std::ffi::c_void, rect: CGRect);
            fn CGPathRelease(path: *mut std::ffi::c_void);
        }
        #[link(name = "QuartzCore", kind = "framework")]
        extern "C" {}
        self.cut = holes.iter().map(|&(o, _)| o).collect();
        // the panels' shadows reach past their bounds, each its own way
        for h in self.hosts.values_mut() {
            let Some(vb) = h.bounds else { continue };
            if !h.shown {
                continue;
            }
            // what of the overlays lies over this page, their bounds alone:
            // the clicks there are theirs, not the page's (`hit_test`)
            {
                let wk = h.view.webview();
                let view = &*wk as *const _ as *mut Object;
                let hits: Vec<[f64; 4]> = holes
                    .iter()
                    .filter_map(|&(o, _)| {
                        let x0 = o.origin.x.max(vb.origin.x);
                        let y0 = o.origin.y.max(vb.origin.y);
                        let x1 = (o.origin.x + o.size.width).min(vb.origin.x + vb.size.width);
                        let y1 = (o.origin.y + o.size.height).min(vb.origin.y + vb.size.height);
                        (x1 > x0 && y1 > y0).then(|| [f64::from(f32::from(x0 - vb.origin.x)), f64::from(f32::from(y0 - vb.origin.y)), f64::from(f32::from(x1 - x0)), f64::from(f32::from(y1 - y0))])
                    })
                    .collect();
                hit::set(view, hits);
            }
            let local: Vec<(f64, f64, f64, f64)> = holes
                .iter()
                .filter_map(|&(o, margin)| {
                    let x0 = (o.origin.x - margin).max(vb.origin.x);
                    let y0 = (o.origin.y - margin).max(vb.origin.y);
                    let x1 = (o.origin.x + o.size.width + margin).min(vb.origin.x + vb.size.width);
                    let y1 = (o.origin.y + o.size.height + margin).min(vb.origin.y + vb.size.height);
                    (x1 > x0 && y1 > y0).then(|| (f64::from(f32::from(x0 - vb.origin.x)), f64::from(f32::from(y0 - vb.origin.y)), f64::from(f32::from(x1 - x0)), f64::from(f32::from(y1 - y0))))
                })
                .collect();
            if local == h.holes {
                continue;
            }
            h.holes = local.clone();
            let wk = h.view.webview();
            let view = &*wk as *const _ as *mut Object;
            // SAFETY: the WKWebView is alive while its host is; CoreAnimation
            // and CoreGraphics calls on the main thread.
            unsafe {
                let layer: *mut Object = msg_send![view, layer];
                if layer.is_null() {
                    continue;
                }
                if local.is_empty() {
                    let nil: *mut Object = std::ptr::null_mut();
                    let _: () = msg_send![layer, setMask: nil];
                    continue;
                }
                let bounds: CGRect = msg_send![layer, bounds];
                let flipped: bool = msg_send![layer, isGeometryFlipped];
                let path = CGPathCreateMutable();
                CGPathAddRect(path, std::ptr::null(), CGRect { origin: CGPoint { x: 0., y: 0. }, size: CGSize { w: bounds.size.w, h: bounds.size.h } });
                for (x, y, w, hh) in &local {
                    // the layer's origin is top-left when flipped, else bottom-left
                    let y = if flipped { *y } else { bounds.size.h - y - hh };
                    CGPathAddRect(path, std::ptr::null(), CGRect { origin: CGPoint { x: *x, y }, size: CGSize { w: *w, h: *hh } });
                }
                let shape: *mut Object = msg_send![class!(CAShapeLayer), layer];
                let _: () = msg_send![shape, setFrame: bounds];
                let _: () = msg_send![shape, setPath: path];
                let rule: *mut Object = msg_send![class!(NSString), stringWithUTF8String: b"even-odd\0".as_ptr()];
                let _: () = msg_send![shape, setFillRule: rule];
                let _: () = msg_send![layer, setMask: shape];
                CGPathRelease(path);
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn set_holes(&mut self, holes: &[(Bounds<Pixels>, Pixels)]) {
        self.cut = holes.iter().map(|&(o, _)| o).collect();
    }

    /// A dialog has the window, and the window goes quiet behind it: gpui
    /// paints its veil over everything it draws, but a page is a native
    /// view above all of that, so each page gets the veil too -- a layer of
    /// the same colour over it (`rgba`), inside the page's own layer and so
    /// cut by the same holes the dialog cuts. A layer takes no events: the
    /// page answers the pointer as it did. `None` takes the veils away.
    #[cfg(target_os = "macos")]
    pub fn set_veil(&mut self, rgba: Option<u32>) {
        use objc::runtime::Object;
        use objc::{class, msg_send, sel, sel_impl};
        use wry::WebViewExtMacOS;
        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" {
            fn CGColorCreateSRGB(r: f64, g: f64, b: f64, a: f64) -> *mut std::ffi::c_void;
            fn CGColorRelease(c: *mut std::ffi::c_void);
        }
        /// kCALayerWidthSizable | kCALayerHeightSizable: the veil follows
        /// the page as it is laid out again
        const SIZABLE: u32 = 2 | 16;
        for h in self.hosts.values_mut() {
            let want = rgba.filter(|_| h.shown);
            if h.veil.as_ref().map(|(c, _)| *c) == want {
                continue;
            }
            h.veil = None; // the old one, if any, comes off
            let Some(c) = want else { continue };
            let wk = h.view.webview();
            let view = &*wk as *const _ as *mut Object;
            // SAFETY: the WKWebView is alive while its host is; CoreAnimation
            // and CoreGraphics calls on the main thread; the layer is
            // retained by `VeilLayer` and released when it is dropped.
            unsafe {
                let host: *mut Object = msg_send![view, layer];
                if host.is_null() {
                    continue;
                }
                // a CGRect, as CoreAnimation passes it
                #[repr(C)]
                #[derive(Clone, Copy)]
                struct Rect {
                    x: f64,
                    y: f64,
                    w: f64,
                    h: f64,
                }
                let bounds: Rect = msg_send![host, bounds];
                let veil: *mut Object = msg_send![class!(CALayer), layer];
                let veil: *mut Object = msg_send![veil, retain];
                let _: () = msg_send![veil, setFrame: bounds];
                let _: () = msg_send![veil, setAutoresizingMask: SIZABLE];
                // over whatever the page's own layers are doing
                let _: () = msg_send![veil, setZPosition: 1.0e6_f64];
                let comp = |shift: u32| f64::from((c >> shift) & 0xff) / 255.;
                let color = CGColorCreateSRGB(comp(24), comp(16), comp(8), comp(0));
                let _: () = msg_send![veil, setBackgroundColor: color];
                CGColorRelease(color);
                let _: () = msg_send![host, addSublayer: veil];
                h.veil = Some((c, VeilLayer(veil)));
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn set_veil(&mut self, _rgba: Option<u32>) {}

    /// The theme changed: every page from a buffer takes the new colours
    /// in place (its `#apex-theme` style rewritten; nothing reloads).
    pub fn restyle(&self) {
        let css = js_string(&theme_css());
        for h in self.hosts.values() {
            if h.html.is_some() {
                let _ = h.view.evaluate_script(&format!("(function(){{var s=document.getElementById('apex-theme');if(s){{s.textContent={css};}}window.__apexMermaid&&window.__apexMermaid();}})();"));
            }
        }
    }

    /// The page's history and reload: Back, Fwd, Get in its tag.
    pub fn go(&mut self, w: WindowId, nav: Nav) {
        let Some(h) = self.hosts.get_mut(&w) else { return };
        h.loading = Some(std::time::Instant::now());
        let _ = match nav {
            Nav::Back => h.view.go_back(),
            Nav::Fwd => h.view.go_forward(),
            Nav::Reload => h.view.reload(),
        };
    }

    /// Keys go where the pointer is, as everywhere in acme: over a page
    /// the page has the keyboard (the window's first responder), over
    /// anything else gpui's view does. Called on a timer while pages
    /// exist, since a native view keeps the pointer's moves to itself.
    pub fn focus_tick(&mut self, window: &Window) {
        let Some(pos) = native_mouse(window) else { return };
        let over = self.window_at(pos);
        if over != self.focused {
            // the page left keeps its selection painted; the one come to
            // has it back (`KEEP_SCRIPT`)
            if let Some(h) = self.focused.and_then(|w| self.hosts.get(&w)) {
                let _ = h.view.evaluate_script("window.__apexAway&&__apexAway()");
            }
        }
        match over {
            Some(w) if over != self.focused => {
                if let Some(h) = self.hosts.get(&w) {
                    let _ = h.view.focus();
                    let _ = h.view.evaluate_script("window.__apexBack&&__apexBack()");
                }
            }
            Some(_) => {}
            // every tick, not only when the pointer leaves a page: the
            // keyboard can go from gpui's view behind our back (a page
            // taking it, a view of AppKit's), and what `focused` says was
            // asked for is then not what the window has. `focus_ui` looks
            // before it acts, so a tick that finds it right costs nothing.
            None => focus_ui(window),
        }
        self.focused = over;
    }

    /// The keyboard back to gpui's view, whatever page the pointer is on:
    /// while an overlay is up (a walk ⌘ or control holds open ends when
    /// that key comes up, and a page with the keys would hear it instead).
    pub fn unfocus(&mut self, window: &Window) {
        focus_ui(window);
        if let Some(h) = self.focused.and_then(|w| self.hosts.get(&w)) {
            let _ = h.view.evaluate_script("window.__apexAway&&__apexAway()");
        }
        self.focused = None;
    }

    fn rect(bounds: Bounds<Pixels>) -> wry::Rect {
        wry::Rect {
            position: wry::dpi::LogicalPosition::new(f32::from(bounds.origin.x), f32::from(bounds.origin.y)).into(),
            size: wry::dpi::LogicalSize::new(f32::from(bounds.size.width), f32::from(bounds.size.height)).into(),
        }
    }

    /// Put window `w`'s view at `bounds`, building it on `url` the first
    /// time; shown or not.
    pub fn place(&mut self, w: WindowId, url: &str, bounds: Bounds<Pixels>, window: &Window, visible: bool) {
        if !self.hosts.contains_key(&w) {
            self.build(w, Page::Url(url), bounds, window, visible);
            return;
        }
        let rect = Self::rect(bounds);
        let h = self.hosts.get_mut(&w).unwrap();
        if h.url != url {
            // the state moved the page (a Goto, another client): follow
            h.url = url.to_string();
            h.loading = Some(std::time::Instant::now());
            let _ = h.view.load_url(&webkit_url(url));
        }
        h.settle_view(rect, bounds, visible);
    }

    /// Show a buffer's HTML (`version`) as window `w`'s page, relative
    /// links resolving in `dir` on the host: the page is patched in place
    /// when the version moves, so scroll and state in it survive.
    pub fn place_html(&mut self, w: WindowId, html: &str, version: u64, dir: &str, bounds: Bounds<Pixels>, window: &Window, visible: bool) {
        if !self.hosts.contains_key(&w) {
            self.build(w, Page::Html { html, version, dir }, bounds, window, visible);
            return;
        }
        let rect = Self::rect(bounds);
        let h = self.hosts.get_mut(&w).unwrap();
        if h.html.as_ref().is_some_and(|(v, _)| *v != version) {
            h.html = Some((version, dir.to_string()));
            let _ = h.view.evaluate_script(&morph_script(&dress(html, dir)));
        }
        h.settle_view(rect, bounds, visible);
    }

    fn build(&mut self, w: WindowId, page: Page, bounds: Bounds<Pixels>, window: &Window, visible: bool) {
        let rect = Self::rect(bounds);
        let watches: Arc<Mutex<HashMap<String, u32>>> = Arc::new(Mutex::new(HashMap::new()));
        let (tx1, tx2, tx3, tx4) = (self.tx.clone(), self.tx.clone(), self.tx.clone(), self.tx.clone());
        let (wake1, wake2, wake3, wake4) = (self.wake.clone(), self.wake.clone(), self.wake.clone(), self.wake.clone());
        let from_buffer = matches!(page, Page::Html { .. });
        let mut b = wry::WebViewBuilder::new()
            .with_bounds(rect)
            .with_on_page_load_handler(move |ev, url| {
                let _ = tx3.send((w, WebEvent::Loading(matches!(ev, wry::PageLoadEvent::Started))));
                // where the page is: the view's own URL, the main frame's
                // (the navigation handler sees every frame's, an iframe's
                // ad or captcha included, and cannot tell them apart)
                if !from_buffer && !url.is_empty() && !url.starts_with("about:") {
                    let _ = tx3.send((w, WebEvent::Navigated(apex_url(&url))));
                }
                if let Some(k) = &wake3 {
                    k();
                }
            })
            // the cursor the page wants under the pointer, as it changes
            .with_initialization_script(CURSOR_SCRIPT)
            .with_initialization_script(KEEP_SCRIPT)
            .with_initialization_script(SCROLL_SCRIPT)
            .with_ipc_handler(move |req| {
                if req.body() == "down:" {
                    let _ = tx4.send((w, WebEvent::Down));
                    if let Some(k) = &wake4 {
                        k();
                    }
                } else if req.body() == "mermaid:" {
                    let _ = tx4.send((w, WebEvent::Mermaid));
                    if let Some(k) = &wake4 {
                        k();
                    }
                } else if let Some(rest) = req.body().strip_prefix("scroll:") {
                    let n: Vec<f64> = rest.split(',').filter_map(|x| x.parse().ok()).collect();
                    if let [top, height, view] = n[..] {
                        let _ = tx4.send((w, WebEvent::Scroll { top, height, view }));
                        if let Some(k) = &wake4 {
                            k();
                        }
                    }
                } else if let Some(c) = req.body().strip_prefix("cursor:") {
                    let _ = tx4.send((w, WebEvent::Cursor(c.to_string())));
                    if let Some(k) = &wake4 {
                        k();
                    }
                } else if let Some(text) = req.body().strip_prefix("copy:") {
                    let _ = tx4.send((w, WebEvent::Copy(text.to_string())));
                    if let Some(k) = &wake4 {
                        k();
                    }
                }
            });
        b = match page {
            Page::Url(url) => {
                if std::env::var_os("APEX_WEB_DEBUG").is_some() {
                    eprintln!("web: {w} loads {}", webkit_url(url));
                }
                b.with_url(&webkit_url(url))
            }
            Page::Html { html, dir, .. } => b.with_initialization_script(COPY_SCRIPT).with_initialization_script(MERMAID_SCRIPT).with_initialization_script(TOC_SCRIPT).with_html(dress(html, dir)),
        };
        b = b
            .with_navigation_handler(move |u| {
                if let Some((path, line)) = file_link(&u) {
                    // a file link with a line: the file in a text window there
                    let _ = tx1.send((w, WebEvent::Open(path, Some(line))));
                    if let Some(k) = &wake1 {
                        k();
                    }
                    return false;
                }
                if from_buffer {
                    // our page does not go anywhere: a link is a window
                    if u == "about:blank" || u.is_empty() {
                        return true;
                    }
                    let _ = tx1.send((w, WebEvent::Link(apex_url(&u))));
                    if let Some(k) = &wake1 {
                        k();
                    }
                    return false;
                }
                // about:blank and its kin are WebKit's own steps (a popup's
                // first page, a redirect's hop), no place of the page's:
                // let them pass without renaming the window after them
                if u.starts_with("about:") || u.is_empty() {
                    return true;
                }
                if let Some(rest) = u.strip_prefix("file://") {
                    // a file link: the host's file, through apexfile://
                    let path = rest.strip_prefix("localhost").unwrap_or(rest);
                    let _ = tx1.send((w, WebEvent::Reroute(format!("apexfile://{path}"))));
                    if let Some(k) = &wake1 {
                        k();
                    }
                    return false;
                }
                if alias_loopback_url(&u) != u {
                    // a bare loopback link: the host's, through the proxy
                    let _ = tx1.send((w, WebEvent::Reroute(u)));
                    if let Some(k) = &wake1 {
                        k();
                    }
                    return false;
                }
                // any other navigation, the page's or a frame's, goes ahead;
                // the window's name follows the page from the load handler
                true
            })
            .with_document_title_changed_handler(move |t| {
                let _ = tx2.send((w, WebEvent::Title(t)));
                if let Some(k) = &wake2 {
                    k();
                }
            });
        if let Some(port) = self.proxy {
            if std::env::var_os("APEX_WEB_DEBUG").is_some() {
                eprintln!("web: {w} through the proxy on {port}");
            }
            b = b.with_proxy_config(wry::ProxyConfig::Http(wry::ProxyEndpoint { host: "127.0.0.1".into(), port: port.to_string() }));
        }
        let fetcher = Fetcher { plane: self.plane.clone(), watches: watches.clone(), events: self.tx.clone(), wake: self.wake.clone(), window: w };
        b = b.with_asynchronous_custom_protocol("apexfile".into(), move |_, request, responder| {
            let f = fetcher.clone();
            std::thread::spawn(move || f.serve(request, responder));
        });
        let (url, html) = match page {
            Page::Url(url) => (url.to_string(), None),
            Page::Html { version, dir, .. } => (String::new(), Some((version, dir.to_string()))),
        };
        match b.build_as_child(window) {
            Ok(view) => {
                let _ = view.set_visible(visible);
                round_foot(&view);
                let loading = if from_buffer { None } else { Some(std::time::Instant::now()) };
                self.hosts.insert(w, WebHost { view, url, bounds: Some(bounds), shown: visible, holes: Vec::new(), html, followed: None, loading, watches, plane: self.plane.clone(), scroll: None, veil: None });
            }
            Err(e) => eprintln!("web: {w}: {e}"),
        }
    }

    /// Hide every view not in `shown` (windows the layout does not draw:
    /// obscured by a full-column window, no body room), and drop the
    /// views of windows that are gone. A view that had the keyboard
    /// hands it back to the window's own view first: keys must not be
    /// left with a hidden or vanished responder.
    pub fn settle(&mut self, shown: &HashSet<WindowId>, alive: impl Fn(WindowId) -> bool) {
        for (w, h) in self.hosts.iter() {
            if !alive(*w) {
                let _ = h.view.focus_parent();
            }
        }
        self.hosts.retain(|w, _| alive(*w));
        for (w, h) in self.hosts.iter_mut() {
            if !shown.contains(w) && h.shown {
                let _ = h.view.focus_parent();
                let _ = h.view.set_visible(false);
                h.shown = false;
            }
        }
        // the page the keyboard was given to is hidden or gone: the tick
        // must not read `focused` as though it still had it
        if self.focused.is_some_and(|w| !self.hosts.get(&w).is_some_and(|h| h.shown)) {
            self.focused = None;
        }
    }

    /// Is window `w`'s page loading? Not after half a minute without a
    /// word from WebKit: a load that failed says nothing.
    pub fn loading(&self, w: WindowId) -> bool {
        self.hosts.get(&w).is_some_and(|h| h.loading.is_some_and(|t| t.elapsed() < Duration::from_secs(30)))
    }

    pub fn any_loading(&self) -> bool {
        self.hosts.keys().any(|w| self.loading(*w))
    }

    /// WebKit's word: content started arriving (already pulsing, mostly),
    /// or the page is done.
    /// A page said what cursor it wants: the system one for the CSS name.
    pub fn set_cursor(&mut self, w: WindowId, css: &str) {
        use gpui::CursorStyle::*;
        let style = match css {
            "pointer" => PointingHand,
            "text" | "vertical-text" => IBeam,
            "grab" => OpenHand,
            "grabbing" => ClosedHand,
            "crosshair" => Crosshair,
            "col-resize" | "ew-resize" | "e-resize" | "w-resize" => ResizeLeftRight,
            "row-resize" | "ns-resize" | "n-resize" | "s-resize" => ResizeUpDown,
            "not-allowed" | "no-drop" => OperationNotAllowed,
            _ => Arrow,
        };
        self.cursors.insert(w, style);
    }

    /// The cursor a page last asked for (the arrow until it says).
    pub fn cursor(&self, w: WindowId) -> gpui::CursorStyle {
        self.cursors.get(&w).copied().unwrap_or(gpui::CursorStyle::Arrow)
    }

    /// Look in a page: the next place `text` is in it, after the page's
    /// selection (backwards when `reverse`), wrapping, selected and scrolled
    /// to; with no text, the page's own selection looked for again.
    ///
    /// What was found is marked in a deeper shade of the selection's
    /// colour, and every other place the text is in the page in the
    /// selection's colour, until a click or a key in the page.
    /// A word the page says it answers (`page_verbs`), run in the page:
    /// its `apexVerb`.
    pub fn verb(&self, w: WindowId, word: &str) {
        if let Some(h) = self.hosts.get(&w) {
            let _ = h.view.evaluate_script(&format!("window.apexVerb && window.apexVerb({})", js_string(word)));
        }
    }

    pub fn find(&self, w: WindowId, text: &str, reverse: bool) {
        if let Some(h) = self.hosts.get(&w) {
            // the other places the selection's tint, the place found that
            // tint deepened a quarter of the way to the ink: told apart by
            // how dark, not by hue
            let t = crate::theme::theme();
            let hex = |c: u32| format!("#{c:06X}");
            let deeper = crate::text_element::mix(t.body_sel, t.text, 0.25);
            // the text last, so that nothing in it is taken for a slot
            let js = LOOK_SCRIPT
                .replace("__REVERSE__", if reverse { "true" } else { "false" })
                .replace("__HERE__", &hex(deeper))
                .replace("__ELSE__", &hex(t.body_sel))
                .replace("__TEXT__", &js_string(text));
            let _ = h.view.evaluate_script(&js);
        }
    }

    /// A page asked for mermaid: given, and its diagrams drawn.
    pub fn give_mermaid(&self, w: WindowId) {
        let js = mermaid_js();
        if js.is_empty() {
            return;
        }
        if let Some(h) = self.hosts.get(&w) {
            let _ = h.view.evaluate_script(&format!("{js}\n;window.__apexMermaid && window.__apexMermaid();"));
        }
    }

    /// The page said where it is scrolled.
    pub fn set_scroll(&mut self, w: WindowId, top: f64, height: f64, view: f64) {
        if let Some(h) = self.hosts.get_mut(&w) {
            h.scroll = Some((top, height, view));
        }
    }

    /// Where the page is scrolled, as a thumb: the part of the scrollbar
    /// (0 to 1, top and bottom) the view covers. All of it before the
    /// page has said, or when it is no longer than the view.
    pub fn thumb(&self, w: WindowId) -> (f32, f32) {
        thumb_of(self.hosts.get(&w).and_then(|h| h.scroll))
    }

    /// The page scrolled by `dy` CSS pixels (down when positive).
    pub fn scroll_by(&self, w: WindowId, dy: f64) {
        if let Some(h) = self.hosts.get(&w) {
            let _ = h.view.evaluate_script(&format!("(document.scrollingElement || document.documentElement).scrollBy(0, {dy});"));
        }
    }

    /// The page scrolled so that `frac` (0 to 1) of the way down it is at
    /// the top: acme's B2 in a scrollbar.
    pub fn scroll_to_fraction(&self, w: WindowId, frac: f64) {
        if let Some(h) = self.hosts.get(&w) {
            let _ = h.view.evaluate_script(&format!("(function(){{const e = document.scrollingElement || document.documentElement; e.scrollTo(0, {frac} * e.scrollHeight);}})();"));
        }
    }

    pub fn set_loading(&mut self, w: WindowId, on: bool) {
        if let Some(h) = self.hosts.get_mut(&w) {
            h.loading = if on { h.loading.or_else(|| Some(std::time::Instant::now())) } else { None };
        }
    }

    /// The page of `w` went to `url`, by its own doing: remember, so the
    /// name following it is not taken for a move to load; it is loading.
    pub fn navigated(&mut self, w: WindowId, url: &str) {
        if let Some(h) = self.hosts.get_mut(&w) {
            h.url = url.to_string();
            h.loading = Some(std::time::Instant::now());
        }
    }

    /// Load `url` in window `w`'s page (a loopback link, under the alias).
    pub fn load(&mut self, w: WindowId, url: &str) {
        if let Some(h) = self.hosts.get_mut(&w) {
            h.url = url.to_string();
            h.loading = Some(std::time::Instant::now());
            let _ = h.view.load_url(&webkit_url(url));
        }
    }

    /// Scroll window `w`'s page to the block whose marker (`data-line`,
    /// as `apex md` writes them) is the last at or before `line`: the
    /// preview follows dot in its source (WEB.md §3.3). Nothing happens
    /// when the page carries no markers.
    pub fn follow_line(&mut self, w: WindowId, line: usize) {
        let Some(h) = self.hosts.get_mut(&w) else { return };
        if h.followed == Some(line) {
            return;
        }
        h.followed = Some(line);
        let js = format!(
            r#"(function(){{
const want = {line};
let best = null, bestLine = -1;
for (const el of document.querySelectorAll('[data-line]')) {{
  const n = parseInt(el.getAttribute('data-line'), 10);
  if (!isNaN(n) && n <= want && n > bestLine) {{ best = el; bestLine = n; }}
}}
if (best) {{
  const y = best.getBoundingClientRect().top + window.scrollY - Math.floor(window.innerHeight / 4);
  window.scrollTo({{ top: Math.max(0, y), behavior: 'auto' }});
}}
}})();"#
        );
        let _ = h.view.evaluate_script(&js);
    }

    /// Load the page again (a host file it uses changed). A page from a
    /// buffer is loaded from its text again at the next placement.
    pub fn reload(&mut self, w: WindowId) {
        if let Some(h) = self.hosts.get_mut(&w) {
            match &mut h.html {
                Some((version, _)) => *version = u64::MAX, // stale: the next place() morphs it in
                None => {
                    let _ = h.view.reload();
                }
            }
        }
    }

    /// What the pages reported since the last call.
    pub fn drain(&mut self) -> Vec<(WindowId, WebEvent)> {
        let mut out = Vec::new();
        while let Ok(ev) = self.rx.try_recv() {
            out.push(ev);
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }

    /// Made over a plane (with a proxy), or on its own?
    pub fn armed(&self) -> bool {
        self.plane.is_some()
    }
}

/// Answers `apexfile://` requests for one page: the file from the host
/// over the plane (or the disk, with no plane), watched from then on so
/// a change reloads the page.
#[derive(Clone)]
struct Fetcher {
    plane: Option<IoPlane>,
    watches: Arc<Mutex<HashMap<String, u32>>>,
    events: Sender<(WindowId, WebEvent)>,
    wake: Option<Wake>,
    window: WindowId,
}

impl Fetcher {
    fn serve(&self, request: wry::http::Request<Vec<u8>>, responder: wry::RequestAsyncResponder) {
        let url = request.uri().to_string();
        let debug = std::env::var_os("APEX_WEB_DEBUG").is_some();
        if debug {
            eprintln!("web: apexfile request {url} (plane: {})", self.plane.is_some());
        }
        // the bundled fonts, from the client itself (`fonts::page_css`
        // names them here, on the page's own scheme and origin)
        if let Some((bytes, mime)) = url.strip_prefix("apexfile://localhost").and_then(crate::fonts::serve) {
            return respond(responder, 200, mime, bytes.to_vec());
        }
        let path = match file_url_path(&format!("file://{}", url.strip_prefix("apexfile://").unwrap_or(&url))) {
            Some(p) => p,
            None => return respond(responder, 400, "text/plain", format!("{url}: not a host file").into_bytes()),
        };
        let path = path.display().to_string();
        let (status, body) = match &self.plane {
            Some(plane) => match plane.fetch("GET", &file_url(&path), &[], None, Duration::from_secs(30)) {
                Ok((status, _, body)) => (status, body),
                Err(e) => (502, e.into_bytes()),
            },
            None => match std::fs::read(&path) {
                Ok(b) => (200, b),
                Err(e) => (if e.kind() == std::io::ErrorKind::NotFound { 404 } else { 500 }, format!("{path}: {e}").into_bytes()),
            },
        };
        if status == 200 {
            self.watch(&path);
        }
        let mime = if status == 200 { mime_for(&path) } else { "text/plain; charset=utf-8" };
        if debug {
            eprintln!("web: apexfile {path}: {status}, {} bytes, {mime}", body.len());
        }
        respond(responder, status, mime, body);
    }

    /// Watch `path` on the host (once per page): a change after the
    /// first frame reloads the page.
    fn watch(&self, path: &str) {
        let Some(plane) = &self.plane else { return };
        {
            let w = self.watches.lock().unwrap();
            if w.contains_key(path) || w.len() >= 200 {
                return;
            }
        }
        let (stream, rx) = plane.open("GET", &file_url(path), &[("Watch", "1")]);
        self.watches.lock().unwrap().insert(path.to_string(), stream);
        let (events, wake, window, watches) = (self.events.clone(), self.wake.clone(), self.window, self.watches.clone());
        let path = path.to_string();
        std::thread::spawn(move || {
            let mut first = true;
            loop {
                match rx.recv() {
                    Ok(IoFrame::Body(b)) => {
                        if first {
                            first = false; // the file now, which the page already has
                            continue;
                        }
                        if FileFrame::decode(&b).is_some() {
                            let _ = events.send((window, WebEvent::Reload));
                            if let Some(k) = &wake {
                                k();
                            }
                        }
                    }
                    Ok(IoFrame::Response { .. }) => {}
                    _ => break,
                }
            }
            watches.lock().unwrap().remove(&path);
        });
    }
}

/// Give the keyboard back to gpui's own view: a click in acme's part of
/// the window after a page had it. A web view that is the window's
/// first responder keeps it until someone takes it, and keys then reach
/// gpui by a roundabout route (WebKit passing them up the responder
/// chain) that delivers them twice.
#[cfg(target_os = "macos")]
pub fn focus_ui(window: &Window) {
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    // gpui's own `window_handle` is another thing: the trait's, by name
    let Ok(h) = HasWindowHandle::window_handle(window) else { return };
    let RawWindowHandle::AppKit(h) = h.as_raw() else { return };
    let view = h.ns_view.as_ptr() as *mut objc::runtime::Object;
    // SAFETY: the view is gpui's own NSView, alive while the window is;
    // plain AppKit messages on the main thread.
    unsafe {
        let ns_window: *mut objc::runtime::Object = msg_send![view, window];
        if ns_window.is_null() {
            return;
        }
        let first: *mut objc::runtime::Object = msg_send![ns_window, firstResponder];
        if first != view {
            let _: bool = msg_send![ns_window, makeFirstResponder: view];
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn focus_ui(_window: &Window) {}

/// Whether ⌘ and control are down now, asked of the system rather than
/// of the events gpui was sent: a page that had the keys hears a
/// modifier come up, and gpui never does.
#[cfg(target_os = "macos")]
pub fn modifiers_down() -> (bool, bool) {
    use objc::{class, msg_send, sel, sel_impl};
    // SAFETY: a class method answering the current modifier flags
    let flags: u64 = unsafe { msg_send![class!(NSEvent), modifierFlags] };
    (flags & (1 << 20) != 0, flags & (1 << 18) != 0)
}

#[cfg(not(target_os = "macos"))]
pub fn modifiers_down() -> (bool, bool) {
    (true, true)
}

/// AppKit's own title bar container, shown or not. In full screen AppKit
/// slides it down with the menu bar when the pointer reaches the top, an
/// empty bar over our strip; hidden, our strip is what comes.
#[cfg(target_os = "macos")]
pub fn set_native_titlebar_hidden(window: &Window, hidden: bool) {
    use objc::runtime::Object;
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(h) = HasWindowHandle::window_handle(window) else { return };
    let RawWindowHandle::AppKit(h) = h.as_raw() else { return };
    let view = h.ns_view.as_ptr() as *mut Object;
    // SAFETY: gpui's own NSView, alive while the window is; AppKit
    // messages on the main thread. The container is the close button's
    // grandparent (NSTitlebarContainerView), as gpui finds it too.
    unsafe {
        let ns_window: *mut Object = msg_send![view, window];
        if ns_window.is_null() {
            return;
        }
        let close: *mut Object = msg_send![ns_window, standardWindowButton: 0u64];
        if close.is_null() {
            return;
        }
        let buttons: *mut Object = msg_send![close, superview];
        if buttons.is_null() {
            return;
        }
        let container: *mut Object = msg_send![buttons, superview];
        if container.is_null() {
            return;
        }
        let _: () = msg_send![container, setHidden: hidden];
        let alpha: f64 = if hidden { 0. } else { 1. };
        let _: () = msg_send![container, setAlphaValue: alpha];
    }
}

#[cfg(not(target_os = "macos"))]
pub fn set_native_titlebar_hidden(_window: &Window, _hidden: bool) {}

/// A page's foot-left corner rounded as its card's is (the card's radius
/// less the inset the page stands in by, `PAGE_INSET`), so the ring round
/// the window the keys go to shows there: the page is a native view over
/// everything gpui draws. Its right is the scrollbar's, drawn by gpui.
#[cfg(target_os = "macos")]
fn round_foot(view: &wry::WebView) {
    use objc::runtime::{Object, YES};
    use objc::{msg_send, sel, sel_impl};
    use wry::WebViewExtMacOS;
    let wk = view.webview();
    let v = &*wk as *const _ as *mut Object;
    // SAFETY: the WKWebView was just made and is alive; CoreAnimation on
    // the main thread.
    unsafe {
        let _: () = msg_send![v, setWantsLayer: YES];
        let layer: *mut Object = msg_send![v, layer];
        if layer.is_null() {
            return;
        }
        let flipped: bool = msg_send![layer, isGeometryFlipped];
        // kCALayerMinXMinYCorner (1) is the foot when y goes up, else
        // kCALayerMinXMaxYCorner (4)
        let corner: usize = if flipped { 4 } else { 1 };
        let _: () = msg_send![layer, setCornerRadius: (crate::text_element::CARD_RADIUS - PAGE_INSET) as f64];
        let _: () = msg_send![layer, setMaskedCorners: corner];
        let _: () = msg_send![layer, setMasksToBounds: YES];
    }
}

#[cfg(not(target_os = "macos"))]
fn round_foot(_view: &wry::WebView) {}

/// How far a page stands in from its card's left and foot: the width of
/// the ring round the window the keys go to, which a native view would
/// cover.
pub const PAGE_INSET: f32 = 2.;

/// The window's buttons shown or not, as Manifold shows them: with the
/// sidebar, faded in and out (AppKit's animator), and not to be pressed
/// while they are not there.
#[cfg(target_os = "macos")]
pub fn set_traffic_lights(window: &Window, visible: bool) {
    use objc::runtime::Object;
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(h) = HasWindowHandle::window_handle(window) else { return };
    let RawWindowHandle::AppKit(h) = h.as_raw() else { return };
    let view = h.ns_view.as_ptr() as *mut Object;
    // SAFETY: gpui's own NSView, alive while the window is; AppKit
    // messages on the main thread
    unsafe {
        let ns_window: *mut Object = msg_send![view, window];
        if ns_window.is_null() {
            return;
        }
        for kind in 0u64..3 {
            let b: *mut Object = msg_send![ns_window, standardWindowButton: kind];
            if b.is_null() {
                continue;
            }
            // as they are, not as they were last set: AppKit makes the
            // buttons anew now and then (a child window come or gone, the
            // key window changed), shown and enabled -- what is told only
            // on a change would leave them showing. Enabled is the mark:
            // set at once, where the alpha fades
            let alpha: f64 = if visible { 1. } else { 0. };
            let enabled: bool = msg_send![b, isEnabled];
            if enabled == visible {
                continue;
            }
            let animator: *mut Object = msg_send![b, animator];
            let _: () = msg_send![animator, setAlphaValue: alpha];
            let _: () = msg_send![b, setEnabled: visible];
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn set_traffic_lights(_window: &Window, _visible: bool) {}

/// What a view shows: a URL, or a buffer's HTML.
enum Page<'a> {
    Url(&'a str),
    Html { html: &'a str, version: u64, dir: &'a str },
}

impl WebHost {
    fn settle_view(&mut self, rect: wry::Rect, bounds: Bounds<Pixels>, visible: bool) {
        if self.bounds != Some(bounds) {
            let _ = self.view.set_bounds(rect);
            self.bounds = Some(bounds);
        }
        if self.shown != visible {
            let _ = self.view.set_visible(visible);
            self.shown = visible;
        }
    }
}

/// The HTML with a `<base>` on the window's directory on the host, so
/// relative links and resources resolve there, unless it brings its own.
/// A page from a buffer dressed for the editor: its base, and the
/// theme's colours as `--apex-*` (a page whose stylesheet uses them,
/// `apex md`'s, takes the editor's look; another is not touched).
fn dress(html: &str, dir: &str) -> String {
    let html = with_base(html, dir);
    let style = format!("<style id=\"apex-theme\">{}</style>", theme_css());
    let lower = html.to_ascii_lowercase();
    match lower.find("</head>") {
        Some(i) => format!("{}{}{}", &html[..i], style, &html[i..]),
        None => format!("{style}{html}"),
    }
}

fn with_base(html: &str, dir: &str) -> String {
    if dir.is_empty() || html.to_ascii_lowercase().contains("<base ") {
        return html.to_string();
    }
    let base = format!("<base href=\"apexfile://localhost{}/\">", dir.trim_end_matches('/'));
    let lower = html.to_ascii_lowercase();
    match lower.find("<head>") {
        Some(i) => format!("{}{}{}", &html[..i + 6], base, &html[i + 6..]),
        None => format!("{base}{html}"),
    }
}

/// A script that patches the document into `html` in place (a small
/// morphdom): nodes are matched by position and name, attributes and
/// text updated, so scroll position and page state survive re-renders.
fn morph_script(html: &str) -> String {
    let json = js_string(html);
    format!(
        r#"(function(){{
const doc = new DOMParser().parseFromString({json}, 'text/html');
function morph(a, b) {{
  if (a.nodeType !== b.nodeType || a.nodeName !== b.nodeName) {{ a.replaceWith(b.cloneNode(true)); return; }}
  if (a.nodeType === 1 && a.nodeName === 'PRE' && a.classList.contains('mermaid') && b.classList.contains('mermaid')) {{
    // a diagram: left drawn while its source is the same, else its new source
    if (a.dataset.apexSrc === b.textContent) return;
    a.textContent = b.textContent;
    delete a.dataset.apexSrc; delete a.dataset.apexDrawn;
    return;
  }}
  if (a.nodeType === 3 || a.nodeType === 8) {{ if (a.nodeValue !== b.nodeValue) a.nodeValue = b.nodeValue; return; }}
  if (a.nodeType === 1) {{
    for (const at of Array.from(a.attributes)) if (!b.hasAttribute(at.name)) a.removeAttribute(at.name);
    for (const at of Array.from(b.attributes)) if (a.getAttribute(at.name) !== at.value) a.setAttribute(at.name, at.value);
  }}
  const ac = Array.from(a.childNodes), bc = Array.from(b.childNodes);
  for (let i = 0; i < Math.max(ac.length, bc.length); i++) {{
    if (i >= bc.length) {{ ac[i].remove(); continue; }}
    if (i >= ac.length) {{ a.appendChild(bc[i].cloneNode(true)); continue; }}
    morph(ac[i], bc[i]);
  }}
}}
morph(document.head, doc.head);
morph(document.body, doc.body);
window.__apexMermaid && window.__apexMermaid();
}})();"#
    )
}

/// `s` as a JavaScript string literal.
fn js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            '<' => out.push_str("\\u003c"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A URL as the view must see it: WebKit dispatches a custom scheme only
/// with a host in the URL, so `apexfile:///path` loads as
/// `apexfile://localhost/path`; and the host's loopback goes under the
/// alias the proxy undoes, since a view never proxies a loopback name.
fn webkit_url(url: &str) -> String {
    match url.strip_prefix("apexfile:///") {
        Some(rest) => format!("apexfile://localhost/{rest}"),
        None => alias_loopback_url(url),
    }
}

/// The form the session names a page by, back from the view's; a
/// `file://` link is the host's file.
fn apex_url(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("file://") {
        let path = rest.strip_prefix("localhost").unwrap_or(rest);
        return format!("apexfile://{path}");
    }
    match url.strip_prefix("apexfile://localhost/") {
        Some(rest) => format!("apexfile:///{rest}"),
        None => unalias_url(url),
    }
}

/// The words a page says it answers, as `<meta name="apex-verbs"
/// content="Prev Next">` in its head: B2 on one of them in the page's tag
/// runs it in the page (its `apexVerb`) rather than as a command. A page
/// declares them, rather than apex guessing, so that a word the page does
/// not answer still means what it means anywhere else.
pub fn page_verbs(html: &str) -> Vec<String> {
    // the head is at the top; a page's body is no place to look for it
    let head = &html[..html.len().min(8192)];
    let Some(at) = head.find("name=\"apex-verbs\"") else { return Vec::new() };
    // the tag it is in
    let from = head[..at].rfind('<').unwrap_or(0);
    let to = head[at..].find('>').map(|i| at + i).unwrap_or(head.len());
    let tag = &head[from..to];
    let Some(c) = tag.find("content=\"") else { return Vec::new() };
    let rest = &tag[c + 9..];
    rest[..rest.find('"').unwrap_or(rest.len())].split_whitespace().map(String::from).collect()
}

/// A link to a file carrying a line (`?line=N`, or `#L123` as GitHub
/// writes it): the host's path, percent-decoded, and the line. `file://`
/// as anyone writes one, and `apexfile://` (the host's files through
/// apex): a page from a buffer has no origin WebKit will let reach a
/// `file:` URL -- it refuses the navigation outright, and the handler
/// never hears of it -- so a page apex writes itself (apex diff) links
/// through its own scheme, which is asked about like any other.
fn file_link(url: &str) -> Option<(String, usize)> {
    let rest = url.strip_prefix("file://").or_else(|| url.strip_prefix("apexfile://"))?;
    let (before_frag, frag) = rest.split_once('#').map(|(a, b)| (a, Some(b))).unwrap_or((rest, None));
    let (path_part, query) = before_frag.split_once('?').map(|(a, b)| (a, Some(b))).unwrap_or((before_frag, None));
    let line = query
        .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("line=")).and_then(|v| v.parse().ok()))
        .or_else(|| frag.and_then(|f| f.strip_prefix('L')).and_then(|v| v.split('-').next()).and_then(|v| v.parse().ok()))?;
    let path = file_url_path(&format!("file://{path_part}"))?;
    Some((path.display().to_string(), line))
}

/// Where the pointer is, in the window's own coordinates, asked of the
/// system: a native view keeps the pointer's moves over it to itself,
/// so gpui's last known position is stale there.
#[cfg(target_os = "macos")]
pub fn native_mouse(window: &Window) -> Option<Point<Pixels>> {
    use objc::{class, msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct P {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct R {
        origin: P,
        size: P,
    }
    let h = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(h) = h.as_raw() else { return None };
    let view = h.ns_view.as_ptr() as *mut objc::runtime::Object;
    // SAFETY: plain AppKit queries on the main thread, on gpui's own view
    unsafe {
        let ns_window: *mut objc::runtime::Object = msg_send![view, window];
        if ns_window.is_null() {
            return None;
        }
        let screen: P = msg_send![class!(NSEvent), mouseLocation];
        let in_window: P = msg_send![ns_window, convertPointFromScreen: screen];
        let frame: R = msg_send![view, frame];
        // AppKit's y grows upward from the bottom of the view; gpui's downward
        Some(Point { x: gpui::px((in_window.x - frame.origin.x) as f32), y: gpui::px((frame.size.y - (in_window.y - frame.origin.y)) as f32) })
    }
}

#[cfg(not(target_os = "macos"))]
pub fn native_mouse(_window: &Window) -> Option<Point<Pixels>> {
    None
}

fn respond(responder: wry::RequestAsyncResponder, status: u16, mime: &str, body: Vec<u8>) {
    let r = wry::http::Response::builder().status(status).header("Content-Type", mime).header("Access-Control-Allow-Origin", "*").body(body).unwrap();
    responder.respond(r);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_mermaid_unpacks_to_the_browser_build() {
        let js = mermaid_js();
        assert_eq!(js.len(), 5_575_485, "the size of mermaid 12.0.0's dist/mermaid.min.js");
        assert!(js.contains("globalThis[\"mermaid\"]"), "it defines the global the page's script uses");
    }

    #[test]
    fn a_pages_thumb_is_the_part_of_it_that_shows() {
        // not said yet, or all of it showing: the whole bar
        assert_eq!(thumb_of(None), (0., 1.));
        assert_eq!(thumb_of(Some((0., 500., 800.))), (0., 1.));
        // a page four views long, at the top, then halfway, then at the end
        assert_eq!(thumb_of(Some((0., 4000., 1000.))), (0., 0.25));
        assert_eq!(thumb_of(Some((2000., 4000., 1000.))), (0.5, 0.75));
        assert_eq!(thumb_of(Some((3000., 4000., 1000.))), (0.75, 1.));
        // overscrolled (a bounce past either end) stays in the bar
        assert_eq!(thumb_of(Some((-50., 4000., 1000.))), (0., 0.2375));
        assert_eq!(thumb_of(Some((3100., 4000., 1000.))), (0.775, 1.));
    }

    #[test]
    fn a_page_says_which_words_it_answers() {
        assert_eq!(page_verbs("<html><head><meta charset=\"utf-8\"><meta name=\"apex-verbs\" content=\"Prev Next\"><title>x</title>"), vec!["Prev", "Next"]);
        assert_eq!(page_verbs("<meta content=\"Up\" name=\"apex-verbs\">"), vec!["Up"], "in either order");
        assert!(page_verbs("<html><head><title>x</title></head><body>apex-verbs</body>").is_empty());
    }

    #[test]
    fn file_links_with_a_line_open_the_file_there() {
        assert_eq!(file_link("file:///a/b.md?line=12"), Some(("/a/b.md".to_string(), 12)));
        assert_eq!(file_link("file://localhost/a/b%20c.go?x=1&line=3"), Some(("/a/b c.go".to_string(), 3)));
        assert_eq!(file_link("file:///a/b.md#L7-L9"), Some(("/a/b.md".to_string(), 7)));
        assert_eq!(file_link("file:///a/b.md"), None);
        // apex's own scheme, as a page it writes links (apex diff)
        assert_eq!(file_link("apexfile://localhost/a/b%20c.rs?line=9"), Some(("/a/b c.rs".to_string(), 9)));
        assert_eq!(file_link("apexfile:///a/b.rs?line=2"), Some(("/a/b.rs".to_string(), 2)));
        assert_eq!(file_link("apexfile://localhost/a/b.html"), None, "no line: a page, as before");
        assert_eq!(file_link("https://x/?line=3"), None);
        assert_eq!(apex_url("file:///a/b.html"), "apexfile:///a/b.html");
        assert_eq!(webkit_url("apexfile:///a/b.html"), "apexfile://localhost/a/b.html");
        assert_eq!(webkit_url("http://localhost:8/"), "http://localhost.apex-host:8/");
    }
}

/// A page's holes to the pointer: AppKit gives a click to the view under
/// it, and a page's view lies over gpui's whatever is drawn in its holes
/// (they are its layer's mask, drawing only). So the page's view is
/// taught to answer no to a point in one of its holes (`hitTest:`), and
/// the click falls through to gpui's view beneath: a toast, a menu, a
/// palette over a page takes its own clicks.
#[cfg(target_os = "macos")]
mod hit {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, Once};

    use objc::runtime::{class_addMethod, class_getInstanceMethod, method_getImplementation, method_setImplementation, object_getClass, Class, Imp, Method, Object, Sel};
    use objc::{msg_send, sel, sel_impl};

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct P {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct R {
        origin: P,
        size: P,
    }

    /// Each page's view's holes, in its own coordinates from its top left.
    static HOLES: Mutex<Option<HashMap<usize, Vec<[f64; 4]>>>> = Mutex::new(None);
    /// The view class's own `hitTest:`, called for every other point.
    static ORIGINAL: AtomicUsize = AtomicUsize::new(0);
    static TAUGHT: Once = Once::new();

    /// Page view `view`'s holes now (none: its clicks all its own).
    pub fn set(view: *mut Object, holes: Vec<[f64; 4]>) {
        teach(view);
        let mut m = HOLES.lock().unwrap();
        let m = m.get_or_insert_with(HashMap::new);
        if holes.is_empty() {
            m.remove(&(view as usize));
        } else {
            m.insert(view as usize, holes);
        }
    }

    /// The page views' class taught to pass a point in a hole on: added to
    /// the class as its own (wry's WKWebView subclass), or, where it has
    /// one of its own already, put in its place -- the old one called for
    /// every point that is not in a hole.
    fn teach(view: *mut Object) {
        TAUGHT.call_once(|| {
            // SAFETY: the Objective-C runtime on the main thread, on the
            // page view's own class; the method keeps `hitTest:`'s type
            unsafe {
                let cls = object_getClass(view as *const Object) as *mut Class;
                let sel = sel!(hitTest:);
                let m = class_getInstanceMethod(cls, sel);
                if m.is_null() {
                    return;
                }
                ORIGINAL.store(method_getImplementation(m) as usize, Ordering::SeqCst);
                let imp: Imp = std::mem::transmute(hit_test as extern "C" fn(&Object, Sel, P) -> *mut Object);
                let types = b"@32@0:8{CGPoint=dd}16\0";
                if class_addMethod(cls, sel, imp, types.as_ptr() as *const _) == objc::runtime::NO {
                    // the class's own: replaced (it is not a superclass's)
                    method_setImplementation(m as *mut Method, imp);
                }
            }
        });
    }

    extern "C" fn hit_test(this: &Object, cmd: Sel, point: P) -> *mut Object {
        let key = this as *const Object as usize;
        let inside = HOLES.lock().ok().and_then(|m| {
            let holes = m.as_ref()?.get(&key)?.clone();
            // SAFETY: plain AppKit geometry on the view being asked
            let (x, y) = unsafe {
                let sup: *mut Object = msg_send![this, superview];
                let local: P = msg_send![this, convertPoint: point fromView: sup];
                let flipped: bool = msg_send![this, isFlipped];
                let b: R = msg_send![this, bounds];
                (local.x, if flipped { local.y } else { b.size.y - local.y })
            };
            Some(holes.iter().any(|h| x >= h[0] && x < h[0] + h[2] && y >= h[1] && y < h[1] + h[3]))
        });
        if inside == Some(true) {
            return std::ptr::null_mut();
        }
        let original = ORIGINAL.load(Ordering::SeqCst);
        if original == 0 {
            return std::ptr::null_mut();
        }
        // SAFETY: the class's own `hitTest:` as it was, with its own type
        let f: extern "C" fn(&Object, Sel, P) -> *mut Object = unsafe { std::mem::transmute(original) };
        f(this, cmd, point)
    }
}
