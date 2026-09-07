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
}

/// One window's view.
pub struct WebHost {
    view: wry::WebView,
    /// The URL we loaded or were told the page went to: when the window's
    /// name differs, the state moved the page and we load it.
    url: String,
    bounds: Option<Bounds<Pixels>>,
    shown: bool,
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
        let rect = Self::rect(bounds);
        if !self.hosts.contains_key(&w) {
            let watches: Arc<Mutex<HashMap<String, u32>>> = Arc::new(Mutex::new(HashMap::new()));
            let (tx1, tx2) = (self.tx.clone(), self.tx.clone());
            let (wake1, wake2) = (self.wake.clone(), self.wake.clone());
            let mut b = wry::WebViewBuilder::new()
                .with_url(&webkit_url(url))
                .with_bounds(rect)
                .with_navigation_handler(move |u| {
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
            match b.build_as_child(window) {
                Ok(view) => {
                    let _ = view.set_visible(visible);
                    self.hosts.insert(w, WebHost { view, url: url.to_string(), bounds: Some(bounds), shown: visible, watches, plane: self.plane.clone() });
                }
                Err(e) => eprintln!("web: {url}: {e}"),
            }
            return;
        }
        let h = self.hosts.get_mut(&w).unwrap();
        if h.url != url {
            // the state moved the page (a Goto, another client): follow
            h.url = url.to_string();
            let _ = h.view.load_url(&webkit_url(url));
        }
        if h.bounds != Some(bounds) {
            let _ = h.view.set_bounds(rect);
            h.bounds = Some(bounds);
        }
        if h.shown != visible {
            let _ = h.view.set_visible(visible);
            h.shown = visible;
        }
    }

    /// Hide every view not in `shown` (windows the layout does not draw:
    /// obscured by a full-column window, no body room), and drop the
    /// views of windows that are gone.
    pub fn settle(&mut self, shown: &HashSet<WindowId>, alive: impl Fn(WindowId) -> bool) {
        self.hosts.retain(|w, _| alive(*w));
        for (w, h) in self.hosts.iter_mut() {
            if !shown.contains(w) && h.shown {
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

    /// Load the page again (a host file it uses changed).
    pub fn reload(&self, w: WindowId) {
        if let Some(h) = self.hosts.get(&w) {
            let _ = h.view.reload();
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
