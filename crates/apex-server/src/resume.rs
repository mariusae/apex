//! Program resume (OSC 7577, revision 0.5): a program in a terminal says
//! the command that brings it back, and the terminal, when it starts its
//! sessions again after the daemon stopped, runs it there.
//! <https://www.superlogical.com/rex/docs/build/program-resume>
//!
//! The wire: `OSC 7577 ; body ST`, the body `key=value` pairs joined by
//! `:`, ending `:mac=` and the SHA-256 (hex) of the body before it, a `:`
//! and the terminal's token for that terminal -- `OSC7577_TOKEN`, which
//! the daemon puts in the environment of each terminal's first process
//! (`token`), so only a program started there can report. `pid` and
//! `time` always; `argv` (base64 elements, joined by `,`) and maybe `cwd`
//! (base64), or `clear=1`; `need` names keys a terminal must know.
//!
//! This is the protocol alone: what a report says, and whether it is to
//! be believed (`parse`). What the daemon keeps of it and when it runs
//! it are the daemon's (`Server::resume_report`, `Server::restored_term`).

use std::time::{SystemTime, UNIX_EPOCH};

/// The environment variable the token is in.
pub const TOKEN_ENV: &str = "OSC7577_TOKEN";

/// The OSC number.
pub const OSC: &str = "7577";

/// The most bytes of a report kept (the spec's least is 2048 for the
/// whole sequence); a longer one is not read whole and is ignored.
pub const MAX_BYTES: usize = 4096;

/// The most elements of `argv` (the spec's least is 64).
pub const MAX_ARGV: usize = 64;

/// How far a report's `time` may be from the terminal's clock.
pub const SKEW_SECS: u64 = 60;

/// What a report asks: a command to resume the program by, or that it
/// is not to be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ask {
    Resume { argv: Vec<String>, cwd: Option<String> },
    Clear,
}

/// A report believed: whose it is (`pid`), when it was made, and what it
/// asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub pid: u32,
    pub time: u64,
    pub ask: Ask,
}

/// Why a report is not taken. `Forged` and `Stale` change nothing; any
/// other, the report being genuine but not acceptable, clears what the
/// terminal kept for its owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reject {
    /// Not a report in the grammar, or no `mac` that matches.
    Forged,
    /// Its time too far from now.
    Stale,
    /// Genuine, but asking what cannot be done: a bad `argv` or `cwd`, a
    /// key it needs that is not known here, too long.
    Unacceptable(String),
}

/// A new token for a terminal: 32 characters of `[A-Za-z0-9_-]`, from
/// the system's random source.
pub fn token() -> String {
    use std::io::Read;
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-";
    let mut bytes = [0u8; 32];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes)).is_err() {
        // no random source: a token nothing can guess is not to be had;
        // one from the clock and pid at least differs per terminal, and
        // reports with it are no worse than none
        let seed = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0) ^ std::process::id() as u128;
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = (seed >> ((i % 16) * 8)) as u8 ^ i as u8;
        }
    }
    bytes.iter().map(|b| ALPHABET[(*b & 63) as usize] as char).collect()
}

