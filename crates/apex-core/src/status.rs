//! The Program Status Protocol (OSC 7501, revision 0.3): a program in a
//! terminal saying what it is doing -- at rest, working (and how far
//! along), finished, waiting on the user (and for what), failed -- as
//! records, one per `id` (none: the root), each replaced whole by the
//! next report naming it. What is shown is the terminal's to decide; this
//! is the protocol's reading, its limits, and the rules of a record's
//! life (`Records`).
//!
//! `OSC 7501 ; state=working:app=brew:msg=<base64> ST`

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// What a record says of its program.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum State {
    /// At rest, the user's next instruction awaited.
    Idle,
    /// Running, perhaps with a `progress`.
    Working,
    /// Finished, the result not yet seen.
    Done,
    /// Cannot go on without the user (`kind` says why).
    Blocked,
    /// Failed, and stopped.
    Error,
}

impl State {
    /// As the protocol writes it.
    pub fn name(self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Working => "working",
            State::Done => "done",
            State::Blocked => "blocked",
            State::Error => "error",
        }
    }

    pub fn from_name(s: &str) -> Option<State> {
        [State::Idle, State::Working, State::Done, State::Blocked, State::Error].into_iter().find(|x| x.name() == s)
    }
}

/// What a blocked program waits on.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Kind {
    Permission,
    Question,
    Auth,
}

impl Kind {
    /// As the protocol writes it.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Permission => "permission",
            Kind::Question => "question",
            Kind::Auth => "auth",
        }
    }

    pub fn from_name(s: &str) -> Option<Kind> {
        [Kind::Permission, Kind::Question, Kind::Auth].into_iter().find(|x| x.name() == s)
    }
}

/// One record: what the last report naming its id said.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Record {
    /// `build/test`; empty for the root.
    pub id: String,
    pub state: State,
    pub kind: Option<Kind>,
    /// 0 to 100; none is not known.
    pub progress: Option<u8>,
    /// The program's name, as it gives it (`app_of` inherits it).
    pub app: Option<String>,
    pub title: Option<String>,
    pub msg: Option<String>,
}

/// A report read: a record set, or records cleared (an id and all under
/// it; none, every one).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Report {
    Set(Record),
    Clear(Option<String>),
}

/// The sequence's body, after `7501;`, when it is the query (`?`).
pub fn is_query(body: &[u8]) -> bool {
    body.iter().filter(|c| !c.is_ascii_whitespace()).eq(b"?".iter())
}

// the limits (the protocol's own; a report past one is discarded whole)
const MAX_BODY: usize = 4096 - 8;
const MAX_KEY: usize = 16;
const MAX_MSG: (usize, usize) = (2732, 2048);
const MAX_TITLE: (usize, usize) = (256, 192);
const MAX_APP: usize = 32;
const MAX_ID: usize = 128;
const MAX_SEGMENT: usize = 32;
const MAX_DEPTH: usize = 8;
/// Records a terminal keeps; past it, the least lately reported goes.
pub const MAX_RECORDS: usize = 256;

/// Read a report's body (after `7501;`): None when it is to be discarded
/// (no state, a bad one, a bad id, a limit passed, bad base64, control
/// characters in what it decodes to).
pub fn parse(body: &[u8]) -> Option<Report> {
    if body.len() > MAX_BODY {
        return None;
    }
    let body = std::str::from_utf8(body).ok()?;
    let mut pairs: BTreeMap<&str, &str> = BTreeMap::new();
    for pair in body.split(':') {
        let Some((k, v)) = pair.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        if k.is_empty() || !k.bytes().all(|c| c.is_ascii_lowercase()) || !v.bytes().all(value_char) {
            continue; // malformed: skipped, the rest still read
        }
        if k.len() > MAX_KEY {
            return None;
        }
        pairs.insert(k, v); // the last of a key wins
    }
    let id = match pairs.get("id") {
        Some(id) => Some(check_id(id)?),
        None => None,
    };
    let state = match *pairs.get("state")? {
        "idle" => State::Idle,
        "working" => State::Working,
        "done" => State::Done,
        "blocked" => State::Blocked,
        "error" => State::Error,
        "clear" => return Some(Report::Clear(id)),
        _ => return None,
    };
    let kind = match (state, pairs.get("kind").copied()) {
        (State::Blocked, Some("permission")) => Some(Kind::Permission),
        (State::Blocked, Some("question")) => Some(Kind::Question),
        (State::Blocked, Some("auth")) => Some(Kind::Auth),
        _ => None,
    };
    let progress = match state {
        State::Working | State::Blocked => pairs.get("progress").and_then(|p| p.parse::<u8>().ok()).filter(|p| *p <= 100 && pairs["progress"].bytes().all(|c| c.is_ascii_digit())),
        _ => None,
    };
    let app = pairs.get("app").filter(|a| a.len() <= MAX_APP && !a.is_empty() && a.bytes().all(id_char)).map(|a| a.to_string());
    let text = |key: &str, (enc, dec): (usize, usize)| -> Option<Option<String>> {
        let Some(v) = pairs.get(key) else { return Some(None) };
        if v.len() > enc {
            return None;
        }
        let t = String::from_utf8(base64(v)?).ok()?;
        if t.len() > dec || t.chars().any(|c| c.is_control()) {
            return None;
        }
        Some(Some(t))
    };
    let title = text("title", MAX_TITLE)?;
    let msg = text("msg", MAX_MSG)?;
    Some(Report::Set(Record { id: id.unwrap_or_default(), state, kind, progress, app, title, msg }))
}

