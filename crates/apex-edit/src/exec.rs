//! Command execution: a port of plan9port `src/cmd/acme/ecmd.c`.
//!
//! Every command runs against the original text and records its changes in
//! an [`Elog`]; dot is tracked in original coordinates and adjusted once
//! the log is applied. Commands with effects on the world return
//! [`Intent`]s instead of performing anything.

use crate::elog::{adjust_dot, Change, Elog};
use crate::parse::{lookup, Addr, Arg, Cmd, DefAddr, Parser};
use crate::regx::{Rangeset, Regex, INFINITY};
use crate::{Error, Result, Text};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Rg {
    q0: usize,
    q1: usize,
}

impl Rg {
    fn new(q0: usize, q1: usize) -> Rg {
        Rg { q0, q1 }
    }
    fn from_set(s: &Rangeset) -> Rg {
        Rg { q0: s[0].q0.max(0) as usize, q1: s[0].q1.max(0) as usize }
    }
}

/// `> cmd`, `< cmd`, `| cmd`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipeKind {
    /// `>`: send the range to the command's standard input
    To,
    /// `<`: replace the range with the command's output
    From,
    /// `|`: replace the range with the command's output given the range
    Through,
}

/// Something the caller has to do; the crate itself has no effects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Intent {
    /// `u n`: undo the last `n` changes (`n < 0` redoes).
    Undo { n: i64 },
    /// `w [name]`: write the range to the file.
    Write { name: Option<String>, q0: usize, q1: usize },
    /// `r [name]`: replace the range with the file's contents.
    Read { name: Option<String>, q0: usize, q1: usize },
    /// `e [name]`: replace the whole buffer with the file's contents.
    Load { name: Option<String> },
    /// `f [name]`: set (or print) the buffer's name.
    SetName { name: Option<String> },
    /// `b name`: make that buffer current.
    Browse { name: String },
    /// `B list`: open windows on the named files.
    Open { list: String },
    /// `D list`: delete windows on the named files (the current one if empty).
    Delete { list: String },
    /// `< | >`: run a shell command over the range.
    Pipe { kind: PipeKind, cmd: String, q0: usize, q1: usize },
}

/// Something a program printed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Printed {
    /// `p`: the text of the range, verbatim.
    Text(String),
    /// `=`: an address, with a trailing newline.
    Address(String),
}

/// The result of running a program.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Changes in application order, in original coordinates.
    pub changes: Vec<Change>,
    /// Dot after the changes are applied.
    pub dot: (usize, usize),
    /// What `p` and `=` printed, in order.
    pub output: Vec<Printed>,
    pub intents: Vec<Intent>,
    /// acme's warnings (changes out of sequence).
    pub warnings: Vec<String>,
}

impl Outcome {
    /// Everything printed, concatenated as acme shows it in `+Errors`.
    pub fn output_string(&self) -> String {
        self.output
            .iter()
            .map(|p| match p {
                Printed::Text(s) | Printed::Address(s) => s.as_str(),
            })
            .collect()
    }
}

/// An `Edit` interpreter. Keeps the last regular expression, which the
/// empty pattern `//` refers to, across programs.
#[derive(Default)]
pub struct Edit {
    last_pat: Vec<char>,
}

impl Edit {
    pub fn new() -> Edit {
        Edit::default()
    }

    /// Run `program` over `text` with the given dot. `name` is the buffer's
    /// file name, used by `=` and the file commands.
    pub fn run(
        &mut self,
        text: &dyn Text,
        dot: (usize, usize),
        name: Option<&str>,
        program: &str,
    ) -> Result<Outcome> {
        let mut parser = Parser::new(program, &mut self.last_pat);
        let mut ex = Exec {
            t: text,
            name,
            dot: Rg::new(dot.0.min(text.len()), dot.1.min(text.len())),
            addr: Rg::default(),
            elog: Elog::new(),
            out: Vec::new(),
            intents: Vec::new(),
            nest: 0,
            re: None,
        };
        while let Some(cmd) = parser.parsecmd(0)? {
            ex.cmdexec(&cmd)?;
        }
        let (changes, warnings) = ex.elog.finish();
        let dot = adjust_dot((ex.dot.q0, ex.dot.q1), &changes, text.len());
        Ok(Outcome { changes, dot, output: ex.out, intents: ex.intents, warnings })
    }
}

