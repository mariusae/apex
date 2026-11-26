//! apex as xclip, xsel, wl-paste and wl-copy, for programs in a session's
//! terminals on a host with no clipboard of its own: the app's -- the
//! Mac's -- read and written through the daemon (`apex_server::clip`).
//! So a program that pastes an image there (Claude Code's ^V runs
//! `xclip -selection clipboard -t TARGETS -o`, then `-t image/png -o`)
//! has the one on the Mac.
//!
//! Each answers as the tool does: the same options (xclip's abbreviated
//! as it lets them be, xsel's and wl-paste's short ones run together),
//! the same output byte for byte (xclip's targets a line each, raw bytes
//! for a target, nothing added; wl-paste's types a line each, a newline
//! after text unless `-n`) and the same errors and exit status (xclip's
//! `Error: target T not available`, wl-paste's `Nothing is copied`). The
//! Mac has one clipboard: the primary and secondary selections are it.
//! What none can do here (wl-paste --watch, xclip -loops) says so.

use std::io::Read;

/// The clipboard as the daemon offers it: what there is, what there is
/// of a kind (none when there is none of it), and putting something there.
pub trait Clip {
    fn targets(&mut self) -> Result<Vec<String>, String>;
    fn get(&mut self, want: &str) -> Result<Option<Vec<u8>>, String>;
    fn put(&mut self, want: &str, bytes: &[u8]) -> Result<(), String>;
}

/// What a run printed and how it ended.
#[derive(Default, Debug, PartialEq)]
pub struct Out {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub status: i32,
}

impl Out {
    fn err(mut self, msg: &str) -> Out {
        self.stderr.extend_from_slice(msg.as_bytes());
        if !msg.ends_with('\n') {
            self.stderr.push(b'\n');
        }
        self.status = 1;
        self
    }
}

/// X's names for text, which xclip asks for and lists.
const X_TEXT: [&str; 5] = ["UTF8_STRING", "TEXT", "STRING", "text/plain;charset=utf-8", "text/plain"];

/// The daemon's word for a target or MIME type: `text`, `image/png`,
/// `image/jpeg`; none for anything else (not to be had here).
pub fn want_of(target: &str) -> Option<&'static str> {
    let t = target.to_ascii_lowercase();
    if X_TEXT.iter().any(|x| x.eq_ignore_ascii_case(target)) || t == "compound_text" || t.starts_with("text/plain") {
        Some("text")
    } else if t == "image/png" {
        Some("image/png")
    } else if t == "image/jpeg" || t == "image/jpg" {
        Some("image/jpeg")
    } else {
        None
    }
}

/// xclip's targets for what there is: TARGETS first, then each kind's
/// names (text's as X apps offer it).
fn x_targets(there: &[String]) -> Vec<String> {
    let mut out = vec!["TARGETS".to_string()];
    for k in there {
        match k.as_str() {
            "text" => out.extend(X_TEXT.iter().map(|s| s.to_string())),
            other => out.push(other.to_string()),
        }
    }
    out
}

/// wl-paste's MIME types for what there is.
fn mime_types(there: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for k in there {
        match k.as_str() {
            "text" => out.extend(["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "TEXT", "STRING"].iter().map(|s| s.to_string())),
            other => out.push(other.to_string()),
        }
    }
    out
}

/// What a write is, by its first bytes, where nothing says (wl-copy).
fn infer(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else {
        "text"
    }
}

/// Run `tool` (`xclip`, `xsel`, `wl-paste`, `wl-copy`) with its
/// arguments, `stdin` its input when it reads any.
pub fn run(tool: &str, args: &[String], clip: &mut dyn Clip, stdin: &mut dyn Read) -> Out {
    match tool {
        "xclip" => xclip(args, clip, stdin),
        "xsel" => xsel(args, clip, stdin),
        "wl-paste" => wl_paste(args, clip),
        "wl-copy" => wl_copy(args, clip, stdin),
        _ => Out::default().err(&format!("{tool}: not a clipboard tool apex stands in for")),
    }
}

/// xclip's option `arg` as the one of `names` it abbreviates (X's way:
/// any unique start, a dash or two before it).
fn x_option<'a>(arg: &str, names: &[&'a str]) -> Result<Option<&'a str>, String> {
    let Some(a) = arg.strip_prefix('-') else { return Ok(None) };
    let a = a.strip_prefix('-').unwrap_or(a);
    if a.is_empty() {
        return Ok(None);
    }
    if let Some(exact) = names.iter().find(|n| **n == a) {
        return Ok(Some(exact));
    }
    let hits: Vec<&&str> = names.iter().filter(|n| n.starts_with(a)).collect();
    match hits.as_slice() {
        [one] => Ok(Some(one)),
        [] => Ok(None),
        _ => Err(format!("-{a}: ambiguous")),
    }
}