/// A report's body (what follows `7501;`) as a program writes it: the
/// inverse of `parse`. `None` for the records cleared (an id and all
/// under it; none, every one).
pub fn encode(r: Option<&Record>, clear: Option<&str>) -> String {
    let Some(r) = r else {
        return match clear {
            Some(id) => format!("state=clear:id={id}"),
            None => "state=clear".to_string(),
        };
    };
    let mut out = format!("state={}", r.state.name());
    if !r.id.is_empty() {
        out += &format!(":id={}", r.id);
    }
    if let Some(k) = r.kind {
        out += &format!(":kind={}", k.name());
    }
    if let Some(p) = r.progress {
        out += &format!(":progress={p}");
    }
    if let Some(a) = &r.app {
        out += &format!(":app={a}");
    }
    for (key, v) in [("title", &r.title), ("msg", &r.msg)] {
        if let Some(v) = v {
            out += &format!(":{key}={}", base64_encode(v.as_bytes()));
        }
    }
    out
}

fn base64_encode(b: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            out.push(if i <= c.len() { A[(n >> (18 - 6 * i)) as usize & 63] as char } else { '=' });
        }
    }
    out
}

fn value_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b"_.,+/=-".contains(&c)
}

fn id_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b"_.+-".contains(&c)
}

fn check_id(id: &str) -> Option<String> {
    let segments: Vec<&str> = id.split('/').collect();
    let ok = id.len() <= MAX_ID && segments.len() <= MAX_DEPTH && segments.iter().all(|s| !s.is_empty() && s.len() <= MAX_SEGMENT && s.bytes().all(id_char));
    ok.then(|| id.to_string())
}

/// Standard base64, padding or not; None for anything else.
fn base64(s: &str) -> Option<Vec<u8>> {
    let s = s.trim_end_matches('=');
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    // left over: fewer than a byte's bits, all of them zero
    (bits < 6 && acc == 0).then_some(out)
}

/// `id` is `of`, or under it.
fn within(id: &str, of: &str) -> bool {
    of.is_empty() || id == of || id.strip_prefix(of).is_some_and(|r| r.starts_with('/'))
}

/// A terminal's records, in the order they were last reported.
#[derive(Clone, Default, Debug)]
pub struct Records {
    list: Vec<Record>,
}

impl Records {
    /// A report applied; whether anything changed.
    pub fn apply(&mut self, report: Report) -> bool {
        match report {
            Report::Clear(None) => !std::mem::take(&mut self.list).is_empty(),
            Report::Clear(Some(of)) => {
                let before = self.list.len();
                self.list.retain(|r| !within(&r.id, &of));
                self.list.len() != before
            }
            Report::Set(record) => {
                let at = self.list.iter().position(|r| r.id == record.id);
                let same = at.is_some_and(|i| self.list[i] == record);
                if let Some(i) = at {
                    self.list.remove(i);
                }
                self.list.push(record);
                if self.list.len() > MAX_RECORDS {
                    self.list.remove(0);
                }
                !same
            }
        }
    }

    /// The program ended, or the shell prompts again: working and blocked
    /// records go (done, error and idle stay). Whether any went.
    pub fn settle(&mut self) -> bool {
        let before = self.list.len();
        self.list.retain(|r| !matches!(r.state, State::Working | State::Blocked));
        self.list.len() != before
    }