struct Exec<'a> {
    t: &'a dyn Text,
    name: Option<&'a str>,
    /// the current selection (acme's `t->q0`, `t->q1`)
    dot: Rg,
    /// the address of the command being executed (acme's global `addr`)
    addr: Rg,
    elog: Elog,
    out: Vec<Printed>,
    intents: Vec<Intent>,
    nest: i32,
    /// the last compiled regular expression
    re: Option<Regex>,
}

fn err<T>(s: impl Into<String>) -> Result<T> {
    Err(Error(s.into()))
}

fn to_string(r: &[char]) -> String {
    r.iter().collect()
}

fn skipbl(r: &[char]) -> &[char] {
    let n = r.iter().take_while(|c| **c == ' ' || **c == '\t' || **c == '\n').count();
    &r[n..]
}

impl<'a> Exec<'a> {
    fn len(&self) -> usize {
        self.t.len()
    }

    fn compile(&mut self, re: &[char], what: &str) -> Result<&Regex> {
        let cached = matches!(&self.re, Some(r) if r.source == re);
        if !cached {
            match Regex::compile(re) {
                Ok(r) => self.re = Some(r),
                Err(e) => return err(format!("{what}: {e}")),
            }
        }
        Ok(self.re.as_ref().unwrap())
    }

    fn cmdexec(&mut self, cp: &Cmd) -> Result<()> {
        let ct = lookup(cp.cmdc);
        if let Some(ct) = ct {
            if ct.defaddr != DefAddr::No {
                let star = if ct.defaddr == DefAddr::All { '*' } else { '.' };
                let mut ap: Option<Addr> = cp.addr.as_deref().cloned();
                if ap.is_none() && cp.cmdc != '\n' {
                    ap = Some(Addr::simple(star));
                } else if let Some(a) = &mut ap {
                    if a.typ == '"' && a.next.is_none() && cp.cmdc != '\n' {
                        a.next = Some(Box::new(Addr::simple(star)));
                    }
                }
                if let Some(a) = &ap {
                    let dot = self.dot;
                    self.addr = self.cmdaddress(a, dot, 0)?;
                }
            }
        }
        match cp.cmdc {
            '{' => {
                let mut dot = self.dot;
                if let Some(a) = &cp.addr {
                    dot = self.cmdaddress(a, dot, 0)?;
                }
                let mut sub = match &cp.arg {
                    Arg::Cmd(c) => Some(c.as_ref()),
                    _ => None,
                };
                while let Some(c) = sub {
                    if dot.q1 > self.len() {
                        return err("dot extends past end of buffer during { command");
                    }
                    self.dot = dot;
                    self.cmdexec(c)?;
                    sub = c.next.as_deref();
                }
                Ok(())
            }
            _ => match ct {
                None => err(format!("unknown command {} in cmdexec", cp.cmdc)),
                Some(_) => self.dispatch(cp),
            },
        }
    }