const XCLIP_USAGE: &str = "Usage: xclip [OPTION] [FILE]...
Access an X server selection for reading or writing (apex: the Mac's clipboard).

  -i, -in          read text into X selection from standard input or files
                   (default)
  -o, -out         prints the selection to standard out (generally for
                   piping to a file or program)
  -l, -loops       number of selection requests to wait for before exiting
  -d, -display     X display to connect to (eg localhost:0\")
  -h, -help        usage information
      -selection   selection to access (\"primary\", \"secondary\", \"clipboard\" or \"buffer-cut\")
      -noutf8      don't treat text as utf-8, use old unicode
      -target      use the given target atom
      -rmlastnl    remove the last newline character if present
      -version     version information
      -silent      errors only, run in background (default)
      -quiet       run in foreground, show what's happening
      -verbose     running commentary
";

fn xclip(args: &[String], clip: &mut dyn Clip, stdin: &mut dyn Read) -> Out {
    const NAMES: [&str; 18] = ["in", "i", "out", "o", "filter", "f", "loops", "l", "display", "d", "help", "h", "selection", "noutf8", "target", "t", "rmlastnl", "version"];
    const MORE: [&str; 4] = ["silent", "quiet", "verbose", "sensitive"];
    let all: Vec<&str> = NAMES.iter().chain(MORE.iter()).copied().collect();
    let (mut output, mut filter, mut rmlastnl, mut noutf8) = (false, false, false, false);
    let mut target: Option<String> = None;
    let mut files = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let opt = match x_option(a, &all) {
            Ok(o) => o,
            Err(e) => return Out::default().err(&format!("xclip: {e}")),
        };
        // the options that take a value
        let mut value = || {
            i += 1;
            args.get(i).cloned()
        };
        match opt {
            Some("out" | "o") => output = true,
            Some("in" | "i") => output = false,
            Some("filter" | "f") => filter = true,
            Some("rmlastnl") => rmlastnl = true,
            Some("noutf8") => noutf8 = true,
            Some("help" | "h") => return Out { stdout: XCLIP_USAGE.as_bytes().to_vec(), ..Out::default() },
            Some("version") => return Out { stderr: b"xclip version 0.13 (apex: the Mac's clipboard)\n".to_vec(), ..Out::default() },
            Some("target" | "t") => match value() {
                Some(v) => target = Some(v),
                None => return Out::default().err("xclip: -target needs a value"),
            },
            // the selection: the Mac has one clipboard, whichever is named
            Some("selection" | "display" | "d") => {
                if value().is_none() {
                    return Out::default().err(&format!("xclip: {a} needs a value"));
                }
            }
            Some("loops" | "l") => {
                if value().is_none() {
                    return Out::default().err("xclip: -loops needs a value");
                }
            }
            Some(_) => {} // silent, quiet, verbose, sensitive
            None if a.starts_with('-') && a.len() > 1 => return Out::default().err(&format!("xclip: {a}: unknown option\n{XCLIP_USAGE}")),
            None => files.push(a.clone()),
        }
        i += 1;
    }
    let target = target.unwrap_or_else(|| if noutf8 { "STRING".into() } else { "UTF8_STRING".into() });
    if output {
        let there = match clip.targets() {
            Ok(t) => t,
            Err(e) => return Out::default().err(&format!("xclip: {e}")),
        };
        if target == "TARGETS" {
            if there.is_empty() {
                return Out::default().err("Error: target TARGETS not available");
            }
            let mut out = x_targets(&there).join("\n").into_bytes();
            out.push(b'\n');
            return Out { stdout: out, ..Out::default() };
        }
        let not_there = || Out::default().err(&format!("Error: target {target} not available"));
        let Some(want) = want_of(&target).filter(|w| there.iter().any(|t| t == w)) else { return not_there() };
        return match clip.get(want) {
            Ok(Some(mut b)) => {
                if rmlastnl && want == "text" && b.last() == Some(&b'\n') {
                    b.pop();
                }
                Out { stdout: b, ..Out::default() }
            }
            Ok(None) => not_there(),
            Err(e) => Out::default().err(&format!("xclip: {e}")),
        };
    }
    // in: files, else standard input
    let mut data = Vec::new();
    if files.is_empty() {
        if let Err(e) = stdin.read_to_end(&mut data) {
            return Out::default().err(&format!("xclip: {e}"));
        }
    } else {
        for f in &files {
            match std::fs::read(f) {
                Ok(b) => data.extend_from_slice(&b),
                Err(e) => return Out::default().err(&format!("xclip: {f}: {e}")),
            }
        }
    }
    let mut out = Out::default();
    if filter {
        out.stdout = data.clone();
    }
    if rmlastnl && data.last() == Some(&b'\n') {
        data.pop();
    }
    let Some(want) = want_of(&target) else { return out.err(&format!("xclip: target {target}: not something the Mac's clipboard takes")) };
    match clip.put(want, &data) {
        Ok(()) => out,
        Err(e) => out.err(&format!("xclip: {e}")),
    }
}

/// Short options run together (`-bo`) as one each; long ones as they
/// are, a `--name=value` split.
fn spread(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for a in args {
        if let Some(long) = a.strip_prefix("--") {
            match long.split_once('=') {
                Some((k, v)) => {
                    out.push(format!("--{k}"));
                    out.push(v.to_string());
                }
                None => out.push(a.clone()),
            }
        } else if a.len() > 2 && a.starts_with('-') {
            out.extend(a[1..].chars().map(|c| format!("-{c}")));
        } else {
            out.push(a.clone());
        }
    }
    out
}

fn xsel(args: &[String], clip: &mut dyn Clip, stdin: &mut dyn Read) -> Out {
    let (mut input, mut output, mut clear, mut append) = (false, false, false, false);
    let args = spread(args);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-i" | "--input" => input = true,
            "-o" | "--output" => output = true,
            "-a" | "--append" => {
                input = true;
                append = true;
            }
            "-c" | "--clear" | "-d" | "--delete" => clear = true,
            "-p" | "--primary" | "-s" | "--secondary" | "-b" | "--clipboard" | "-k" | "--keep" | "-x" | "--exchange" | "-n" | "--nodetach" | "-v" | "--verbose" | "-z" | "--zeroflush" => {}
            "--display" | "-t" | "--selectionTimeout" | "-l" | "--logfile" => i += 1,
            "-f" | "--follow" => return Out::default().err("xsel: --follow: not here (apex: the Mac's clipboard is put once)"),
            "-h" | "--help" => return Out { stdout: b"Usage: xsel [options]\n  -a, --append, -f, --follow, -i, --input, -o, --output, -c, --clear,\n  -p, --primary, -s, --secondary, -b, --clipboard (apex: the Mac's clipboard)\n".to_vec(), ..Out::default() },
            "--version" => return Out { stdout: b"xsel version 1.2.1 (apex: the Mac's clipboard)\n".to_vec(), ..Out::default() },
            a => return Out::default().err(&format!("xsel: Invalid option: {a}")),
        }
        i += 1;
    }
    if clear {
        return match clip.put("text", b"") {
            Ok(()) => Out::default(),
            Err(e) => Out::default().err(&format!("xsel: {e}")),
        };
    }
    // neither said: in when the input is not a terminal, else out
    if !input && !output {
        if is_tty(0) {
            output = true;
        } else {
            input = true;
        }
    }
    let mut out = Out::default();
    if output {
        match clip.targets().and_then(|there| if there.iter().any(|t| t == "text") { clip.get("text") } else { Ok(None) }) {
            Ok(Some(b)) => out.stdout = b,
            Ok(None) => {}
            Err(e) => return out.err(&format!("xsel: {e}")),
        }
    }
    if input {
        let mut data = Vec::new();
        if append {
            if let Ok(Some(b)) = clip.targets().and_then(|t| if t.iter().any(|x| x == "text") { clip.get("text") } else { Ok(None) }) {
                data = b;
            }
        }
        if let Err(e) = stdin.read_to_end(&mut data) {
            return out.err(&format!("xsel: {e}"));
        }
        if let Err(e) = clip.put("text", &data) {
            return out.err(&format!("xsel: {e}"));
        }
    }
    out
}