    /// A full reset (RIS): every record gone. Whether there were any.
    pub fn reset(&mut self) -> bool {
        !std::mem::take(&mut self.list).is_empty()
    }

    /// The records, by id.
    pub fn sorted(&self) -> Vec<Record> {
        let mut v = self.list.clone();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

/// A record's program: its own `app`, else its nearest ancestor's.
pub fn app_of<'a>(records: &'a [Record], id: &str) -> Option<&'a str> {
    let mut at = id;
    loop {
        if let Some(app) = records.iter().find(|r| r.id == at).and_then(|r| r.app.as_deref()) {
            return Some(app);
        }
        if at.is_empty() {
            return None;
        }
        at = at.rsplit_once('/').map_or("", |(parent, _)| parent);
    }
}

/// What the records come to, for a window's marks: the most pressing
/// record's state (blocked, then error, then done, then working, then
/// idle), and the progress of a working one (the root's first).
pub fn summary(records: &[Record]) -> Option<(State, Option<u8>)> {
    let top = top(records)?;
    let working = records.iter().filter(|r| r.state == State::Working).max_by_key(|r| r.id.is_empty());
    Some((top.state, working.and_then(|r| r.progress)))
}

/// The most pressing record: blocked, then error, then done, then
/// working, then idle; of those alike, the root.
pub fn top(records: &[Record]) -> Option<&Record> {
    let rank = |s: State| match s {
        State::Blocked => 4,
        State::Error => 3,
        State::Done => 2,
        State::Working => 1,
        State::Idle => 0,
    };
    records.iter().max_by_key(|r| (rank(r.state), r.id.is_empty()))
}

/// A state said in a word, where a status has no message of its own
/// (none for idle: at rest, nothing to say).
pub fn word(state: State, kind: Option<Kind>) -> Option<&'static str> {
    Some(match (state, kind) {
        (State::Idle, _) => return None,
        (State::Working, _) => "Working",
        (State::Done, _) => "Done",
        (State::Error, _) => "Failed",
        (State::Blocked, Some(Kind::Permission)) => "Needs permission",
        (State::Blocked, Some(Kind::Question)) => "Has a question",
        (State::Blocked, Some(Kind::Auth)) => "Needs signing in",
        (State::Blocked, None) => "Blocked",
    })
}