    fn dispatch(&mut self, cp: &Cmd) -> Result<()> {
        match cp.cmdc {
            '\n' => self.nl_cmd(cp),
            'a' => self.append(cp, self.addr.q1),
            'i' => self.append(cp, self.addr.q0),
            'c' => {
                let a = self.addr;
                self.elog.replace(self.t, a.q0, a.q1, cp.text())?;
                self.dot = a;
                Ok(())
            }
            'd' => {
                let a = self.addr;
                if a.q1 > a.q0 {
                    self.elog.delete(a.q0, a.q1)?;
                }
                self.dot = Rg::new(a.q0, a.q0);
                Ok(())
            }
            's' => self.s_cmd(cp),
            'm' | 't' => self.m_cmd(cp),
            'p' => self.p_cmd(),
            '=' => self.eq_cmd(cp),
            'x' | 'y' => {
                if cp.re.is_some() {
                    self.looper(cp, cp.cmdc == 'x')
                } else {
                    self.linelooper(cp)
                }
            }
            'g' | 'v' => self.g_cmd(cp),
            'u' => {
                self.intents.push(Intent::Undo { n: cp.num });
                Ok(())
            }
            'w' => {
                if self.elog.is_modified() {
                    return err("can't write file with pending modifications");
                }
                let Some(name) = self.cmdname(cp.text()) else {
                    return err("no name specified for 'w' command");
                };
                let a = self.addr;
                self.intents.push(Intent::Write { name: Some(name), q0: a.q0, q1: a.q1 });
                Ok(())
            }
            'e' => {
                let name = self.cmdname(cp.text());
                if name.is_none() {
                    return err("no file name given");
                }
                self.intents.push(Intent::Load { name });
                Ok(())
            }
            'r' => {
                let name = self.cmdname(cp.text());
                if name.is_none() {
                    return err("no file name given");
                }
                let a = self.addr;
                self.intents.push(Intent::Read { name, q0: a.q0, q1: a.q1 });
                Ok(())
            }
            'f' => {
                let name = to_string(skipbl(cp.text()));
                let name = if name.is_empty() { None } else { Some(name) };
                self.intents.push(Intent::SetName { name });
                Ok(())
            }
            'b' => {
                let name = to_string(skipbl(cp.text()));
                if name.is_empty() {
                    return err("no such file\"\"");
                }
                self.intents.push(Intent::Browse { name });
                Ok(())
            }
            'B' => {
                let list = to_string(skipbl(cp.text()));
                if list.is_empty() {
                    return err("no file name given");
                }
                self.intents.push(Intent::Open { list });
                Ok(())
            }
            'D' => {
                self.intents.push(Intent::Delete { list: to_string(skipbl(cp.text())) });
                Ok(())
            }
            'X' | 'Y' => err(format!("{} command is not supported", cp.cmdc)),
            '<' | '|' | '>' => {
                let cmd = to_string(skipbl(cp.text()));
                if cmd.is_empty() {
                    return err(format!("no command specified for {}", cp.cmdc));
                }
                let kind = match cp.cmdc {
                    '<' => PipeKind::From,
                    '|' => PipeKind::Through,
                    _ => PipeKind::To,
                };
                let a = self.addr;
                self.dot = a;
                self.intents.push(Intent::Pipe { kind, cmd, q0: a.q0, q1: a.q1 });
                Ok(())
            }
            c => err(format!("unknown command {c} in cmdexec")),
        }
    }

    // ---- commands ---------------------------------------------------------

    fn append(&mut self, cp: &Cmd, p: usize) -> Result<()> {
        let text = cp.text();
        if !text.is_empty() {
            self.elog.insert(p, text)?;
        }
        self.dot = Rg::new(p, p);
        Ok(())
    }

    fn nl_cmd(&mut self, cp: &Cmd) -> Result<()> {
        if cp.addr.is_none() {
            // first put it on newline boundaries
            let a = self.dot;
            let mut addr = self.lineaddr(0, a, -1)?;
            let a1 = self.lineaddr(0, a, 1)?;
            addr.q1 = a1.q1;
            if addr == self.dot {
                addr = self.lineaddr(1, a, 1)?;
            }
            self.addr = addr;
        }
        self.dot = self.addr;
        Ok(())
    }

    fn p_cmd(&mut self) -> Result<()> {
        let a = self.addr;
        let q1 = a.q1.min(self.len());
        if q1 > a.q0 {
            self.out.push(Printed::Text(self.t.read(a.q0, q1).into_iter().collect()));
        }
        self.dot = a;
        Ok(())
    }

    fn nlcount(&self, mut q0: usize, q1: usize) -> (usize, usize) {
        let mut nl = 0;
        let mut start = q0;
        while q0 < q1 {
            if self.t.char_at(q0) == '\n' {
                start = q0 + 1;
                nl += 1;
            }
            q0 += 1;
        }
        (nl, q0 - start)
    }

    fn eq_cmd(&mut self, cp: &Cmd) -> Result<()> {
        let text = cp.text();
        let mode = match text.len() {
            0 => 0,
            1 if text[0] == '#' => 1,
            1 if text[0] == '+' => 2,
            _ => return err("newline expected"),
        };
        let mut line = String::new();
        if let Some(name) = self.name {
            line.push_str(name);
            line.push(':');
        }
        let a = self.addr;
        match mode {
            1 => {
                line.push_str(&format!("#{}", a.q0));
                if a.q1 != a.q0 {
                    line.push_str(&format!(",#{}", a.q1));
                }
            }
            0 => {
                let l1 = 1 + self.nlcount(0, a.q0).0;
                let mut l2 = l1 + self.nlcount(a.q0, a.q1).0;
                // check if addr ends with '\n'
                if a.q1 > 0 && a.q1 > a.q0 && self.t.char_at(a.q1 - 1) == '\n' {
                    l2 -= 1;
                }
                line.push_str(&format!("{l1}"));
                if l2 != l1 {
                    line.push_str(&format!(",{l2}"));
                }
            }
            _ => {
                let (n1, r1) = self.nlcount(0, a.q0);
                let l1 = 1 + n1;
                let (n2, mut r2) = self.nlcount(a.q0, a.q1);
                let l2 = l1 + n2;
                if l2 == l1 {
                    r2 += r1;
                }
                line.push_str(&format!("{l1}+#{r1}"));
                if l2 != l1 {
                    line.push_str(&format!(",{l2}+#{r2}"));
                }
            }
        }
        line.push('\n');
        self.out.push(Printed::Address(line));
        Ok(())
    }

