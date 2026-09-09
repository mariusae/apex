//! Plumbing rules: what they match, and how their templates expand.
//! The table itself lives in the metalog (`Meta::rules`); the server
//! walks it in priority order for a plumb or a verb, and the leader reads
//! it for the verbs a window's tag shows.

use crate::ids::WindowId;
use std::collections::BTreeMap;

use crate::entry::{PlumbRule, WinKind};
use crate::ids::{AttachmentId, RuleId};
use crate::state::Rule;

/// What a rule's templates can name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bindings {
    /// `$0`..`$9`: the text match and its groups.
    pub groups: Vec<String>,
    pub file: String,
    pub dir: String,
    pub win: String,
    pub line: String,
    pub sel: String,
}

/// Expand `$0`..`$9`, `$file`, `$dir`, `$win`, `$line`, `$sel`; `$$` is
/// a dollar. Anything else after `$` is left as it is.
pub fn expand(template: &str, b: &Bindings) -> String {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('$') => {
                chars.next();
                out.push('$');
            }
            Some(d) if d.is_ascii_digit() => {
                chars.next();
                let i = d as usize - '0' as usize;
                out.push_str(b.groups.get(i).map(String::as_str).unwrap_or(""));
            }
            Some(a) if a.is_ascii_alphabetic() => {
                let mut name = String::new();
                while let Some(&n) = chars.peek() {
                    if n.is_ascii_alphanumeric() || n == '_' {
                        name.push(n);
                        chars.next();
                    } else {
                        break;
                    }
                }
                match name.as_str() {
                    "file" => out.push_str(&b.file),
                    "dir" => out.push_str(&b.dir),
                    "win" => out.push_str(&b.win),
                    "line" => out.push_str(&b.line),
                    "sel" => out.push_str(&b.sel),
                    _ => {
                        out.push('$');
                        out.push_str(&name);
                    }
                }
            }
            _ => out.push('$'),
        }
    }
    out
}

/// A compiled regexp, anchored to the whole text as plan 9's `matches`
/// is; a bad pattern matches nothing.
fn whole(re: &str) -> Option<regex::Regex> {
    regex::Regex::new(&format!("^(?:{re})$")).ok()
}

impl PlumbRule {
    /// Does this rule apply to a window of this name and kind (the parts
    /// of the predicate that do not need the text)?
    pub fn applies_to(&self, name: &str, kind: WinKind, w: Option<WindowId>) -> bool {
        if let Some(id) = self.win {
            if w != Some(id) {
                return false;
            }
        }
        if let Some(k) = self.kind {
            if k != kind {
                return false;
            }
        }
        if let Some(f) = &self.file {
            match regex::Regex::new(f) {
                Ok(re) => {
                    if !re.is_match(name) {
                        return false;
                    }
                }
                Err(_) => return false,
            }
        }
        true
    }

    /// Match the text: the groups it binds, or `None`. A rule with no
    /// text predicate matches any text and binds it as `$0`.
    pub fn match_text(&self, text: &str) -> Option<Vec<String>> {
        match &self.text {
            None => Some(vec![text.to_string()]),
            Some(re) => {
                let re = whole(re)?;
                let caps = re.captures(text)?;
                Some((0..caps.len().min(10)).map(|i| caps.get(i).map(|m| m.as_str().to_string()).unwrap_or_default()).collect())
            }
        }
    }

    /// The pattern is a valid regexp (checked when a rule is added).
    pub fn check(&self) -> Result<(), String> {
        for (what, re) in [("--text", &self.text), ("--file", &self.file)] {
            if let Some(re) = re {
                regex::Regex::new(re).map_err(|e| format!("{what} {re:?}: {e}"))?;
            }
        }
        if self.verb.is_empty() || self.verb.contains(char::is_whitespace) {
            return Err(format!("bad verb {:?}", self.verb));
        }
        Ok(())
    }
}