/// The `mac` of `body` (the report before `:mac=`) under `token`.
pub fn mac(body: &str, token: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(body.as_bytes());
    h.update(b":");
    h.update(token.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// A report as a program writes it: the body, its `mac` under `token`
/// appended (for programs -- the hooks -- and tests).
pub fn encode(pid: u32, time: u64, ask: &Ask, token: &str) -> String {
    let body = match ask {
        Ask::Clear => format!("pid={pid}:time={time}:clear=1"),
        Ask::Resume { argv, cwd } => {
            let argv = argv.iter().map(|a| b64_encode(a.as_bytes())).collect::<Vec<_>>().join(",");
            let cwd = cwd.as_ref().map(|c| format!(":cwd={}", b64_encode(c.as_bytes()))).unwrap_or_default();
            format!("pid={pid}:time={time}:argv={argv}{cwd}")
        }
    };
    let m = mac(&body, token);
    format!("{body}:mac={m}")
}

/// The whole sequence for a report.
pub fn sequence(pid: u32, time: u64, ask: &Ask, token: &str) -> String {
    format!("\x1b]{OSC};{}\x1b\\", encode(pid, time, ask, token))
}

/// Read the body of an OSC 7577 (after `7577;`) from a terminal whose
/// token is `token`, at `now` (seconds since the epoch): the report, if
/// it is genuine, timely and acceptable.
pub fn parse(body: &str, token: &str, now: u64) -> Result<Report, Reject> {
    // `?` is reserved, and anything not a report is not one
    if body == "?" || body.len() > MAX_BYTES {
        return Err(Reject::Forged);
    }
    let (signed, given) = body.rsplit_once(":mac=").ok_or(Reject::Forged)?;
    if given.len() != 64 || !given.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(Reject::Forged);
    }
    if !constant_eq(mac(signed, token).as_bytes(), given.as_bytes()) {
        return Err(Reject::Forged);
    }
    let mut pid = None;
    let mut time = None;
    let mut argv = None;
    let mut cwd = None;
    let mut clear = false;
    let mut need: Vec<String> = Vec::new();
    for pair in signed.split(':') {
        let (k, v) = pair.split_once('=').ok_or(Reject::Forged)?;
        if k.is_empty() || !k.bytes().all(|b| b.is_ascii_lowercase()) || !v.bytes().all(value_byte) {
            return Err(Reject::Forged);
        }
        match k {
            "pid" => pid = Some(positive(v).ok_or(Reject::Forged)?),
            "time" => time = Some(positive(v).ok_or(Reject::Forged)?),
            "argv" => argv = Some(v.to_string()),
            "cwd" => cwd = Some(v.to_string()),
            "clear" => clear = v == "1",
            "need" => need = v.split(',').filter(|n| !n.is_empty()).map(String::from).collect(),
            _ => {} // unknown keys are ignored, unless needed
        }
    }
    let (Some(pid), Some(time)) = (pid, time) else { return Err(Reject::Forged) };
    let pid = u32::try_from(pid).map_err(|_| Reject::Forged)?;
    if now.abs_diff(time) > SKEW_SECS {
        return Err(Reject::Stale);
    }
    let unacceptable = |why: &str| Err(Reject::Unacceptable(why.to_string()));
    if let Some(n) = need.iter().find(|n| !["pid", "time", "argv", "cwd", "clear", "need", "mac"].contains(&n.as_str())) {
        return unacceptable(&format!("needs `{n}`, which is not known here"));
    }
    let ask = match (argv, clear) {
        (Some(_), true) | (None, false) => return unacceptable("one of argv and clear"),
        (None, true) => Ask::Clear,
        (Some(a), false) => {
            if a.is_empty() {
                return unacceptable("an empty argv");
            }
            let parts: Vec<&str> = a.split(',').collect();
            if parts.len() > MAX_ARGV {
                return unacceptable("too many arguments");
            }
            let Some(argv) = parts.iter().map(|p| text(p)).collect::<Option<Vec<String>>>() else {
                return unacceptable("an argument not base64 UTF-8 text");
            };
            let prog = &argv[0];
            if prog.is_empty() || prog.starts_with('-') || (prog.contains('/') && !prog.starts_with('/')) {
                return unacceptable("the program neither a bare name nor an absolute path");
            }
            let cwd = match cwd {
                None => None,
                Some(c) => match text(&c) {
                    Some(d) if d.starts_with('/') => Some(d),
                    _ => return unacceptable("cwd not an absolute path"),
                },
            };
            Ask::Resume { argv, cwd }
        }
    };
    Ok(Report { pid, time, ask })
}

/// A `value` byte: `[A-Za-z0-9_.,+/=-]`.
fn value_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"_.,+/=-".contains(&b)
}