    fn s_cmd(&mut self, cp: &Cmd) -> Result<()> {
        let a = self.addr;
        let mut n = cp.num;
        let mut op: i64 = -1;
        let re = cp.re.clone().unwrap_or_default();
        let re = self.compile(&re, "bad regexp in s command")?.clone();
        let mut sets: Vec<Rangeset> = Vec::new();
        let mut p1 = a.q0;
        while p1 <= a.q1 {
            let Some(sel) = re.execute(self.t, p1, a.q1) else { break };
            let m = Rg::from_set(&sel);
            if m.q0 == m.q1 {
                // empty match?
                if m.q0 as i64 == op {
                    p1 += 1;
                    continue;
                }
                p1 = m.q1 + 1;
            } else {
                p1 = m.q1;
            }
            op = m.q1 as i64;
            n -= 1;
            if n > 0 {
                continue;
            }
            sets.push(sel);
        }
        let rhs = cp.text();
        let mut didsub = false;
        for sel in &sets {
            let mut buf: Vec<char> = Vec::new();
            let mut i = 0;
            while i < rhs.len() {
                let c = rhs[i];
                if c == '\\' && i < rhs.len() - 1 {
                    i += 1;
                    let c = rhs[i];
                    if ('1'..='9').contains(&c) {
                        let j = c as usize - '0' as usize;
                        let r = sel[j];
                        if r.q1 > r.q0 && r.q0 >= 0 {
                            buf.extend(self.t.read(r.q0 as usize, (r.q1 as usize).min(self.len())));
                        }
                    } else {
                        buf.push(c);
                    }
                } else if c != '&' {
                    buf.push(c);
                } else {
                    let r = Rg::from_set(sel);
                    buf.extend(self.t.read(r.q0, r.q1));
                }
                i += 1;
            }
            let m = Rg::from_set(sel);
            self.elog.replace(self.t, m.q0, m.q1, &buf)?;
            didsub = true;
            if !cp.flag {
                break;
            }
        }
        if !didsub && self.nest == 0 {
            return err("no substitution");
        }
        self.dot = a;
        Ok(())
    }

    fn m_cmd(&mut self, cp: &Cmd) -> Result<()> {
        let Arg::Addr(mt) = &cp.arg else { return err("bad address") };
        let dot = self.dot;
        let addr2 = self.cmdaddress(mt, dot, 0)?;
        let a = self.addr;
        if cp.cmdc == 'm' {
            // move
            if a.q1 <= addr2.q0 {
                self.elog.delete(a.q0, a.q1)?;
                self.copy(a, addr2)?;
            } else if a.q0 >= addr2.q1 {
                self.copy(a, addr2)?;
                self.elog.delete(a.q0, a.q1)?;
            } else if a.q0 == addr2.q0 && a.q1 == addr2.q1 {
                // move to self; no-op
            } else {
                return err("move overlaps itself");
            }
        } else {
            self.copy(a, addr2)?;
        }
        Ok(())
    }

    fn copy(&mut self, a: Rg, addr2: Rg) -> Result<()> {
        if a.q1 > a.q0 {
            let text = self.t.read(a.q0, a.q1);
            self.elog.insert(addr2.q1, &text)?;
        }
        Ok(())
    }

    fn g_cmd(&mut self, cp: &Cmd) -> Result<()> {
        let a = self.addr;
        let re = cp.re.clone().unwrap_or_default();
        let re = self.compile(&re, "bad regexp in g command")?.clone();
        let matched = re.execute(self.t, a.q0, a.q1).is_some();
        if matched ^ (cp.cmdc == 'v') {
            self.dot = a;
            if let Arg::Cmd(sub) = &cp.arg {
                return self.cmdexec(sub);
            }
        }
        Ok(())
    }

