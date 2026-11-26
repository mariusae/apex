//! Things dropped on apex from outside it: files from the Finder (a
//! screenshot's thumbnail among them), their paths put where they land.
//!
//! - On a terminal, the paths as Terminal.app types them: each escaped
//!   for a shell with backslashes, a space between and after, pasted
//!   (bracketed, where the program asks). An agent there takes a path to
//!   an image for the image (Claude Code's `[Image #1]`), as it does from
//!   Terminal.app.
//! - On a text, at the place under the pointer, selected as a paste is:
//!   escaped the same where the text goes to a shell (a live window, win's;
//!   an input strip, which an agent's lens types into its terminal), as
//!   they are anywhere else.
//!
//! A session on this Mac has the files where they are; one on another
//! host is given them first, each to `~/.apex/drops/KEY/NAME` there (its
//! own name, so an agent sees an image for what it is), and the paths
//! there put in once they are all there. Meanwhile the window is held, as
//! if the drop took that long: its keys thrown away (nothing typed lands
//! before the paths, or among them), its clicks too; a panel over it says
//! how far the files have got, and Escape (or its Stop, or the sidebar's
//! ×) stops them -- nothing put in, nothing left there. Other windows go
//! on as ever. The sidebar lists the transfer too (`transfers`).

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use gpui::{anchored, deferred, div, point, prelude::*, px, rgb, AnyElement, Context, MouseButton, Pixels, Point};

use apex_core::{ViewId, WindowId};

use crate::app::{Acme, Region, Target};