/// `[1-9][0-9]*`.
fn positive(v: &str) -> Option<u64> {
    (!v.is_empty() && !v.starts_with('0') && v.bytes().all(|b| b.is_ascii_digit())).then(|| v.parse().ok()).flatten()
}

/// A base64 element as text: valid UTF-8, no control characters.
fn text(b64: &str) -> Option<String> {
    let s = String::from_utf8(b64_decode(b64)?).ok()?;
    (!s.chars().any(char::is_control)).then_some(s)
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64, padded.
pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Standard base64, padding optional; None for anything else.
pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim_end_matches('=');
    if s.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        let v = B64.iter().position(|&x| x == c)? as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// What the daemon keeps of a report it took, for its terminal: whose
/// it is (the process, and when it started, so a pid used again is not
/// taken for it -- none while a resumed program has yet to report), when
/// it was made, and the command and directory to resume by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub pid: Option<u32>,
    pub started: Option<u64>,
    pub time: u64,
    pub argv: Vec<String>,
    pub cwd: Option<String>,
}

impl Record {
    /// Whether its process has gone: exited, or its pid another's now.
    pub fn gone(&self) -> bool {
        match (self.pid, self.started) {
            (Some(pid), Some(started)) => proc_info(pid).is_none_or(|p| p.started != started),
            _ => false,
        }
    }
}

/// The programs resumed unless the session says otherwise
/// (`Resume.programs`).
pub const DEFAULT_PROGRAMS: &str = "claude codex";

/// Whether a command may be resumed, by `programs` (the session's
/// `Resume.programs`: names, space-separated, or `off`): its program's
/// name -- a path's last part -- one of them.
pub fn allowed(programs: &str, argv0: &str) -> bool {
    let name = argv0.rsplit('/').next().unwrap_or(argv0);
    let list: Vec<&str> = programs.split_whitespace().collect();
    !list.contains(&"off") && list.contains(&name)
}

/// What the system says of a running process: its owner, and when it
/// started (seconds, or on Linux clock ticks since boot: only ever
/// compared with itself).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Proc {
    pub uid: u32,
    pub started: u64,
}

#[cfg(target_os = "macos")]
pub fn proc_info(pid: u32) -> Option<Proc> {
    // SAFETY: proc_pidinfo fills at most `size` bytes of `info`
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let n = unsafe { libc::proc_pidinfo(pid as i32, libc::PROC_PIDTBSDINFO, 0, &mut info as *mut _ as *mut libc::c_void, size) };
    (n == size).then_some(Proc { uid: info.pbi_uid, started: info.pbi_start_tvsec })
}

#[cfg(target_os = "linux")]
pub fn proc_info(pid: u32) -> Option<Proc> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // after the command's closing parenthesis: state is field 3, start
    // time field 22
    let rest = stat.rsplit_once(')')?.1;
    let started = rest.split_whitespace().nth(19)?.parse().ok()?;
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let uid = status.lines().find_map(|l| l.strip_prefix("Uid:"))?.split_whitespace().next()?.parse().ok()?;
    Some(Proc { uid, started })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn proc_info(_pid: u32) -> Option<Proc> {
    None
}

/// A process's current directory.
#[cfg(target_os = "macos")]
pub fn proc_cwd(pid: u32) -> Option<String> {
    // SAFETY: as `proc_info`; the path is a NUL-terminated C string in
    // the buffer
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as i32;
    let n = unsafe { libc::proc_pidinfo(pid as i32, libc::PROC_PIDVNODEPATHINFO, 0, &mut info as *mut _ as *mut libc::c_void, size) };
    if n != size {
        return None;
    }
    let path = unsafe { std::ffi::CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr() as *const libc::c_char) };
    Some(path.to_string_lossy().into_owned()).filter(|p| p.starts_with('/'))
}