    fn loopcmd(&mut self, cp: &Cmd, ranges: &[Rg]) -> Result<()> {
        let Arg::Cmd(sub) = &cp.arg else { return Ok(()) };
        for r in ranges {
            self.dot = *r;
            self.cmdexec(sub)?;
        }
        Ok(())
    }

    fn looper(&mut self, cp: &Cmd, xy: bool) -> Result<()> {
        let r = self.addr;
        let mut op: i64 = if xy { -1 } else { r.q0 as i64 };
        self.nest += 1;
        let pat = cp.re.clone().unwrap_or_default();
        let re = self.compile(&pat, &format!("bad regexp in {} command", cp.cmdc))?.clone();
        let mut ranges: Vec<Rg> = Vec::new();
        let mut p = r.q0;
        let mut last_q1: i64 = -1;
        while p <= r.q1 {
            let tr;
            match re.execute(self.t, p, r.q1) {
                None => {
                    // no match, but y should still run
                    if xy || op > r.q1 as i64 {
                        break;
                    }
                    tr = Rg::new(op as usize, r.q1);
                    p = r.q1 + 1; // exit next loop
                }
                Some(sel) => {
                    let m = Rg::from_set(&sel);
                    if m.q0 == m.q1 {
                        // empty match?
                        if m.q0 as i64 == op {
                            p += 1;
                            continue;
                        }
                        p = m.q1 + 1;
                    } else {
                        p = m.q1;
                    }
                    tr = if xy { m } else { Rg::new(op as usize, m.q0) };
                    last_q1 = m.q1 as i64;
                }
            }
            op = last_q1;
            ranges.push(tr);
        }
        self.loopcmd(cp, &ranges)?;
        self.nest -= 1;
        Ok(())
    }

    fn linelooper(&mut self, cp: &Cmd) -> Result<()> {
        self.nest += 1;
        let r = self.addr;
        let mut ranges: Vec<Rg> = Vec::new();
        let mut a3 = Rg::new(r.q0, r.q0);
        let mut linesel = self.lineaddr(0, a3, 1)?;
        let mut p = r.q0;
        while p < r.q1 {
            a3.q0 = a3.q1;
            if p != r.q0 || linesel.q1 == p {
                linesel = self.lineaddr(1, a3, 1)?;
            }
            if linesel.q0 >= r.q1 {
                break;
            }
            if linesel.q1 >= r.q1 {
                linesel.q1 = r.q1;
            }
            if linesel.q1 > linesel.q0 && linesel.q0 >= a3.q1 && linesel.q1 > a3.q1 {
                a3 = linesel;
                ranges.push(linesel);
                p = a3.q1;
                continue;
            }
            break;
        }
        self.loopcmd(cp, &ranges)?;
        self.nest -= 1;
        Ok(())
    }

    fn cmdname(&self, s: &[char]) -> Option<String> {
        if s.is_empty() {
            return self.name.map(|n| n.to_string());
        }
        let s = skipbl(s);
        if s.is_empty() {
            return None;
        }
        Some(to_string(s))
    }

    // ---- addresses --------------------------------------------------------

    fn nextmatch(&mut self, re: &[char], mut p: usize, sign: i32) -> Result<Rg> {
        let re = self.compile(re, "bad regexp in command address")?.clone();
        if sign >= 0 {
            let Some(mut sel) = re.execute(self.t, p, INFINITY) else {
                return err("no match for regexp");
            };
            if sel[0].q0 == sel[0].q1 && sel[0].q0 == p as i64 {
                p += 1;
                if p > self.len() {
                    p = 0;
                }
                match re.execute(self.t, p, INFINITY) {
                    Some(s) => sel = s,
                    None => return err("address"),
                }
            }
            Ok(Rg::from_set(&sel))
        } else {
            let Some(mut sel) = re.bexecute(self.t, p) else {
                return err("no match for regexp");
            };
            if sel[0].q0 == sel[0].q1 && sel[0].q1 == p as i64 {
                let np = if p == 0 { self.len() } else { p - 1 };
                match re.bexecute(self.t, np) {
                    Some(s) => sel = s,
                    None => return err("address"),
                }
            }
            Ok(Rg::from_set(&sel))
        }
    }