/// Whether file descriptor `fd` is a terminal.
fn is_tty(fd: i32) -> bool {
    use std::io::IsTerminal;
    match fd {
        0 => std::io::stdin().is_terminal(),
        _ => false,
    }
}

fn wl_paste(args: &[String], clip: &mut dyn Clip) -> Out {
    let args = spread(args);
    let (mut list, mut no_newline) = (false, false);
    let mut ty: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-l" | "--list-types" => list = true,
            "-n" | "--no-newline" => no_newline = true,
            "-p" | "--primary" => {}
            "-t" | "--type" => {
                i += 1;
                match args.get(i) {
                    Some(t) => ty = Some(t.clone()),
                    None => return Out::default().err("wl-paste: option requires an argument -- 't'"),
                }
            }
            "-s" | "--seat" => i += 1,
            "-w" | "--watch" => return Out::default().err("wl-paste: --watch: not here (apex: the Mac's clipboard is read once)"),
            "-h" | "--help" => return Out { stdout: b"Usage: wl-paste [options]\n  -l, --list-types  -t, --type TYPE  -n, --no-newline  -p, --primary\n(apex: the Mac's clipboard)\n".to_vec(), ..Out::default() },
            "-v" | "--version" => return Out { stdout: b"wl-clipboard 2.2.1 (apex: the Mac's clipboard)\n".to_vec(), ..Out::default() },
            a => return Out::default().err(&format!("wl-paste: unrecognized option '{a}'")),
        }
        i += 1;
    }
    let there = match clip.targets() {
        Ok(t) => t,
        Err(e) => return Out::default().err(&format!("wl-paste: {e}")),
    };
    if there.is_empty() {
        return Out::default().err("Nothing is copied");
    }
    let types = mime_types(&there);
    if list {
        let mut out = types.join("\n").into_bytes();
        out.push(b'\n');
        return Out { stdout: out, ..Out::default() };
    }
    // the type asked for (a main type alone, `text` or `image`, any of
    // it), else text
    let chosen = match &ty {
        Some(t) if t.contains('/') || types.contains(t) => types.iter().find(|x| x.eq_ignore_ascii_case(t)).cloned(),
        Some(main) => types.iter().find(|x| x.split('/').next() == Some(main.as_str())).cloned(),
        None => types.iter().find(|x| want_of(x) == Some("text")).cloned(),
    };
    let Some(chosen) = chosen else {
        return match ty {
            Some(t) => Out::default().err(&format!("Clipboard content is not available as requested type \"{t}\"")),
            None => Out::default().err("No suitable type of content copied"),
        };
    };
    let Some(want) = want_of(&chosen) else { return Out::default().err(&format!("Clipboard content is not available as requested type \"{chosen}\"")) };
    match clip.get(want) {
        Ok(Some(mut b)) => {
            if want == "text" && !no_newline {
                b.push(b'\n');
            }
            Out { stdout: b, ..Out::default() }
        }
        Ok(None) => Out::default().err(&format!("Clipboard content is not available as requested type \"{chosen}\"")),
        Err(e) => Out::default().err(&format!("wl-paste: {e}")),
    }
}