#[cfg(target_os = "linux")]
pub fn proc_cwd(pid: u32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok().map(|p| p.display().to_string())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn proc_cwd(_pid: u32) -> Option<String> {
    None
}

/// A process's arguments as it was started, exactly (no shell's words
/// split again).
#[cfg(target_os = "macos")]
pub fn proc_argv(pid: u32) -> Option<Vec<String>> {
    // KERN_PROCARGS2: argc, the executable's path, NUL padding, then the
    // arguments, each NUL-terminated (then the environment)
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    let mut size: libc::size_t = 0;
    // SAFETY: the first call sizes the buffer, the second fills it
    if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, std::ptr::null_mut(), &mut size, std::ptr::null_mut(), 0) } != 0 || size < 4 {
        return None;
    }
    let mut buf = vec![0u8; size];
    if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr().cast(), &mut size, std::ptr::null_mut(), 0) } != 0 {
        return None;
    }
    buf.truncate(size);
    let argc = i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?) as usize;
    let mut rest = &buf[4..];
    // the executable's path, then the padding
    let end = rest.iter().position(|&b| b == 0)?;
    rest = &rest[end..];
    let start = rest.iter().position(|&b| b != 0)?;
    rest = &rest[start..];
    let args: Vec<String> = rest.split(|&b| b == 0).take(argc).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
    (args.len() == argc).then_some(args)
}

#[cfg(target_os = "linux")]
pub fn proc_argv(pid: u32) -> Option<Vec<String>> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let raw = raw.strip_suffix(&[0]).unwrap_or(&raw);
    Some(raw.split(|&b| b == 0).map(|a| String::from_utf8_lossy(a).into_owned()).collect())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn proc_argv(_pid: u32) -> Option<Vec<String>> {
    None
}

/// Whether `pid` is in the session whose leader is `leader` (a
/// terminal's shell: what runs on that pty).
pub fn in_session(pid: u32, leader: u32) -> bool {
    // SAFETY: getsid only reads
    let sid = unsafe { libc::getsid(pid as libc::pid_t) };
    sid > 0 && sid as u32 == leader
}