/// What a record says beside a window's label: its message's first
/// line, else its title's.
pub fn said(r: &Record) -> Option<String> {
    [&r.msg, &r.title].into_iter().flatten().map(|s| s.lines().next().unwrap_or("").trim()).find(|s| !s.is_empty()).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(s: &str) -> String {
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for c in s.as_bytes().chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for i in 0..=c.len() {
                out.push(A[(n >> (18 - 6 * i)) as usize & 63] as char);
            }
        }
        out
    }

    fn set(body: &str) -> Record {
        match parse(body.as_bytes()) {
            Some(Report::Set(r)) => r,
            other => panic!("{body}: {other:?}"),
        }
    }

    #[test]
    fn a_report_written_is_read_back_as_it_was() {
        let r = Record { id: "build/test".into(), state: State::Blocked, kind: Some(Kind::Permission), progress: Some(40), app: Some("make".into()), title: Some("Tests".into()), msg: Some("Allow ünïcode? y/n".into()) };
        assert_eq!(parse(encode(Some(&r), None).as_bytes()), Some(Report::Set(r)));
        let idle = Record { id: String::new(), state: State::Idle, kind: None, progress: None, app: None, title: None, msg: Some("ab".into()) };
        assert_eq!(parse(encode(Some(&idle), None).as_bytes()), Some(Report::Set(idle)));
        assert_eq!(parse(encode(None, Some("build")).as_bytes()), Some(Report::Clear(Some("build".into()))));
        assert_eq!(parse(encode(None, None).as_bytes()), Some(Report::Clear(None)));
    }

    #[test]
    fn the_most_pressing_record_and_what_it_says() {
        let rec = |id: &str, state: State, msg: Option<&str>| Record { id: id.into(), state, kind: None, progress: None, app: None, title: None, msg: msg.map(str::to_string) };
        assert_eq!(top(&[]), None);
        let blocked = Record { kind: Some(Kind::Question), ..rec("build", State::Blocked, None) };
        let all = [rec("", State::Working, Some("Building")), blocked.clone()];
        assert_eq!(top(&all), Some(&blocked), "blocked before working");
        assert_eq!(said(&rec("", State::Working, Some("Reading files\nand more"))).as_deref(), Some("Reading files"));
        assert_eq!(said(&Record { title: Some("Tests".into()), ..rec("", State::Done, Some(" ")) }).as_deref(), Some("Tests"), "no message: the title");
        assert_eq!((word(State::Idle, None), word(State::Blocked, Some(Kind::Question))), (None, Some("Has a question")));
    }

    #[test]
    fn reports_are_read_as_the_protocol_says() {
        let r = set(&format!("state=working:app=brew:progress=40:msg={}", b64("Installing updates")));
        assert_eq!((r.id.as_str(), r.state, r.progress, r.app.as_deref(), r.msg.as_deref()), ("", State::Working, Some(40), Some("brew"), Some("Installing updates")));
        // padding or not; whitespace round keys and values; the last of a
        // key; a malformed pair skipped, the rest read
        let r = set(&format!(" state = blocked :kind=auth:kind=question:bogus:x=a;b:msg={}==", b64("why")));
        assert_eq!((r.state, r.kind, r.msg.as_deref()), (State::Blocked, Some(Kind::Question), Some("why")));
        // kind only when blocked; progress only working or blocked, 0..=100
        let r = set("state=done:kind=auth:progress=50");
        assert_eq!((r.kind, r.progress), (None, None));
        assert_eq!(set("state=working:progress=101").progress, None);
        assert_eq!(set("state=working:progress=+5").progress, None);
        // an unknown key, ignored; a bad app, as if none
        let r = set("state=idle:color=red:app=a/b");
        assert_eq!(r.app, None);
        // a pair with a byte past the value set is malformed: skipped
        assert_eq!(set("state=working:msg=***").msg, None);
        assert_eq!(set("state=idle:id=build/test").id, "build/test");
        assert_eq!(parse(b"state=clear:id=build"), Some(Report::Clear(Some("build".into()))));
        assert_eq!(parse(b"state=clear"), Some(Report::Clear(None)));
        assert!(is_query(b"?") && is_query(b" ? ") && !is_query(b"state=idle"));
    }

    #[test]
    fn a_bad_report_is_discarded_whole() {
        for body in [
            "app=x".to_string(),
            "state=busy".into(),
            "state=working:id=a//b".into(),
            format!("state=working:id={}", "a/".repeat(8) + "a"),
            format!("state=working:id={}", "x".repeat(33)),
            "state=working:msg=Y".into(),
            format!("state=working:msg={}", b64("bell\u{7}")),
            format!("state=working:title={}", b64(&"x".repeat(193))),
            format!("state=working:{}=1", "k".repeat(17)),
        ] {
            assert_eq!(parse(body.as_bytes()), None, "{body}");
        }
    }

    #[test]
    fn records_live_and_go_as_the_protocol_says() {
        let mut rs = Records::default();
        assert!(rs.apply(parse(b"state=working:app=deploy").unwrap()));
        assert!(!rs.apply(parse(b"state=working:app=deploy").unwrap()), "the same again: no change");
        rs.apply(parse(b"state=blocked:id=us-east:kind=permission").unwrap());
        rs.apply(parse(b"state=done:id=eu-west").unwrap());
        rs.apply(parse(b"state=working:id=eu-west/db").unwrap());
        let v = rs.sorted();
        assert_eq!(app_of(&v, "us-east"), Some("deploy"), "inherited from the root");
        assert_eq!(summary(&v), Some((State::Blocked, None)));
        // a clear takes the id and all under it
        assert!(rs.apply(parse(b"state=clear:id=eu-west").unwrap()));
        assert_eq!(rs.sorted().iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), vec!["", "us-east"]);
        // replacement is whole: a key left out is gone
        rs.apply(parse(b"state=working").unwrap());
        assert_eq!(rs.sorted()[0].app, None);
        // the program ends, or the prompt is back: working and blocked go,
        // done and error stay
        rs.apply(parse(b"state=error:id=x").unwrap());
        assert!(rs.settle());
        assert_eq!(rs.sorted().iter().map(|r| (r.id.as_str(), r.state)).collect::<Vec<_>>(), vec![("x", State::Error)]);
        assert!(rs.reset() && rs.is_empty());
        // past the cap, the least lately reported goes
        for i in 0..=MAX_RECORDS {
            rs.apply(parse(format!("state=idle:id=r{i}").as_bytes()).unwrap());
        }
        assert_eq!(rs.sorted().len(), MAX_RECORDS);
        assert!(!rs.sorted().iter().any(|r| r.id == "r0"));
    }
}