fn wl_copy(args: &[String], clip: &mut dyn Clip, stdin: &mut dyn Read) -> Out {
    let args = spread(args);
    let (mut clear, mut trim) = (false, false);
    let mut ty: Option<String> = None;
    let mut words: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "-c" | "--clear" => clear = true,
            "-n" | "--trim-newline" => trim = true,
            "-o" | "--paste-once" | "-f" | "--foreground" | "-p" | "--primary" | "--regular" => {}
            "-t" | "--type" => {
                i += 1;
                match args.get(i) {
                    Some(t) => ty = Some(t.clone()),
                    None => return Out::default().err("wl-copy: option requires an argument -- 't'"),
                }
            }
            "-s" | "--seat" => i += 1,
            "-h" | "--help" => return Out { stdout: b"Usage: wl-copy [options] text...\n  -c, --clear  -n, --trim-newline  -t, --type TYPE  -p, --primary\n(apex: the Mac's clipboard)\n".to_vec(), ..Out::default() },
            "-v" | "--version" => return Out { stdout: b"wl-clipboard 2.2.1 (apex: the Mac's clipboard)\n".to_vec(), ..Out::default() },
            "--" => {
                words.extend(args[i + 1..].iter().cloned());
                break;
            }
            a if a.starts_with('-') && a.len() > 1 => return Out::default().err(&format!("wl-copy: unrecognized option '{a}'")),
            a => words.push(a.to_string()),
        }
        i += 1;
    }
    if clear {
        return match clip.put("text", b"") {
            Ok(()) => Out::default(),
            Err(e) => Out::default().err(&format!("wl-copy: {e}")),
        };
    }
    let mut data = Vec::new();
    if words.is_empty() {
        if let Err(e) = stdin.read_to_end(&mut data) {
            return Out::default().err(&format!("wl-copy: {e}"));
        }
    } else {
        data = words.join(" ").into_bytes();
    }
    if trim && data.last() == Some(&b'\n') {
        data.pop();
    }
    let want = match &ty {
        Some(t) => match want_of(t) {
            Some(w) => w,
            None => return Out::default().err(&format!("wl-copy: type {t}: not something the Mac's clipboard takes")),
        },
        None => infer(&data),
    };
    match clip.put(want, &data) {
        Ok(()) => Out::default(),
        Err(e) => Out::default().err(&format!("wl-copy: {e}")),
    }
}