/// Seconds since the epoch, now.
pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: u64 = 1795543200;
    /// The spec's examples' token.
    const SPEC_TOKEN: &str = "dummy-token-replace-me";

    #[test]
    fn the_specs_clear_example_is_read() {
        let body = "pid=74877:time=1795543200:clear=1:mac=c30603369f98fef581a4098be889175b0c74f0641e49f8b25f4326cb38f97c93";
        assert_eq!(parse(body, SPEC_TOKEN, T), Ok(Report { pid: 74877, time: T, ask: Ask::Clear }));
        // and written as the spec writes it
        assert_eq!(encode(74877, T, &Ask::Clear, SPEC_TOKEN), body);
    }

    #[test]
    fn the_specs_lazygit_example_is_read() {
        let body = "pid=74877:time=1795543200:argv=bGF6eWdpdA==:cwd=L2hvbWUvdXNlci9zcmMvcHJvamVjdA==:mac=35cb909805c380336efd3689a125acc774a9dec8422a1d21492ebec06ca758e1";
        assert_eq!(parse(body, SPEC_TOKEN, T), Ok(Report { pid: 74877, time: T, ask: Ask::Resume { argv: vec!["lazygit".into()], cwd: Some("/home/user/src/project".into()) } }));
    }

    #[test]
    fn what_a_program_writes_is_read_back() {
        let ask = Ask::Resume { argv: vec!["claude".into(), "--resume".into(), "0b1c-aaaa".into(), "".into(), "a b:c;d".into()], cwd: Some("/tmp/x y".into()) };
        let body = encode(42, T, &ask, "tok_ABCDEFGHIJKLMNOPQRSTUV");
        assert_eq!(parse(&body, "tok_ABCDEFGHIJKLMNOPQRSTUV", T + 5), Ok(Report { pid: 42, time: T, ask }));
    }

    #[test]
    fn a_report_without_the_token_or_tampered_with_is_forged() {
        let body = encode(42, T, &Ask::Clear, "the-right-token-0123456789");
        assert_eq!(parse(&body, "another-token-0123456789ab", T), Err(Reject::Forged));
        let tampered = body.replace("pid=42", "pid=43");
        assert_eq!(parse(&tampered, "the-right-token-0123456789", T), Err(Reject::Forged));
        assert_eq!(parse("?", "the-right-token-0123456789", T), Err(Reject::Forged));
        assert_eq!(parse("pid=1:time=2", "the-right-token-0123456789", T), Err(Reject::Forged));
    }

    #[test]
    fn a_report_out_of_its_time_is_stale() {
        let body = encode(42, T, &Ask::Clear, "the-right-token-0123456789");
        assert_eq!(parse(&body, "the-right-token-0123456789", T + 61), Err(Reject::Stale));
        assert!(parse(&body, "the-right-token-0123456789", T - 60).is_ok());
    }

    #[test]
    fn genuine_but_unacceptable_reports_say_why() {
        let tok = "the-right-token-0123456789";
        let signed = |body: &str| format!("{body}:mac={}", mac(body, tok));
        let bad = |body: &str| matches!(parse(&signed(body), tok, T), Err(Reject::Unacceptable(_)));
        // a relative program, an option for a program, no argv nor clear,
        // both, an empty argv, a relative cwd, a key needed and unknown
        assert!(bad(&format!("pid=1:time={T}:argv={}", b64_encode(b"bin/x"))));
        assert!(bad(&format!("pid=1:time={T}:argv={}", b64_encode(b"-x"))));
        assert!(bad(&format!("pid=1:time={T}")));
        assert!(bad(&format!("pid=1:time={T}:argv={}:clear=1", b64_encode(b"x"))));
        assert!(bad(&format!("pid=1:time={T}:argv=")));
        assert!(bad(&format!("pid=1:time={T}:argv={}:cwd={}", b64_encode(b"x"), b64_encode(b"rel"))));
        assert!(bad(&format!("pid=1:time={T}:argv={}:need=env", b64_encode(b"x"))));
        // a key not needed and unknown is passed over
        assert!(parse(&signed(&format!("pid=1:time={T}:argv={}:future=1", b64_encode(b"x"))), tok, T).is_ok());
    }

    #[test]
    fn base64_both_ways_and_padding_optional() {
        for s in ["", "a", "ab", "abc", "abcd", "nvim", "/home/user/src/project"] {
            let e = b64_encode(s.as_bytes());
            assert_eq!(b64_decode(&e).as_deref(), Some(s.as_bytes()));
            assert_eq!(b64_decode(e.trim_end_matches('=')).as_deref(), Some(s.as_bytes()));
        }
        assert_eq!(b64_encode(b"nvim"), "bnZpbQ==");
        assert_eq!(b64_decode("not base64!"), None);
    }

    #[test]
    fn only_the_programs_listed_are_resumed() {
        assert!(allowed(DEFAULT_PROGRAMS, "claude"));
        assert!(allowed(DEFAULT_PROGRAMS, "/opt/homebrew/bin/codex"));
        assert!(!allowed(DEFAULT_PROGRAMS, "nvim"));
        assert!(!allowed("claude off", "claude"), "off is off");
        assert!(!allowed("", "claude"));
    }

    #[test]
    fn this_process_is_seen_as_it_is() {
        let me = std::process::id();
        let p = proc_info(me).expect("this process");
        // SAFETY: getuid only reads
        assert_eq!(p.uid, unsafe { libc::getuid() });
        let r = Record { pid: Some(me), started: Some(p.started), time: 1, argv: vec!["x".into()], cwd: None };
        assert!(!r.gone());
        assert!(Record { started: Some(p.started + 1), ..r.clone() }.gone(), "another process with the pid");
        assert_eq!(proc_cwd(me).map(std::path::PathBuf::from), std::env::current_dir().ok());
        assert_eq!(proc_argv(me), Some(std::env::args().collect::<Vec<_>>()));
    }

    #[test]
    fn tokens_are_long_random_and_of_the_alphabet() {
        let (a, b) = (token(), token());
        assert_ne!(a, b);
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-'));
    }
}
