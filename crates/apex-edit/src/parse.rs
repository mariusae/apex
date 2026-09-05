//! The Edit command parser: a port of plan9port `src/cmd/acme/edit.c`.
//!
//! A program is one or more commands, each on its own line, of the form
//! `[address] command [arguments]`. The parser builds a [`Cmd`] tree; the
//! empty regular expression `//` refers to the last one seen, which persists
//! across programs in [`Edit`](crate::Edit).

use crate::{Error, Result};

/// Default address of a command when none is given.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DefAddr {
    /// The command takes no address.
    No,
    /// Dot.
    Dot,
    /// The whole file (`0,$`).
    All,
}

/// One row of acme's command table.
#[derive(Clone, Copy, Debug)]
pub struct CmdTab {
    pub cmdc: char,
    /// takes a textual argument?
    pub text: bool,
    /// takes a regular expression?
    pub regexp: bool,
    /// takes an address (m or t)?
    pub addr: bool,
    /// default command (for x, y, g, v, X, Y)
    pub defcmd: Option<char>,
    pub defaddr: DefAddr,
    /// takes a count, e.g. `s2///`: 1 = unsigned, 2 = signed
    pub count: u8,
    /// takes text terminated by one of these
    pub token: Option<&'static str>,
}

const LINEX: &str = "\n";
const WORDX: &str = " \t\n";

macro_rules! row {
    ($c:expr, $text:expr, $re:expr, $addr:expr, $defcmd:expr, $defaddr:expr, $count:expr, $token:expr) => {
        CmdTab {
            cmdc: $c,
            text: $text,
            regexp: $re,
            addr: $addr,
            defcmd: $defcmd,
            defaddr: $defaddr,
            count: $count,
            token: $token,
        }
    };
}

/// acme's `cmdtab`, verbatim.
pub const CMDTAB: &[CmdTab] = &[
    //   cmdc  text   regexp addr   defcmd     defaddr       count token
    row!('\n', false, false, false, None, DefAddr::Dot, 0, None),
    row!('a', true, false, false, None, DefAddr::Dot, 0, None),
    row!('b', false, false, false, None, DefAddr::No, 0, Some(LINEX)),
    row!('c', true, false, false, None, DefAddr::Dot, 0, None),
    row!('d', false, false, false, None, DefAddr::Dot, 0, None),
    row!('e', false, false, false, None, DefAddr::No, 0, Some(WORDX)),
    row!('f', false, false, false, None, DefAddr::No, 0, Some(WORDX)),
    row!('g', false, true, false, Some('p'), DefAddr::Dot, 0, None),
    row!('i', true, false, false, None, DefAddr::Dot, 0, None),
    row!('m', false, false, true, None, DefAddr::Dot, 0, None),
    row!('p', false, false, false, None, DefAddr::Dot, 0, None),
    row!('r', false, false, false, None, DefAddr::Dot, 0, Some(WORDX)),
    row!('s', false, true, false, None, DefAddr::Dot, 1, None),
    row!('t', false, false, true, None, DefAddr::Dot, 0, None),
    row!('u', false, false, false, None, DefAddr::No, 2, None),
    row!('v', false, true, false, Some('p'), DefAddr::Dot, 0, None),
    row!('w', false, false, false, None, DefAddr::All, 0, Some(WORDX)),
    row!('x', false, true, false, Some('p'), DefAddr::Dot, 0, None),
    row!('y', false, true, false, Some('p'), DefAddr::Dot, 0, None),
    row!('=', false, false, false, None, DefAddr::Dot, 0, Some(LINEX)),
    row!('B', false, false, false, None, DefAddr::No, 0, Some(LINEX)),
    row!('D', false, false, false, None, DefAddr::No, 0, Some(LINEX)),
    row!('X', false, true, false, Some('f'), DefAddr::No, 0, None),
    row!('Y', false, true, false, Some('f'), DefAddr::No, 0, None),
    row!('<', false, false, false, None, DefAddr::Dot, 0, Some(LINEX)),
    row!('|', false, false, false, None, DefAddr::Dot, 0, Some(LINEX)),
    row!('>', false, false, false, None, DefAddr::Dot, 0, Some(LINEX)),
];