/// The clipboard through the daemon a session's terminal names
/// (`APEX_SOCKET`, `apexsession`), over the I/O plane.
pub struct Daemon {
    plane: apex_server::plane::IoPlane,
    // held open while the plane is used
    _remote: apex_server::remote::Remote,
    targets: Option<Vec<String>>,
}

impl Daemon {
    pub fn connect(socket: &std::path::Path, session: &str, tool: &str) -> Result<Daemon, String> {
        let remote = apex_server::remote::Remote::connect_as(socket, session, tool, apex_core::AttachmentKind::Tool).map_err(|e| format!("{}: {e}", socket.display()))?;
        Ok(Daemon { plane: remote.io_plane(), _remote: remote, targets: None })
    }

    fn fetch(&self, method: &str, want: &str, body: Option<&[u8]>) -> Result<(u16, Vec<u8>), String> {
        let (status, _, b) = self.plane.fetch(method, &format!("apex-clip:{want}"), &[], body, std::time::Duration::from_secs(25))?;
        Ok((status, b))
    }
}

/// A refusal in words: what the daemon said, else its status.
fn refused(status: u16, body: &[u8]) -> String {
    let t = String::from_utf8_lossy(body).trim().to_string();
    if t.is_empty() {
        format!("the Mac's clipboard: {status}")
    } else {
        format!("the Mac's clipboard: {t}")
    }
}