    fn cmdaddress(&mut self, ap: &Addr, mut a: Rg, mut sign: i32) -> Result<Rg> {
        let mut ap = Some(ap);
        while let Some(cur) = ap {
            match cur.typ {
                'l' | '#' => {
                    a = if cur.typ == '#' {
                        self.charaddr(cur.num, a, sign)?
                    } else {
                        self.lineaddr(cur.num, a, sign)?
                    };
                }
                '.' => a = self.dot,
                '$' => a = Rg::new(self.len(), self.len()),
                '\'' => return err("can't handle '"),
                '?' | '/' => {
                    if cur.typ == '?' {
                        sign = -sign;
                        if sign == 0 {
                            sign = -1;
                        }
                    }
                    let re = cur.re.clone().unwrap_or_default();
                    let p = if sign >= 0 { a.q1 } else { a.q0 };
                    a = self.nextmatch(&re, p, sign)?;
                }
                '"' => return err("file addresses are not supported"),
                '*' => return Ok(Rg::new(0, self.len())),
                ',' | ';' => {
                    let a1 = match &cur.left {
                        Some(l) => self.cmdaddress(l, a, 0)?,
                        None => Rg::new(0, 0),
                    };
                    if cur.typ == ';' {
                        a = a1;
                        self.dot = a1;
                    }
                    let a2 = match &cur.next {
                        Some(n) => self.cmdaddress(n, a, 0)?,
                        None => Rg::new(self.len(), self.len()),
                    };
                    let r = Rg::new(a1.q0, a2.q1);
                    if r.q1 < r.q0 {
                        return err("addresses out of order");
                    }
                    return Ok(r);
                }
                '+' | '-' => {
                    sign = if cur.typ == '-' { -1 } else { 1 };
                    let next_is_op = matches!(cur.next.as_deref(), None | Some(Addr { typ: '+', .. }) | Some(Addr { typ: '-', .. }));
                    if next_is_op {
                        a = self.lineaddr(1, a, sign)?;
                    }
                }
                _ => return err("cmdaddress"),
            }
            ap = cur.next.as_deref();
        }
        Ok(a)
    }

    fn charaddr(&self, l: i64, a: Rg, sign: i32) -> Result<Rg> {
        let (mut q0, mut q1) = (a.q0 as i64, a.q1 as i64);
        if sign == 0 {
            q0 = l;
            q1 = l;
        } else if sign < 0 {
            q0 -= l;
            q1 = q0;
        } else {
            q1 += l;
            q0 = q1;
        }
        if q0 < 0 || q1 > self.len() as i64 {
            return err("address out of range");
        }
        Ok(Rg::new(q0 as usize, q1 as usize))
    }

    fn lineaddr(&self, l: i64, a: Rg, sign: i32) -> Result<Rg> {
        let nc = self.len();
        let ch = |p: usize| self.t.char_at(p);
        let mut r = Rg::default();
        if sign >= 0 {
            let mut p;
            if l == 0 {
                if sign == 0 || a.q1 == 0 {
                    return Ok(Rg::new(0, 0));
                }
                r.q0 = a.q1;
                p = a.q1 - 1;
            } else {
                let mut n: i64;
                if sign == 0 || a.q1 == 0 {
                    p = 0;
                    n = 1;
                } else {
                    p = a.q1 - 1;
                    n = i64::from(ch(p) == '\n');
                    p += 1;
                }
                while n < l {
                    if p >= nc {
                        return err("address out of range");
                    }
                    if ch(p) == '\n' {
                        n += 1;
                    }
                    p += 1;
                }
                r.q0 = p;
            }
            while p < nc {
                let c = ch(p);
                p += 1;
                if c == '\n' {
                    break;
                }
            }
            r.q1 = p;
        } else {
            let mut p = a.q0;
            if l == 0 {
                r.q1 = a.q0;
            } else {
                let mut n: i64 = 0;
                while n < l {
                    // always runs once
                    if p == 0 {
                        n += 1;
                        if n != l {
                            return err("address out of range");
                        }
                    } else {
                        let c = ch(p - 1);
                        if c != '\n' {
                            p -= 1;
                        } else {
                            n += 1;
                            if n != l {
                                p -= 1;
                            }
                        }
                    }
                }
                r.q1 = p;
                if p > 0 {
                    p -= 1;
                }
            }
            while p > 0 && ch(p - 1) != '\n' {
                // lines start after a newline
                p -= 1;
            }
            r.q0 = p;
        }
        Ok(r)
    }
}
