//! `apex tool web` (ARCHITECTURE.md §5): pages on the web, kept by a
//! tool and not by apex. It answers `Web` (the address given, else the
//! window's selection: a URL, or a path that is the host's file; with
//! neither, a blank page whose address is typed) and `Newweb URL`, in
//! any window, a column's tag or the top row; it owns the pages they make
//! and every other page at an address nobody owns, taking them back when
//! it starts again. Each page's history is its own: the log holds only
//! where the page is, and Back, Fwd and Get in its tag are answered here
//! (Back and Fwd by navigating, Get by a reload every client follows).
//! A link followed goes where it goes on the web, the host's files
//! included; one of another kind (`mailto:`) does what the client does
//! with it.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use apex_core::entry::RuleAction;
use apex_core::{Body, Source, WinKind, WindowId};
use apex_tool::{Event, NavAnswer, PageEvent, Plumb, Rule, Tool};

/// A page's history: where it has been, and where in that it is.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct History {
    pub places: Vec<String>,
    pub at: usize,
    /// Where a Back or Fwd of ours sent it: its arrival there is that
    /// step, not a new place.
    stepping: Option<String>,
}

impl History {
    pub fn new(url: &str) -> History {
        History { places: if url.is_empty() { Vec::new() } else { vec![url.to_string()] }, at: 0, stepping: None }
    }

    /// The page arrived at `url`: a step of ours, or a new place (the
    /// places ahead of here forgotten, as a browser forgets them).
    pub fn arrived(&mut self, url: &str) {
        if self.stepping.as_deref() == Some(url) {
            self.stepping = None;
            return;
        }
        if self.places.get(self.at).map(String::as_str) == Some(url) {
            return;
        }
        if !self.places.is_empty() {
            self.places.truncate(self.at + 1);
        }
        self.places.push(url.to_string());
        self.at = self.places.len() - 1;
    }

    /// Back (`by` -1) or Fwd (+1): the place to go, if there is one.
    pub fn step(&mut self, by: isize) -> Option<String> {
        let to = self.at.checked_add_signed(by).filter(|i| *i < self.places.len())?;
        self.at = to;
        let url = self.places[to].clone();
        self.stepping = Some(url.clone());
        Some(url)
    }
}

/// The address `Web` opens for `target` (an argument or a selection), from
/// a window whose directory is `dir`: a URL as it is; `file://` and a path
/// the host's file (`apexfile://`, which the client serves from the host).
pub fn web_url(target: &str, dir: &str) -> String {
    if let Some(rest) = target.strip_prefix("file://") {
        let path = rest.strip_prefix("localhost").unwrap_or(rest);
        return format!("apexfile://{path}");
    }
    if apex_core::is_url(target) {
        return target.to_string();
    }
    if target.starts_with('/') {
        return format!("apexfile://{target}");
    }
    let dir = dir.trim_end_matches('/');
    format!("apexfile://{dir}/{target}")
}

/// Where a followed link goes, as Web says: on the web and the host's
/// files, there; anything else (a `mailto:`), what the client does.
pub fn where_to(url: &str) -> NavAnswer {
    let web = ["http://", "https://", "apexfile://", "file://", "tool://", "about:"];
    if web.iter().any(|s| url.starts_with(s)) {
        NavAnswer::Allow
    } else {
        NavAnswer::Default
    }
}

/// The tool: attached as `web` until the session ends.
pub fn run(socket: &Path, session: &str) -> Result<(), String> {
    let mut t = Tool::attach_to(socket, session, "web").map_err(|e| e.to_string())?;
    t.handle_pages();
    // the verbs: anywhere (unlisted: every menu would have them), unless
    // the session's rules already send them here; and its pages' own words
    let session_has = |t: &Tool, verb: &str| {
        t.meta().rules.values().any(|r| r.rule.verb == verb && r.rule.action == RuleAction::Tool("web".into()))
    };
    let mut rules = vec![];
    for verb in ["Web", "Newweb"] {
        if !session_has(&t, verb) {
            rules.push(Rule::verb(verb).unlisted());
        }
    }
    for verb in ["Back", "Fwd", "Get"] {
        rules.push(Rule::verb(verb).owner("^web$").kind(WinKind::Page));
    }
    for rule in rules {
        t.offer(rule).map_err(|e| e.to_string())?;
    }
    let mut pages: BTreeMap<WindowId, History> = BTreeMap::new();
    let mut idle_since = Some(Instant::now());
    loop {
        adopt(&mut t, &mut pages);
        // gone when it has had no page for a while: the session's rule
        // starts it again (ARCHITECTURE.md §5)
        match (pages.is_empty(), idle_since) {
            (true, None) => idle_since = Some(Instant::now()),
            (true, Some(at)) if at.elapsed() >= IDLE => return Ok(()),
            (true, Some(_)) => {}
            (false, _) => idle_since = None,
        }
        match t.next_event(Some(Duration::from_millis(500))) {
            Ok(Some(Event::Plumb(p))) => {
                let taken = verb(&mut t, &mut pages, &p);
                let _ = t.answer(&p, taken);
            }
            Ok(Some(Event::Navigate(n))) => {
                let answer = where_to(&n.url);
                let _ = t.answer_navigation(&n, answer);
            }
            Ok(Some(Event::Page { window, event: PageEvent::Navigated { url } })) => pages.entry(window).or_default().arrived(&url),
            Ok(Some(Event::Deleted { window })) => {
                pages.remove(&window);
            }
            Err(e) if e.is_closed() => return Ok(()),
            _ => {}
        }
    }
}