/// A path as Terminal.app types one dropped on it: every character a
/// shell would take for something else behind a backslash; letters of
/// any script, digits and `_ . / + , : @ % = -` as they are.
pub fn escape(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        let plain = c.is_alphanumeric() || matches!(c, '_' | '.' | '/' | '+' | ',' | ':' | '@' | '%' | '=' | '-') || (!c.is_ascii() && !c.is_whitespace() && !c.is_control());
        if !plain {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The paths as text for where they land: escaped and a space between
/// them for a shell, as they are a line each for a text.
pub fn words(paths: &[String], shell: bool) -> String {
    if shell {
        paths.iter().map(|p| escape(p)).collect::<Vec<_>>().join(" ")
    } else {
        paths.join("\n")
    }
}

/// A window held while what was dropped on it goes to the host.
pub struct Held {
    /// Its transfer (`transfers`).
    pub transfer: u64,
}

impl Acme {
    /// Files dropped at `pos`: their paths where they land (above). Whether
    /// anything took them.
    pub fn drop_paths(&mut self, paths: &[PathBuf], pos: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let paths: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).filter(|p| !p.is_empty()).collect();
        if paths.is_empty() {
            return false;
        }
        let Some((target, region)) = self.locate(pos) else { return false };
        // this Mac's paths mean nothing on another host: the files go
        // there first
        if !self.url.is_local() {
            let files: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
            return self.drop_remote(target, region, files, cx);
        }
        let took = self.put_paths(target, region, &paths);
        if took {
            cx.notify();
        }
        took
    }

    /// Files dropped on a session on another host: sent there, the window
    /// held until they are (above). Whether the drop was taken.
    fn drop_remote(&mut self, target: Target, region: Region, files: Vec<PathBuf>, cx: &mut Context<Self>) -> bool {
        // only where the paths could go
        let w = match (target, region) {
            (Target::Term(w, _), _) | (Target::View(ViewId::Body(w) | ViewId::Tag(w) | ViewId::Input(w)), Region::Text(_)) => w,
            _ => return false,
        };
        if self.held.contains_key(&w) {
            return false;
        }
        if let Some(f) = files.iter().find(|f| !f.is_file()) {
            self.notice(&format!("{}: only files go to another host, not folders\n", f.display()));
            cx.notify();
            return false;
        }
        let crate::app::Backend::Remote(link) = &self.backend else { return false };
        let (transfer, rx) = crate::transfers::upload(link.io_plane(), files, &self.url.arg);
        self.held.insert(w, Held { transfer });
        self.watch_held(w, target, region, rx, cx);
        cx.notify();
        true
    }

    /// Until the files are there: drawn again as they go (the panel, the
    /// sidebar); then the window let go and the paths put in where they
    /// were dropped -- or, stopped, nothing; failed, why, in the errors.
    /// Drawn on a while after, as the sidebar says it finished.
    fn watch_held(&mut self, w: WindowId, target: Target, region: Region, rx: Receiver<crate::transfers::Landed>, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut waiting = true;
            loop {
                cx.background_executor().timer(Duration::from_millis(if waiting { 50 } else { 250 })).await;
                let go_on = this.update(cx, |this, cx| {
                    cx.notify();
                    if !waiting {
                        return crate::transfers::live();
                    }
                    let landed = match rx.try_recv() {
                        Ok(r) => r,
                        Err(TryRecvError::Empty) => return true,
                        Err(TryRecvError::Disconnected) => Err("the upload ended unanswered".into()),
                    };
                    waiting = false;
                    this.held.remove(&w);
                    match landed {
                        Ok(paths) if this.node.state.window(w).is_ok() => {
                            this.put_paths(target, region, &paths);
                        }
                        Ok(_) => {}
                        Err(e) if e == "stopped" => {}
                        Err(e) => this.notice(&format!("drop: {e}\n")),
                    }
                    true
                });
                if !go_on.unwrap_or(false) {
                    break;
                }
            }
        })
        .detach();
    }

    /// Over each window held: what is going, how far it has got, and how
    /// to stop it.
    pub fn held_panels(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = crate::theme::theme();
        let now = std::time::Instant::now();
        let mut out = Vec::new();
        for (w, h) in &self.held {
            let bounds = self.term_layouts.get(w).map(|l| l.bounds).or_else(|| self.layouts.get(&ViewId::Body(*w)).map(|l| l.bounds));
            let (Some(b), Some(it)) = (bounds, crate::transfers::item(h.transfer)) else { continue };
            let id = h.transfer;
            let width = (f32::from(b.size.width) - 24.).clamp(160., 380.);
            let stop = div()
                .id(("held-stop", id))
                .flex_none()
                .px(px(8.))
                .h(px(20.))
                .rounded(px(5.))
                .flex()
                .items_center()
                .border_1()
                .border_color(rgb(t.panel_border))
                .text_size(px(12.))
                .cursor_default()
                .hover(move |s| s.bg(rgb(t.panel_hover)))
                .child("Stop")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |_, _, _, cx| {
                        crate::transfers::stop(id);
                        cx.stop_propagation();
                        cx.notify();
                    }),
                );
            let panel = crate::shell::popover()
                .id(("held", id))
                .w(px(width))
                .p(px(10.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .text_color(rgb(t.panel_text))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.))
                        .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_size(px(13.)).child(format!("Sending {}", it.title)))
                        .child(stop),
                )
                .child(div().h(px(3.)).w_full().rounded(px(2.)).bg(rgb(t.panel_hover)).child(div().h_full().rounded(px(2.)).w(gpui::relative(it.fraction())).bg(rgb(t.accent))))
                .child(div().overflow_hidden().text_ellipsis().whitespace_nowrap().text_size(px(11.)).text_color(rgb(t.panel_dim)).child(format!("{} · esc to stop", it.said(now))));
            let at = point(b.left() + (b.size.width - px(width)) / 2., b.top() + px(12.));
            out.push(deferred(anchored().anchor(gpui::Anchor::TopLeft).position(at).child(panel)).with_priority(2).into_any_element());
        }
        out
    }

    /// The paths (each on the host the session is on) put into `target`
    /// at `region`: whether it took them.
    pub(crate) fn put_paths(&mut self, target: Target, region: Region, paths: &[String]) -> bool {
        match target {
            Target::Term(w, t) => {
                self.attend(w);
                // as Terminal.app types them: a space after, for the next
                self.term_paste(t, format!("{} ", words(paths, true)));
                true
            }
            Target::View(v) => {
                let Region::Text(at) = region else { return false };
                // (the text may have changed while the files went)
                let at = self.text_of(v).map_or(0, |t| at.min(t.len()));
                let shell = matches!(v, ViewId::Input(_)) || v.window().is_some_and(|w| self.node.window_live(w));
                let text = words(paths, shell);
                if self.node.select(&mut self.log, v, at, at).is_err() || self.node.replace_selection(&mut self.log, v, &text).is_err() {
                    return false;
                }
                self.node.seltext = Some(v);
                self.want_visible.insert(v);
                self.after();
                true
            }
            // a page takes its own drops (WebKit's)
            Target::Web(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_escaped_as_terminal_app_escapes_it() {
        assert_eq!(escape("/Users/a/notes.md"), "/Users/a/notes.md");
        assert_eq!(escape("/Users/a/Screenshot 2026-11-26 at 15.20.png"), "/Users/a/Screenshot\\ 2026-11-26\\ at\\ 15.20.png");
        assert_eq!(escape("/tmp/it's (1) & $x*?[a]~"), "/tmp/it\\'s\\ \\(1\\)\\ \\&\\ \\$x\\*\\?\\[a\\]\\~");
        assert_eq!(escape("/tmp/a\\b\"c"), "/tmp/a\\\\b\\\"c");
        // letters of any script stay; a space of any kind does not
        assert_eq!(escape("/tmp/Überblick 日本.txt"), "/tmp/Überblick\\ 日本.txt");
        assert_eq!(escape("/tmp/a\u{202f}PM.png"), "/tmp/a\\\u{202f}PM.png");
    }

    #[test]
    fn paths_are_words_for_a_shell_and_lines_for_a_text() {
        let ps = vec!["/a b".to_string(), "/c".to_string()];
        assert_eq!(words(&ps, true), "/a\\ b /c");
        assert_eq!(words(&ps, false), "/a b\n/c");
    }
}
