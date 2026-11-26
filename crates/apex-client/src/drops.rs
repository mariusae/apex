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
//! host is given them first (`transfers`).

use std::path::PathBuf;

use gpui::{Context, Pixels, Point};

use apex_core::ViewId;

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

impl Acme {
    /// Files dropped at `pos`: their paths where they land (above). Whether
    /// anything took them.
    pub fn drop_paths(&mut self, paths: &[PathBuf], pos: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let paths: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).filter(|p| !p.is_empty()).collect();
        if paths.is_empty() {
            return false;
        }
        let Some((target, region)) = self.locate(pos) else { return false };
        // this Mac's paths mean nothing on another host
        if !self.url.is_local() {
            return false;
        }
        let took = self.put_paths(target, region, &paths);
        if took {
            cx.notify();
        }
        took
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
