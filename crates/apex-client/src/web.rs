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

use gpui::{Bounds, Pixels, Window};

use apex_core::WindowId;
use apex_server::plane::{mime_for, start_connect_proxy, IoPlane};
use apex_server::proto::{file_url_path, FileFrame, IoFrame};
use apex_server::remote::{file_url, Wake};

pub enum WebEvent {
    /// The page went to `url` (a link, a redirect, a form).
    Navigated(String),
    /// The document's title changed (not kept yet: WEB.md §2.1).
    #[allow(dead_code)]
    Title(String),
    /// A host file the page uses changed: load it again.
    Reload,
    /// A link followed in a page rendered from a buffer: open it.
    Link(String),
}

/// One window's view.
pub struct WebHost {
    view: wry::WebView,
    /// The URL we loaded or were told the page went to: when the window's
    /// name differs, the state moved the page and we load it.
    url: String,
    bounds: Option<Bounds<Pixels>>,
    shown: bool,
    /// A page from a buffer: the buffer version shown, and the directory
    /// its relative links resolve in.
    html: Option<(u64, String)>,
    /// The source line the page was last scrolled to follow.
    followed: Option<usize>,
    /// Watch streams on the host files this page fetched, by path.
    watches: Arc<Mutex<HashMap<String, u32>>>,
    plane: Option<IoPlane>,
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

pub struct Webs {
    hosts: HashMap<WindowId, WebHost>,
    tx: Sender<(WindowId, WebEvent)>,
    rx: Receiver<(WindowId, WebEvent)>,
    /// The session's I/O plane, and the proxy's port on it.
    plane: Option<IoPlane>,
    proxy: Option<u16>,
    /// Wakes the UI when a page reports something.
    wake: Option<Wake>,
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
        Webs { hosts: HashMap::new(), tx, rx, plane, proxy, wake }
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
            let _ = h.view.evaluate_script(&morph_script(&with_base(html, dir)));
        }
        h.settle_view(rect, bounds, visible);
    }

    fn build(&mut self, w: WindowId, page: Page, bounds: Bounds<Pixels>, window: &Window, visible: bool) {
        let rect = Self::rect(bounds);
        let watches: Arc<Mutex<HashMap<String, u32>>> = Arc::new(Mutex::new(HashMap::new()));
        let (tx1, tx2) = (self.tx.clone(), self.tx.clone());
        let (wake1, wake2) = (self.wake.clone(), self.wake.clone());
        let from_buffer = matches!(page, Page::Html { .. });
        let mut b = wry::WebViewBuilder::new().with_bounds(rect);
        b = match page {
            Page::Url(url) => b.with_url(&webkit_url(url)),
            Page::Html { html, dir, .. } => b.with_html(with_base(html, dir)),
        };
        b = b
            .with_navigation_handler(move |u| {
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
                let _ = tx1.send((w, WebEvent::Navigated(apex_url(&u))));
                if let Some(k) = &wake1 {
                    k();
                }
                true
            })
            .with_document_title_changed_handler(move |t| {
                let _ = tx2.send((w, WebEvent::Title(t)));
                if let Some(k) = &wake2 {
                    k();
                }
            });
        if let Some(port) = self.proxy {
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
                self.hosts.insert(w, WebHost { view, url, bounds: Some(bounds), shown: visible, html, followed: None, watches, plane: self.plane.clone() });
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
    }

    /// The page of `w` went to `url`, by its own doing: remember, so the
    /// name following it is not taken for a move to load.
    pub fn navigated(&mut self, w: WindowId, url: &str) {
        if let Some(h) = self.hosts.get_mut(&w) {
            h.url = url.to_string();
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

/// WebKit dispatches a custom scheme only with a host in the URL: our
/// `apexfile:///path` loads as `apexfile://localhost/path`.
fn webkit_url(url: &str) -> String {
    match url.strip_prefix("apexfile:///") {
        Some(rest) => format!("apexfile://localhost/{rest}"),
        None => url.to_string(),
    }
}

/// The form the session names a host file by, back from WebKit's.
fn apex_url(url: &str) -> String {
    match url.strip_prefix("apexfile://localhost/") {
        Some(rest) => format!("apexfile:///{rest}"),
        None => url.to_string(),
    }
}

fn respond(responder: wry::RequestAsyncResponder, status: u16, mime: &str, body: Vec<u8>) {
    let r = wry::http::Response::builder().status(status).header("Content-Type", mime).header("Access-Control-Allow-Origin", "*").body(body).unwrap();
    responder.respond(r);
}
