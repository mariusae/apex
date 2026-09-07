//! Preview (WEB.md §3): a file shown as a page by piping it through a
//! converter, a command reading the source on stdin and writing HTML
//! on stdout, named by extension in the settings (`Preview.md`). Some
//! extensions have a converter unless a setting says otherwise; an
//! empty setting turns one off.

use std::collections::BTreeSet;

use crate::state::Meta;
use crate::SERVER;

/// What converts an extension when no setting names one.
pub const DEFAULTS: &[(&str, &str)] = &[("md", "apex md"), ("markdown", "apex md"), ("html", "cat"), ("htm", "cat"), ("svg", "cat")];

/// The converter for files with extension `ext`: the session's setting,
/// else any attachment's (an attach script's), else the default; none
/// when nothing names one, or a setting is empty.
pub fn converter(meta: &Meta, ext: &str) -> Option<String> {
    let key = format!("Preview.{}", ext.to_lowercase());
    let owners = std::iter::once(SERVER).chain(meta.settings.keys().copied().filter(|a| *a != SERVER));
    for owner in owners {
        if let Some(v) = meta.settings.get(&owner).and_then(|m| m.get(&key)) {
            return if v.trim().is_empty() { None } else { Some(v.clone()) };
        }
    }
    DEFAULTS.iter().find(|(e, _)| *e == ext.to_lowercase()).map(|(_, c)| c.to_string())
}

/// Every extension a converter exists for: what Preview is offered on.
pub fn exts(meta: &Meta) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = DEFAULTS.iter().map(|(e, _)| e.to_string()).collect();
    for m in meta.settings.values() {
        out.extend(m.keys().filter_map(|k| k.strip_prefix("Preview.")).filter(|e| !e.is_empty()).map(|e| e.to_lowercase()));
    }
    out.retain(|e| converter(meta, e).is_some());
    out
}

/// The `-file` pattern of a Preview rule for an extension, and back.
pub fn pattern_of_ext(ext: &str) -> String {
    format!("(?i)\\.{}$", regex_escape(ext))
}

pub fn ext_of_pattern(p: &str) -> Option<String> {
    p.strip_prefix("(?i)\\.").and_then(|r| r.strip_suffix('$')).map(regex_unescape)
}

pub fn regex_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if !c.is_ascii_alphanumeric() {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn regex_unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The window a preview of `file` lives in.
pub fn preview_name(file: &str) -> String {
    format!("{file}+Preview")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AttachmentId;

    #[test]
    fn converters_come_from_settings_then_defaults() {
        let mut meta = Meta::default();
        assert_eq!(converter(&meta, "md").as_deref(), Some("apex md"));
        assert_eq!(converter(&meta, "MD").as_deref(), Some("apex md"));
        assert_eq!(converter(&meta, "rst"), None);
        meta.settings.entry(AttachmentId(7)).or_default().insert("Preview.rst".into(), "pandoc -f rst -t html5".into());
        assert_eq!(converter(&meta, "rst").as_deref(), Some("pandoc -f rst -t html5"));
        meta.settings.entry(SERVER).or_default().insert("Preview.md".into(), "".into());
        assert_eq!(converter(&meta, "md"), None);
        let e: Vec<String> = exts(&meta).into_iter().collect();
        assert_eq!(e, vec!["htm", "html", "markdown", "rst", "svg"]);
        assert_eq!(ext_of_pattern(&pattern_of_ext("c++")).as_deref(), Some("c++"));
        assert_eq!(pattern_of_ext("md"), r"(?i)\.md$");
        assert_eq!(preview_name("/a/b.md"), "/a/b.md+Preview");
    }
}