/// A shell word: quoted when it needs to be.
fn word(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=+@%".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

impl PlumbRule {
    /// The rule as the `apex plumb rule add` flags that make it.
    pub fn to_flags(&self) -> String {
        let mut out = Vec::new();
        if self.verb != "plumb" {
            out.push(format!("-verb={}", word(&self.verb)));
        }
        if let Some(t) = &self.text {
            out.push(format!("-text={}", word(t)));
        }
        if let Some(f) = &self.file {
            out.push(format!("-file={}", word(f)));
        }
        if let Some(k) = self.kind {
            out.push(format!("-kind={}", k.name()));
        }
        if let Some(w) = self.win {
            out.push(format!("-win={}", w.0));
        }
        if let Some(t) = &self.isfile {
            out.push(format!("-isfile={}", word(t)));
        }
        if let Some(t) = &self.isdir {
            out.push(format!("-isdir={}", word(t)));
        }
        match &self.action {
            crate::entry::RuleAction::Edit(t) => out.push(format!("-edit={}", word(t))),
            crate::entry::RuleAction::Run(t) => out.push(format!("-run={}", word(t))),
            crate::entry::RuleAction::Client { verb, args } => out.push(format!("-client={} -args={}", word(verb), word(args))),
            crate::entry::RuleAction::Tool(t) => out.push(format!("-tool={}", word(t))),
        }
        if let Some(to) = self.to {
            out.push(format!("-to={}", match to {
                crate::entry::RunTo::Errors => "errors",
                crate::entry::RunTo::Window => "window",
            }));
        }
        out.join(" ")
    }
}

/// The rules in the order the server tries them: priority high to low,
/// then by id (the order they were installed).
pub fn ordered(rules: &BTreeMap<RuleId, Rule>) -> Vec<(RuleId, &Rule)> {
    let mut v: Vec<(RuleId, &Rule)> = rules.iter().map(|(id, r)| (*id, r)).collect();
    v.sort_by(|a, b| b.1.priority.cmp(&a.1.priority).then(a.0.cmp(&b.0)));
    v
}

/// The verbs a window shows in its tag: every rule that applies to it
/// and answers something other than `plumb`, once each, in order.
/// The verb that takes every B2 command in a window that nothing else
/// took (no builtin, no verb rule): win's, so that B2 on an old command
/// line types it to the shell. Not a word in the menu.
pub const EXEC: &str = "exec";

pub fn offers_verb(rules: &BTreeMap<RuleId, Rule>, verb: &str, name: &str, kind: WinKind, w: Option<WindowId>) -> bool {
    rules
        .values()
        .any(|r| r.rule.verb == verb && r.rule.applies_to(name, kind, w))
}

pub fn verbs_for(rules: &BTreeMap<RuleId, Rule>, name: &str, kind: WinKind, w: Option<WindowId>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (_, r) in ordered(rules) {
        if r.rule.verb != "plumb" && r.rule.verb != EXEC && r.rule.applies_to(name, kind, w) && !out.contains(&r.rule.verb) {
            out.push(r.rule.verb.clone());
        }
    }
    out
}

/// The rules an attachment installed (to remove when it goes).
pub fn owned_by(rules: &BTreeMap<RuleId, Rule>, a: AttachmentId) -> Vec<RuleId> {
    rules.iter().filter(|(_, r)| r.attachment == a).map(|(id, _)| *id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::RuleAction;

    fn rule(text: Option<&str>, file: Option<&str>) -> PlumbRule {
        PlumbRule { verb: "plumb".into(), text: text.map(String::from), file: file.map(String::from), kind: None, isfile: None, isdir: None, action: RuleAction::Edit("$0".into()), win: None, to: None }
    }

    #[test]
    fn templates_expand() {
        let b = Bindings { groups: vec!["a.go:12".into(), "a.go".into(), "12".into()], file: "/x/y.md".into(), dir: "/x".into(), win: "3".into(), line: "".into(), sel: "s".into() };
        assert_eq!(expand("$1:$2 in $dir ($file) $$ $sel $win $nope", &b), "a.go:12 in /x (/x/y.md) $ s 3 $nope");
    }

    #[test]
    fn text_matches_whole_and_binds_groups() {
        let r = rule(Some(r"([^:\s]+):(\d+)"), None);
        assert_eq!(r.match_text("main.go:12").unwrap(), vec!["main.go:12", "main.go", "12"]);
        assert!(r.match_text("main.go:12 extra").is_none());
        assert_eq!(rule(None, None).match_text("anything").unwrap(), vec!["anything"]);
        assert!(rule(Some("("), None).match_text("x").is_none());
    }

    #[test]
    fn windows_are_matched_by_name_and_kind() {
        let mut r = rule(None, Some(r"\.md$"));
        assert!(r.applies_to("/a/notes.md", WinKind::File, None));
        assert!(!r.applies_to("/a/notes.txt", WinKind::File, None));
        r.kind = Some(WinKind::Dir);
        assert!(!r.applies_to("/a/notes.md", WinKind::File, None));
    }
}
