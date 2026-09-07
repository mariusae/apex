//! Web windows on the client (WEB.md §2): a native web view (`wry`,
//! WebKit) as a child view of the gpui window, placed over the body
//! rectangle the layout gives a `Body::Web` window, hidden while a gpui
//! overlay (the tools menu, the finder, the picker) would be painted
//! under it. The page's navigations come back as events the app turns
//! into `WebNavigate` proposals, so the window's name follows the page.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, Sender};

use gpui::{Bounds, Pixels, Window};

use apex_core::WindowId;

pub enum WebEvent {
    /// The page went to `url` (a link, a redirect, a form).
    Navigated(String),
    /// The document's title changed (not kept yet: WEB.md §2.1).
    #[allow(dead_code)]
    Title(String),
}

/// One window's view.
pub struct WebHost {
    view: wry::WebView,
    /// The URL we loaded or were told the page went to: when the window's
    /// name differs, the state moved the page and we load it.
    url: String,
    bounds: Option<Bounds<Pixels>>,
    shown: bool,
}

pub struct Webs {
    hosts: HashMap<WindowId, WebHost>,
    tx: Sender<(WindowId, WebEvent)>,
    rx: Receiver<(WindowId, WebEvent)>,
}

impl Default for Webs {
    fn default() -> Self {
        Self::new()
    }
}

impl Webs {
    pub fn new() -> Webs {
        let (tx, rx) = channel();
        Webs { hosts: HashMap::new(), tx, rx }
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
            let (tx1, tx2) = (self.tx.clone(), self.tx.clone());
            let built = wry::WebViewBuilder::new()
                .with_url(url)
                .with_bounds(rect)
                .with_navigation_handler(move |u| {
                    let _ = tx1.send((w, WebEvent::Navigated(u)));
                    true
                })
                .with_document_title_changed_handler(move |t| {
                    let _ = tx2.send((w, WebEvent::Title(t)));
                })
                .build_as_child(window);
            match built {
                Ok(view) => {
                    let _ = view.set_visible(visible);
                    self.hosts.insert(w, WebHost { view, url: url.to_string(), bounds: Some(bounds), shown: visible });
                }
                Err(e) => eprintln!("web: {url}: {e}"),
            }
            return;
        }
        let h = self.hosts.get_mut(&w).unwrap();
        if h.url != url {
            // the state moved the page (a Goto, another client): follow
            h.url = url.to_string();
            let _ = h.view.load_url(url);
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
}