impl Clip for Daemon {
    fn targets(&mut self) -> Result<Vec<String>, String> {
        if let Some(t) = &self.targets {
            return Ok(t.clone());
        }
        let (status, b) = self.fetch("GET", "TARGETS", None)?;
        if status != 200 {
            return Err(refused(status, &b));
        }
        let t: Vec<String> = String::from_utf8_lossy(&b).lines().filter(|l| !l.is_empty()).map(String::from).collect();
        self.targets = Some(t.clone());
        Ok(t)
    }

    fn get(&mut self, want: &str) -> Result<Option<Vec<u8>>, String> {
        let (status, b) = self.fetch("GET", want, None)?;
        match status {
            200 => Ok(Some(b)),
            404 => Ok(None),
            s => Err(refused(s, &b)),
        }
    }

    fn put(&mut self, want: &str, bytes: &[u8]) -> Result<(), String> {
        let (status, b) = self.fetch("PUT", want, Some(bytes))?;
        if status == 200 {
            Ok(())
        } else {
            Err(refused(status, &b))
        }
    }
}

/// apex run as `tool` (by the link's name): the clipboard through the
/// daemon of the session the terminal is in. Its exit status.
pub fn main(tool: &str, args: &[String]) -> i32 {
    use std::io::Write;
    let socket = std::env::var("APEX_SOCKET").map(std::path::PathBuf::from).unwrap_or_else(|_| apex_server::daemon::default_socket());
    let session = std::env::var("apexsession").or_else(|_| std::env::var("APEX_SESSION")).unwrap_or_else(|_| "default".into());
    // help and version need no daemon
    let asks_help = args.iter().any(|a| matches!(a.as_str(), "-h" | "-help" | "--help" | "-version" | "--version" | "-v"));
    let out = if asks_help {
        struct None_;
        impl Clip for None_ {
            fn targets(&mut self) -> Result<Vec<String>, String> {
                Err("no clipboard".into())
            }
            fn get(&mut self, _: &str) -> Result<Option<Vec<u8>>, String> {
                Err("no clipboard".into())
            }
            fn put(&mut self, _: &str, _: &[u8]) -> Result<(), String> {
                Err("no clipboard".into())
            }
        }
        run(tool, args, &mut None_, &mut std::io::stdin())
    } else {
        match Daemon::connect(&socket, &session, tool) {
            Ok(mut d) => run(tool, args, &mut d, &mut std::io::stdin()),
            Err(e) => Out::default().err(&format!("{tool}: {e}")),
        }
    };
    let _ = std::io::stdout().write_all(&out.stdout);
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().write_all(&out.stderr);
    out.status
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clipboard of the test's: what there is, by kind.
    #[derive(Default)]
    struct Fake {
        there: Vec<(String, Vec<u8>)>,
        put: Vec<(String, Vec<u8>)>,
    }

    impl Fake {
        fn with(kinds: &[(&str, &[u8])]) -> Fake {
            Fake { there: kinds.iter().map(|(k, b)| (k.to_string(), b.to_vec())).collect(), put: Vec::new() }
        }
    }

    impl Clip for Fake {
        fn targets(&mut self) -> Result<Vec<String>, String> {
            Ok(self.there.iter().map(|(k, _)| k.clone()).collect())
        }
        fn get(&mut self, want: &str) -> Result<Option<Vec<u8>>, String> {
            Ok(self.there.iter().find(|(k, _)| k == want).map(|(_, b)| b.clone()))
        }
        fn put(&mut self, want: &str, bytes: &[u8]) -> Result<(), String> {
            self.put.push((want.to_string(), bytes.to_vec()));
            Ok(())
        }
    }

    fn go(tool: &str, args: &str, clip: &mut Fake, stdin: &[u8]) -> Out {
        let args: Vec<String> = args.split_whitespace().map(String::from).collect();
        run(tool, &args, clip, &mut &stdin[..])
    }

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00binary\n\x00";

    #[test]
    fn claude_codes_paste_of_an_image_through_xclip() {
        // what Claude Code runs on Linux, as it runs it
        let mut c = Fake::with(&[("image/png", PNG)]);
        let t = go("xclip", "-selection clipboard -o -t TARGETS", &mut c, b"");
        assert_eq!((t.status, String::from_utf8_lossy(&t.stdout).as_ref()), (0, "TARGETS\nimage/png\n"));
        assert!(t.stderr.is_empty());
        let i = go("xclip", "-selection clipboard -o -t image/png", &mut c, b"");
        assert_eq!((i.status, i.stdout.as_slice()), (0, PNG), "the bytes as they are, nothing added");
    }

    #[test]
    fn xclip_says_what_is_not_there_as_xclip_says_it() {
        let mut c = Fake::with(&[("text", b"hello")]);
        let o = go("xclip", "-sel c -o -t image/png", &mut c, b"");
        assert_eq!((o.status, String::from_utf8_lossy(&o.stderr).as_ref(), o.stdout.len()), (1, "Error: target image/png not available\n", 0));
        // nothing there at all
        let mut empty = Fake::default();
        let o = go("xclip", "-selection clipboard -o -t TARGETS", &mut empty, b"");
        assert_eq!((o.status, String::from_utf8_lossy(&o.stderr).as_ref()), (1, "Error: target TARGETS not available\n"));
        let o = go("xclip", "-o", &mut empty, b"");
        assert_eq!((o.status, String::from_utf8_lossy(&o.stderr).as_ref()), (1, "Error: target UTF8_STRING not available\n"));
    }

    #[test]
    fn xclip_reads_text_by_any_of_its_names_and_lists_them() {
        let mut c = Fake::with(&[("image/png", PNG), ("text", b"two\nlines\n")]);
        let t = go("xclip", "-selection clipboard -t TARGETS -o", &mut c, b"");
        assert_eq!(String::from_utf8_lossy(&t.stdout), "TARGETS\nimage/png\nUTF8_STRING\nTEXT\nSTRING\ntext/plain;charset=utf-8\ntext/plain\n");
        for target in ["", "-t UTF8_STRING", "-t STRING", "-t text/plain", "-target text/plain;charset=utf-8"] {
            let o = go("xclip", &format!("-o {target}"), &mut c, b"");
            assert_eq!((o.status, o.stdout.as_slice()), (0, &b"two\nlines\n"[..]), "{target}");
        }
        let o = go("xclip", "-o -rmlastnl", &mut c, b"");
        assert_eq!(o.stdout, b"two\nlines");
        // abbreviated as X lets an option be; one that could be two is not
        let o = go("xclip", "-out -selec clip -targ TARGETS", &mut c, b"");
        assert_eq!(o.status, 0);
        let o = go("xclip", "-s clipboard -o", &mut c, b"");
        assert_eq!(o.status, 1, "-s: selection or silent");
    }

    #[test]
    fn xclip_puts_what_it_is_given() {
        let mut c = Fake::default();
        let o = go("xclip", "-selection clipboard", &mut c, b"copied\n");
        assert_eq!(o.status, 0);
        assert_eq!(c.put, vec![("text".to_string(), b"copied\n".to_vec())]);
        let o = go("xclip", "-selection clipboard -i -t image/png -f", &mut c, PNG);
        assert_eq!((o.status, o.stdout.as_slice()), (0, PNG), "-filter prints what it was given");
        assert_eq!(c.put.last().unwrap(), &("image/png".to_string(), PNG.to_vec()));
        go("xclip", "-rmlastnl", &mut c, b"trimmed\n");
        assert_eq!(c.put.last().unwrap().1, b"trimmed");
    }

    #[test]
    fn wl_paste_as_claude_code_runs_it_and_as_people_do() {
        let mut c = Fake::with(&[("image/png", PNG), ("text", b"hi")]);
        let l = go("wl-paste", "--list-types", &mut c, b"");
        assert_eq!(String::from_utf8_lossy(&l.stdout), "image/png\ntext/plain;charset=utf-8\ntext/plain\nUTF8_STRING\nTEXT\nSTRING\n");
        let i = go("wl-paste", "--type image/png", &mut c, b"");
        assert_eq!((i.status, i.stdout.as_slice()), (0, PNG), "an image: no newline after it");
        assert_eq!(go("wl-paste", "", &mut c, b"").stdout, b"hi\n", "text: a newline after it");
        assert_eq!(go("wl-paste", "-n", &mut c, b"").stdout, b"hi");
        assert_eq!(go("wl-paste", "--no-newline --type text", &mut c, b"").stdout, b"hi", "a main type alone");
        assert_eq!(go("wl-paste", "-nt image", &mut c, b"").stdout, PNG, "short options run together");
        assert_eq!(go("wl-paste", "--type=text/plain -n", &mut c, b"").stdout, b"hi");
        let o = go("wl-paste", "-t image/jpeg", &mut c, b"");
        assert_eq!((o.status, String::from_utf8_lossy(&o.stderr).as_ref()), (1, "Clipboard content is not available as requested type \"image/jpeg\"\n"));
        let o = go("wl-paste", "", &mut Fake::default(), b"");
        assert_eq!((o.status, String::from_utf8_lossy(&o.stderr).as_ref()), (1, "Nothing is copied\n"));
        let o = go("wl-paste", "", &mut Fake::with(&[("image/png", PNG)]), b"");
        assert_eq!((o.status, String::from_utf8_lossy(&o.stderr).as_ref()), (1, "No suitable type of content copied\n"));
    }

    #[test]
    fn wl_copy_and_xsel_put_and_read_text() {
        let mut c = Fake::default();
        assert_eq!(go("wl-copy", "some words", &mut c, b"").status, 0);
        assert_eq!(c.put.last().unwrap(), &("text".to_string(), b"some words".to_vec()));
        go("wl-copy", "-n", &mut c, b"line\n");
        assert_eq!(c.put.last().unwrap().1, b"line");
        go("wl-copy", "", &mut c, PNG);
        assert_eq!(c.put.last().unwrap().0, "image/png", "what it is, from its bytes");
        go("wl-copy", "--clear", &mut c, b"");
        assert_eq!(c.put.last().unwrap(), &("text".to_string(), Vec::new()));
        let mut c = Fake::with(&[("text", b"sel")]);
        assert_eq!(go("xsel", "-bo", &mut c, b"").stdout, b"sel", "short options run together, no newline added");
        assert_eq!(go("xsel", "--clipboard --output", &mut c, b"").stdout, b"sel");
        go("xsel", "-bi", &mut c, b"new");
        assert_eq!(c.put.last().unwrap(), &("text".to_string(), b"new".to_vec()));
        assert_eq!(go("xsel", "-ob", &mut Fake::with(&[("image/png", PNG)]), b"").stdout, b"", "no text: nothing, as xsel prints");
    }

    #[test]
    fn kinds_are_the_daemons_words() {
        for t in ["UTF8_STRING", "STRING", "TEXT", "text/plain", "text/plain;charset=utf-8", "TEXT/PLAIN"] {
            assert_eq!(want_of(t), Some("text"), "{t}");
        }
        assert_eq!(want_of("image/png"), Some("image/png"));
        assert_eq!(want_of("image/jpg"), Some("image/jpeg"));
        assert_eq!(want_of("image/gif"), None);
        assert_eq!(want_of("TARGETS"), None);
        for w in ["text", "image/png", "image/jpeg"] {
            assert!(apex_server::clip::valid_want(w));
        }
    }
}