/// How long the tool stays with no page of its.
const IDLE: Duration = Duration::from_secs(60);

/// Every page at an address that nobody owns is ours (a restart's pages,
/// a link's): its history begins where it is.
fn adopt(t: &mut Tool, pages: &mut BTreeMap<WindowId, History>) {
    let orphans: Vec<(WindowId, String)> = t
        .windows()
        .into_iter()
        .filter(|w| w.kind == WinKind::Page && !pages.contains_key(&w.id))
        .filter(|w| t.window_body(w.id) == Some(Body::Page(Source::Url)) && t.window_owner(w.id).is_none())
        .map(|w| (w.id, w.path))
        .collect();
    for (w, url) in orphans {
        if t.set_owner(w, true).is_ok() {
            pages.insert(w, History::new(&url));
        }
    }
}

/// A verb of ours, B2'd: whether it was ours to do.
fn verb(t: &mut Tool, pages: &mut BTreeMap<WindowId, History>, p: &Plumb) -> bool {
    match p.verb.as_str() {
        "Web" | "Newweb" => {
            // the address given, else the window's selection
            let mut target = p.text.trim().to_string();
            if target.is_empty() && p.verb == "Web" {
                if let (Some(w), Some(r)) = (p.window, p.at) {
                    target = t.read(w).map(|text| text.chars().skip(r.q0).take(r.q1.saturating_sub(r.q0)).collect::<String>()).unwrap_or_default().trim().to_string();
                }
            }
            if target.is_empty() && p.verb == "Newweb" {
                let _ = t.errors(Some(&p.dir), "Newweb needs a URL\n");
                return true;
            }
            let url = if target.is_empty() { String::new() } else { web_url(&target, &p.dir) };
            match t.new_web_page(&url, p.window) {
                Ok(w) => {
                    pages.insert(w, History::new(&url));
                    true
                }
                Err(e) => {
                    let _ = t.errors(Some(&p.dir), &format!("{}: {e}\n", p.verb));
                    true
                }
            }
        }
        "Back" | "Fwd" => {
            let Some(w) = p.window else { return false };
            let by = if p.verb == "Back" { -1 } else { 1 };
            if let Some(url) = pages.entry(w).or_default().step(by) {
                let _ = t.navigate(w, &url);
            }
            true
        }
        "Get" => {
            let Some(w) = p.window else { return false };
            let _ = t.reload(w);
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pages_history_goes_back_and_forth_and_forgets_what_was_ahead() {
        let mut h = History::new("a");
        h.arrived("b");
        h.arrived("c");
        assert_eq!(h.step(-1).as_deref(), Some("b"));
        h.arrived("b"); // the step itself, arriving
        assert_eq!(h.step(-1).as_deref(), Some("a"));
        h.arrived("a");
        assert_eq!(h.step(-1), None);
        assert_eq!(h.step(1).as_deref(), Some("b"));
        h.arrived("b");
        // somewhere new from here: c is forgotten
        h.arrived("d");
        assert_eq!(h.places, vec!["a", "b", "d"]);
        assert_eq!(h.step(1), None);
    }

    #[test]
    fn web_opens_a_url_as_it_is_and_a_path_as_the_hosts_file() {
        assert_eq!(web_url("https://example.com/x", "/d"), "https://example.com/x");
        assert_eq!(web_url("file:///tmp/a.html", "/d"), "apexfile:///tmp/a.html");
        assert_eq!(web_url("/tmp/a.html", "/d"), "apexfile:///tmp/a.html");
        assert_eq!(web_url("a.html", "/d/"), "apexfile:///d/a.html");
    }

    #[test]
    fn links_go_on_the_web_and_anything_else_to_the_client() {
        assert_eq!(where_to("https://example.com/"), NavAnswer::Allow);
        assert_eq!(where_to("apexfile:///tmp/x.html"), NavAnswer::Allow);
        assert_eq!(where_to("mailto:x@example.com"), NavAnswer::Default);
    }
}