pub fn lookup(c: char) -> Option<&'static CmdTab> {
    CMDTAB.iter().find(|t| t.cmdc == c)
}

/// An address. `typ` is one of `# l / ? . $ + - , ; " ' *` (`*` is the
/// internal "whole file" address).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Addr {
    pub typ: char,
    pub re: Option<Vec<char>>,
    /// left side of `,` and `;`
    pub left: Option<Box<Addr>>,
    pub num: i64,
    /// the next simple address, or the right side of `,` and `;`
    pub next: Option<Box<Addr>>,
}

impl Addr {
    pub fn simple(typ: char) -> Addr {
        Addr { typ, ..Default::default() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arg {
    None,
    /// target of x, g, {, etc.
    Cmd(Box<Cmd>),
    /// text of a, c, i; rhs of s; token of b, e, f, w, =, ...
    Text(Vec<char>),
    /// address for m, t
    Addr(Box<Addr>),
}

/// A parsed command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cmd {
    pub addr: Option<Box<Addr>>,
    /// regular expression for e.g. `x`
    pub re: Option<Vec<char>>,
    pub arg: Arg,
    /// next command in a `{}` block
    pub next: Option<Box<Cmd>>,
    /// count, e.g. `s2///`
    pub num: i64,
    /// the `g` flag of `s`
    pub flag: bool,
    pub cmdc: char,
}

impl Cmd {
    fn new(cmdc: char) -> Cmd {
        Cmd { addr: None, re: None, arg: Arg::None, next: None, num: 0, flag: false, cmdc }
    }

    pub fn text(&self) -> &[char] {
        match &self.arg {
            Arg::Text(t) => t,
            _ => &[],
        }
    }
}

pub struct Parser<'a> {
    s: Vec<char>,
    pos: usize,
    lastpat: &'a mut Vec<char>,
}

fn okdelim(c: char) -> Result<()> {
    if c == '\\' || c.is_ascii_alphanumeric() {
        return Err(Error(format!("bad delimiter {c}")));
    }
    Ok(())
}

impl<'a> Parser<'a> {
    /// `lastpat` is the last regular expression seen, shared across programs.
    pub fn new(program: &str, lastpat: &'a mut Vec<char>) -> Parser<'a> {
        let mut s: Vec<char> = program.chars().collect();
        if s.last() != Some(&'\n') {
            s.push('\n');
        }
        Parser { s, pos: 0, lastpat }
    }

    fn getch(&mut self) -> Option<char> {
        let c = self.s.get(self.pos).copied();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn nextc(&self) -> Option<char> {
        self.s.get(self.pos).copied()
    }

    fn ungetch(&mut self) {
        self.pos = self.pos.checked_sub(1).expect("ungetch");
    }

    /// A number; no number defaults to 1. `signok > 1` allows a leading `-`.
    fn getnum(&mut self, signok: u8) -> i64 {
        let mut sign = 1;
        if signok > 1 && self.nextc() == Some('-') {
            sign = -1;
            self.getch();
        }
        match self.nextc() {
            Some(c) if c.is_ascii_digit() => {}
            _ => return sign,
        }
        let mut n: i64 = 0;
        while let Some(c) = self.nextc() {
            if let Some(d) = c.to_digit(10) {
                self.getch();
                n = n.saturating_mul(10).saturating_add(d as i64);
            } else {
                break;
            }
        }
        sign * n
    }

    /// Skip blanks; returns the next char without consuming it.
    fn cmdskipbl(&mut self) -> Option<char> {
        loop {
            match self.getch() {
                Some(' ') | Some('\t') => continue,
                Some(c) => {
                    self.ungetch();
                    return Some(c);
                }
                None => return None,
            }
        }
    }

    fn atnl(&mut self) -> Result<()> {
        self.cmdskipbl();
        match self.getch() {
            Some('\n') => Ok(()),
            Some(c) => Err(Error(format!("newline expected (saw {c})"))),
            None => Err(Error("newline expected (saw EOF)".into())),
        }
    }

    /// The right-hand side of `s`, or delimited text for a/c/i.
    fn getrhs(&mut self, delim: char, cmd: char) -> Result<Vec<char>> {
        let mut s = Vec::new();
        loop {
            let Some(c) = self.getch() else { return Ok(s) };
            if c == delim || c == '\n' {
                self.ungetch(); // let the caller see the delimiter or newline
                return Ok(s);
            }
            let mut c = c;
            if c == '\\' {
                let Some(e) = self.getch() else {
                    return Err(Error("bad right hand side".into()));
                };
                c = e;
                if c == '\n' {
                    self.ungetch();
                    c = '\\';
                } else if c == 'n' {
                    c = '\n';
                } else if c != delim && (cmd == 's' || c != '\\') {
                    s.push('\\'); // s does its own
                }
            }
            s.push(c);
        }
    }

    fn collecttoken(&mut self, end: &str) -> Result<Vec<char>> {
        let mut s = Vec::new();
        while let Some(c) = self.nextc() {
            if c == ' ' || c == '\t' {
                s.push(c); // blanks significant for getname()
                self.getch();
            } else {
                break;
            }
        }
        let mut last = None;
        while let Some(c) = self.getch() {
            if end.contains(c) {
                last = Some(c);
                break;
            }
            s.push(c);
        }
        if last != Some('\n') {
            self.atnl()?;
        }
        Ok(s)
    }

    fn collecttext(&mut self) -> Result<Vec<char>> {
        let mut s = Vec::new();
        if self.cmdskipbl() == Some('\n') {
            self.getch();
            loop {
                let begline = s.len();
                let mut c = None;
                while let Some(ch) = self.getch() {
                    if ch == '\n' {
                        c = Some(ch);
                        break;
                    }
                    s.push(ch);
                }
                s.push('\n');
                if c.is_none() {
                    return Ok(s);
                }
                if s.get(begline) == Some(&'.') && s.get(begline + 1) == Some(&'\n') {
                    s.truncate(s.len() - 2);
                    return Ok(s);
                }
            }
        }
        let delim = self.getch().unwrap_or('\n');
        okdelim(delim)?;
        s = self.getrhs(delim, 'a')?;
        if self.nextc() == Some(delim) {
            self.getch();
        }
        self.atnl()?;
        Ok(s)
    }

    /// Parse one command; `None` at end of input or at a `}`.
    pub fn parsecmd(&mut self, nest: u32) -> Result<Option<Cmd>> {
        let addr = self.compoundaddr()?;
        if self.cmdskipbl().is_none() {
            return Ok(None);
        }
        let Some(c) = self.getch() else { return Ok(None) };
        let mut cmd = Cmd::new(c);
        cmd.addr = addr.map(Box::new);
        if cmd.cmdc == 'c' && self.nextc() == Some('d') {
            // sleazy two-character case; acme has no cd
            return Err(Error("unknown command cd".into()));
        }
        if let Some(ct) = lookup(cmd.cmdc) {
            if cmd.cmdc == '\n' {
                return Ok(Some(cmd)); // let nl_cmd work it all out
            }
            if ct.defaddr == DefAddr::No && cmd.addr.is_some() {
                return Err(Error("command takes no address".into()));
            }
            if ct.count > 0 {
                cmd.num = self.getnum(ct.count);
            }
            if ct.regexp {
                // x without pattern -> .*\n, indicated by cmd.re == None
                // X without pattern is all files
                let n = self.nextc();
                if (ct.cmdc != 'x' && ct.cmdc != 'X')
                    || (n != Some(' ') && n != Some('\t') && n != Some('\n'))
                {
                    self.cmdskipbl();
                    let c = self.getch();
                    let Some(c) = c else { return Err(Error("no address".into())) };
                    if c == '\n' {
                        return Err(Error("no address".into()));
                    }
                    okdelim(c)?;
                    cmd.re = Some(self.getregexp(c)?);
                    if ct.cmdc == 's' {
                        cmd.arg = Arg::Text(self.getrhs(c, 's')?);
                        if self.nextc() == Some(c) {
                            self.getch();
                            if self.nextc() == Some('g') {
                                self.getch();
                                cmd.flag = true;
                            }
                        }
                    }
                }
            }
            if ct.addr {
                match self.simpleaddr()? {
                    Some(a) => cmd.arg = Arg::Addr(Box::new(a)),
                    None => return Err(Error("bad address".into())),
                }
            }
            if let Some(defcmd) = ct.defcmd {
                if self.cmdskipbl() == Some('\n') {
                    self.getch();
                    cmd.arg = Arg::Cmd(Box::new(Cmd::new(defcmd)));
                } else {
                    match self.parsecmd(nest)? {
                        Some(sub) => cmd.arg = Arg::Cmd(Box::new(sub)),
                        None => return Err(Error("defcmd".into())),
                    }
                }
            } else if ct.text {
                cmd.arg = Arg::Text(self.collecttext()?);
            } else if let Some(token) = ct.token {
                cmd.arg = Arg::Text(self.collecttoken(token)?);
            } else {
                self.atnl()?;
            }
        } else {
            match cmd.cmdc {
                '{' => {
                    let mut subs: Vec<Cmd> = Vec::new();
                    loop {
                        if self.cmdskipbl() == Some('\n') {
                            self.getch();
                        }
                        match self.parsecmd(nest + 1)? {
                            Some(sub) => subs.push(sub),
                            None => break,
                        }
                    }
                    let mut chain: Option<Box<Cmd>> = None;
                    for mut sub in subs.into_iter().rev() {
                        sub.next = chain;
                        chain = Some(Box::new(sub));
                    }
                    if let Some(first) = chain {
                        cmd.arg = Arg::Cmd(first);
                    }
                }
                '}' => {
                    self.atnl()?;
                    if nest == 0 {
                        return Err(Error("right brace with no left brace".into()));
                    }
                    return Ok(None);
                }
                c => return Err(Error(format!("unknown command {c}"))),
            }
        }
        Ok(Some(cmd))
    }

    /// A delimited regular expression; empty means the last one.
    fn getregexp(&mut self, delim: char) -> Result<Vec<char>> {
        let mut buf = Vec::new();
        let mut last = None;
        loop {
            let Some(c) = self.getch() else { break };
            let mut c = c;
            if c == '\\' {
                if self.nextc() == Some(delim) {
                    c = self.getch().unwrap();
                } else if self.nextc() == Some('\\') {
                    buf.push(c);
                    c = self.getch().unwrap();
                }
            } else if c == delim || c == '\n' {
                last = Some(c);
                break;
            }
            buf.push(c);
        }
        if let Some(c) = last {
            if c != delim {
                self.ungetch();
            }
        }
        if !buf.is_empty() {
            *self.lastpat = buf;
        }
        if self.lastpat.is_empty() {
            return Err(Error("no regular expression defined".into()));
        }
        Ok(self.lastpat.clone())
    }

    fn simpleaddr(&mut self) -> Result<Option<Addr>> {
        let mut addr = Addr::default();
        match self.cmdskipbl() {
            Some('#') => {
                addr.typ = self.getch().unwrap();
                addr.num = self.getnum(1);
            }
            Some('0'..='9') => {
                addr.num = self.getnum(1);
                addr.typ = 'l';
            }
            Some('/') | Some('?') | Some('"') => {
                let d = self.getch().unwrap();
                addr.typ = d;
                addr.re = Some(self.getregexp(d)?);
            }
            Some('.') | Some('$') | Some('+') | Some('-') | Some('\'') => {
                addr.typ = self.getch().unwrap();
            }
            _ => return Ok(None),
        }
        if let Some(next) = self.simpleaddr()? {
            let insert_plus = match next.typ {
                '.' | '$' | '\'' => {
                    if addr.typ != '"' {
                        return Err(Error("bad address syntax".into()));
                    }
                    false
                }
                '"' => return Err(Error("bad address syntax".into())),
                'l' | '#' => addr.typ != '"' && addr.typ != '+' && addr.typ != '-',
                '/' | '?' => addr.typ != '+' && addr.typ != '-',
                '+' | '-' => false,
                _ => return Err(Error("simpleaddr".into())),
            };
            if insert_plus {
                // insert the missing '+'
                let mut plus = Addr::simple('+');
                plus.next = Some(Box::new(next));
                addr.next = Some(Box::new(plus));
            } else {
                addr.next = Some(Box::new(next));
            }
        }
        Ok(Some(addr))
    }

    fn compoundaddr(&mut self) -> Result<Option<Addr>> {
        let left = self.simpleaddr()?;
        let typ = match self.cmdskipbl() {
            Some(c) if c == ',' || c == ';' => c,
            _ => return Ok(left),
        };
        self.getch();
        let next = self.compoundaddr()?;
        if let Some(n) = &next {
            if (n.typ == ',' || n.typ == ';') && n.left.is_none() {
                return Err(Error("bad address syntax".into()));
            }
        }
        Ok(Some(Addr {
            typ,
            re: None,
            left: left.map(Box::new),
            num: 0,
            next: next.map(Box::new),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(p: &str) -> Vec<Cmd> {
        let mut lp = Vec::new();
        let mut parser = Parser::new(p, &mut lp);
        let mut v = Vec::new();
        while let Some(c) = parser.parsecmd(0).unwrap() {
            v.push(c);
        }
        v
    }

    #[test]
    fn simple_commands() {
        let v = parse("a/hello/");
        assert_eq!(v[0].cmdc, 'a');
        assert_eq!(v[0].text(), "hello".chars().collect::<Vec<_>>());
        let v = parse("s/a/b/g");
        assert!(v[0].flag);
        assert_eq!(v[0].num, 1);
        assert_eq!(v[0].re.as_deref(), Some(&['a'][..]));
        let v = parse("s2/a/\\1&/");
        assert_eq!(v[0].num, 2);
        assert_eq!(v[0].text(), "\\1&".chars().collect::<Vec<_>>());
    }

    #[test]
    fn addresses() {
        let v = parse("1,$d");
        let a = v[0].addr.as_ref().unwrap();
        assert_eq!(a.typ, ',');
        assert_eq!(a.left.as_ref().unwrap().typ, 'l');
        assert_eq!(a.next.as_ref().unwrap().typ, '$');
        // juxtaposition inserts a +
        let v = parse("./x/d");
        let a = v[0].addr.as_ref().unwrap();
        assert_eq!(a.typ, '.');
        assert_eq!(a.next.as_ref().unwrap().typ, '+');
        assert_eq!(a.next.as_ref().unwrap().next.as_ref().unwrap().typ, '/');
        let v = parse("-3p");
        let a = v[0].addr.as_ref().unwrap();
        assert_eq!(a.typ, '-');
        assert_eq!(a.next.as_ref().unwrap().num, 3);
    }

    #[test]
    fn loops_and_blocks() {
        let v = parse("x/a/ c/b/");
        assert_eq!(v[0].cmdc, 'x');
        match &v[0].arg {
            Arg::Cmd(c) => assert_eq!(c.cmdc, 'c'),
            _ => panic!(),
        }
        let v = parse("x\n");
        assert!(v[0].re.is_none());
        match &v[0].arg {
            Arg::Cmd(c) => assert_eq!(c.cmdc, 'p'),
            _ => panic!(),
        }
        let v = parse("{\na/x/\nd\n}\n");
        match &v[0].arg {
            Arg::Cmd(c) => {
                assert_eq!(c.cmdc, 'a');
                assert_eq!(c.next.as_ref().unwrap().cmdc, 'd');
            }
            _ => panic!(),
        }
    }

    #[test]
    fn multiline_text() {
        let v = parse("a\nline1\nline2\n.\n");
        assert_eq!(v[0].text().iter().collect::<String>(), "line1\nline2\n");
    }

    #[test]
    fn errors() {
        let mut lp = Vec::new();
        assert_eq!(Parser::new("1u", &mut lp).parsecmd(0).unwrap_err().0, "command takes no address");
        assert_eq!(Parser::new("q", &mut lp).parsecmd(0).unwrap_err().0, "unknown command q");
        assert_eq!(Parser::new("axhellox", &mut lp).parsecmd(0).unwrap_err().0, "bad delimiter x");
        assert_eq!(Parser::new("}", &mut lp).parsecmd(0).unwrap_err().0, "right brace with no left brace");
        assert_eq!(Parser::new("x//p", &mut lp).parsecmd(0).unwrap_err().0, "no regular expression defined");
    }
}
