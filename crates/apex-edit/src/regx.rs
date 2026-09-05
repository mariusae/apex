//! Plan 9 regular expressions as acme implements them: a port of
//! plan9port `src/cmd/acme/regx.c`.
//!
//! Matching is *leftmost-longest*. A pattern compiles to two programs, one
//! for searching forward and one backward (the catenation order reversed),
//! run by a Thompson-style simulation over rune positions. Searches may wrap
//! around the end of the text, as acme's address searches do.
//!
//! Departures from the C: the thread lists, instruction table and parser
//! stacks are dynamic (the C code fails with "regexp list overflow" or
//! "expression too long" beyond fixed limits).

use crate::Text;

/// Number of subexpression ranges tracked (`\0`..`\9`).
pub const NRANGE: usize = 10;

/// `eof` value meaning "no limit; wrap around the end of the text".
pub const INFINITY: usize = usize::MAX;

/// A rune range `q0..q1`. Signed because the machine uses negative values
/// internally while searching backwards.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Range {
    pub q0: i64,
    pub q1: i64,
}

/// The whole match (`[0]`) and up to nine subexpressions.
pub type Rangeset = [Range; NRANGE];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Char(char),
    Any,
    Bol,
    Eol,
    Class(usize),
    NClass(usize),
    Or,
    Nop,
    Lbra(i32),
    Rbra(i32),
    End,
}

#[derive(Clone, Copy, Debug)]
struct Inst {
    op: Op,
    /// `u1.next` / `u1.left` in the C: the continuation.
    next: usize,
    /// `u.right`: the alternative of an `Or`.
    right: usize,
}

#[derive(Clone, Debug)]
enum ClassItem {
    One(char),
    Range(char, char),
}

// Operator tokens; the value is the precedence, as in the C.
const START: i32 = 0;
const RBRA: i32 = 1;
const LBRA: i32 = 2;
const OR: i32 = 3;
const CAT: i32 = 4;
const STAR: i32 = 5;
const PLUS: i32 = 6;
const QUEST: i32 = 7;

enum Tok {
    Operator(i32),
    Operand(Op),
    End,
}

#[derive(Clone, Copy)]
struct Node {
    first: usize,
    last: usize,
}

/// A compiled plan 9 regular expression.
#[derive(Clone, Debug)]
pub struct Regex {
    prog: Vec<Inst>,
    classes: Vec<Vec<ClassItem>>,
    start: usize,
    bstart: usize,
    /// The pattern as given, in runes.
    pub source: Vec<char>,
}

struct Compiler<'a> {
    expr: &'a [char],
    pos: usize,
    prog: Vec<Inst>,
    classes: Vec<Vec<ClassItem>>,
    andstack: Vec<Node>,
    atorstack: Vec<(i32, i32)>, // (operator, subid)
    lastwasand: bool,
    cursubid: i32,
    backwards: bool,
    nbra: i32,
}

fn opname(op: i32) -> char {
    match op {
        LBRA => '(',
        OR => '|',
        STAR => '*',
        PLUS => '+',
        QUEST => '?',
        _ => '?',
    }
}

impl<'a> Compiler<'a> {
    fn newinst(&mut self, op: Op) -> usize {
        self.prog.push(Inst { op, next: usize::MAX, right: usize::MAX });
        self.prog.len() - 1
    }

    fn compile(&mut self) -> Result<usize, String> {
        self.pos = 0;
        self.nbra = 0;
        self.atorstack.clear();
        self.andstack.clear();
        self.cursubid = 0;
        self.lastwasand = false;
        // Start with a low priority operator to prime the parser.
        self.pushator(START - 1);
        loop {
            match self.lex()? {
                Tok::End => break,
                Tok::Operator(t) => self.operator(t)?,
                Tok::Operand(op) => self.operand(op)?,
            }
        }
        // Close with a low priority operator.
        self.evaluntil(START)?;
        // Force END.
        self.operand(Op::End)?;
        self.evaluntil(START)?;
        if self.nbra != 0 {
            return Err("unmatched `('".into());
        }
        let n = self.andstack.pop().ok_or("malformed regexp")?; // first and only operand
        Ok(n.first)
    }

    fn operand(&mut self, op: Op) -> Result<(), String> {
        if self.lastwasand {
            self.operator(CAT)?; // catenation is implicit
        }
        let i = self.newinst(op);
        self.pushand(i, i);
        self.lastwasand = true;
        Ok(())
    }

    fn operator(&mut self, t: i32) -> Result<(), String> {
        if t == RBRA {
            self.nbra -= 1;
            if self.nbra < 0 {
                return Err("unmatched `)'".into());
            }
        }
        if t == LBRA {
            self.cursubid += 1; // silently ignored beyond NRANGE
            self.nbra += 1;
            if self.lastwasand {
                self.operator(CAT)?;
            }
        } else {
            self.evaluntil(t)?;
        }
        if t != RBRA {
            self.pushator(t);
        }
        self.lastwasand = false;
        if t == STAR || t == QUEST || t == PLUS || t == RBRA {
            self.lastwasand = true; // these look like operands
        }
        Ok(())
    }

    fn pushand(&mut self, first: usize, last: usize) {
        self.andstack.push(Node { first, last });
    }

    fn pushator(&mut self, t: i32) {
        let subid = if self.cursubid >= NRANGE as i32 { -1 } else { self.cursubid };
        self.atorstack.push((t, subid));
    }

    fn popand(&mut self, op: i32) -> Result<Node, String> {
        match self.andstack.pop() {
            Some(n) => Ok(n),
            None => {
                if op != 0 {
                    Err(format!("missing operand for {}", opname(op)))
                } else {
                    Err("malformed regexp".into())
                }
            }
        }
    }

    fn popator(&mut self) -> Result<(i32, i32), String> {
        self.atorstack.pop().ok_or_else(|| "operator stack underflow".to_string())
    }

    fn evaluntil(&mut self, pri: i32) -> Result<(), String> {
        while pri == RBRA || self.atorstack.last().map(|t| t.0).unwrap_or(i32::MIN) >= pri {
            let (op, subid) = self.popator()?;
            match op {
                LBRA => {
                    let op1 = self.popand(LBRA)?;
                    let inst2 = self.newinst(Op::Rbra(subid));
                    self.prog[op1.last].next = inst2;
                    let inst1 = self.newinst(Op::Lbra(subid));
                    self.prog[inst1].next = op1.first;
                    self.pushand(inst1, inst2);
                    return Ok(()); // must have been RBRA
                }
                OR => {
                    let op2 = self.popand(OR)?;
                    let op1 = self.popand(OR)?;
                    let inst2 = self.newinst(Op::Nop);
                    self.prog[op2.last].next = inst2;
                    self.prog[op1.last].next = inst2;
                    let inst1 = self.newinst(Op::Or);
                    self.prog[inst1].right = op1.first;
                    self.prog[inst1].next = op2.first;
                    self.pushand(inst1, inst2);
                }
                CAT => {
                    let mut op2 = self.popand(0)?;
                    let mut op1 = self.popand(0)?;
                    if self.backwards && self.prog[op2.first].op != Op::End {
                        std::mem::swap(&mut op1, &mut op2);
                    }
                    self.prog[op1.last].next = op2.first;
                    self.pushand(op1.first, op2.last);
                }
                STAR => {
                    let op2 = self.popand(STAR)?;
                    let inst1 = self.newinst(Op::Or);
                    self.prog[op2.last].next = inst1;
                    self.prog[inst1].right = op2.first;
                    self.pushand(inst1, inst1);
                }
                PLUS => {
                    let op2 = self.popand(PLUS)?;
                    let inst1 = self.newinst(Op::Or);
                    self.prog[op2.last].next = inst1;
                    self.prog[inst1].right = op2.first;
                    self.pushand(op2.first, inst1);
                }
                QUEST => {
                    let op2 = self.popand(QUEST)?;
                    let inst1 = self.newinst(Op::Or);
                    let inst2 = self.newinst(Op::Nop);
                    self.prog[inst1].next = inst2;
                    self.prog[inst1].right = op2.first;
                    self.prog[op2.last].next = inst2;
                    self.pushand(inst1, inst2);
                }
                _ => return Err("unknown regexp operator".into()),
            }
        }
        Ok(())
    }

    /// Skip `Nop`s in the continuation chains of the program from `start`.
    fn optimize(&mut self, start: usize) {
        let mut i = start;
        while self.prog[i].op != Op::End {
            let mut target = self.prog[i].next;
            while target != usize::MAX && self.prog[target].op == Op::Nop {
                target = self.prog[target].next;
            }
            self.prog[i].next = target;
            i += 1;
        }
    }

    fn peek(&self) -> Option<char> {
        self.expr.get(self.pos).copied()
    }

    fn lex(&mut self) -> Result<Tok, String> {
        let Some(c) = self.peek() else {
            return Ok(Tok::End);
        };
        self.pos += 1;
        Ok(match c {
            '\\' => match self.peek() {
                Some(n) => {
                    self.pos += 1;
                    Tok::Operand(Op::Char(if n == 'n' { '\n' } else { n }))
                }
                None => Tok::Operand(Op::Char('\\')),
            },
            '*' => Tok::Operator(STAR),
            '?' => Tok::Operator(QUEST),
            '+' => Tok::Operator(PLUS),
            '|' => Tok::Operator(OR),
            '.' => Tok::Operand(Op::Any),
            '(' => Tok::Operator(LBRA),
            ')' => Tok::Operator(RBRA),
            '^' => Tok::Operand(Op::Bol),
            '$' => Tok::Operand(Op::Eol),
            '[' => {
                let (idx, negate) = self.bldcclass()?;
                Tok::Operand(if negate { Op::NClass(idx) } else { Op::Class(idx) })
            }
            c => Tok::Operand(Op::Char(c)),
        })
    }

    /// Next rune of a character class; `(c, quoted)`.
    fn nextrec(&mut self) -> Result<(char, bool), String> {
        match self.peek() {
            None => Err("malformed `[]'".into()),
            Some('\\') => {
                if self.expr.get(self.pos + 1).is_none() {
                    return Err("malformed `[]'".into());
                }
                self.pos += 1;
                let c = self.expr[self.pos];
                self.pos += 1;
                if c == 'n' {
                    Ok(('\n', false))
                } else {
                    Ok((c, true))
                }
            }
            Some(c) => {
                self.pos += 1;
                Ok((c, false))
            }
        }
    }

    fn bldcclass(&mut self) -> Result<(usize, bool), String> {
        let mut items = Vec::new();
        let negate = if self.peek() == Some('^') {
            self.pos += 1;
            items.push(ClassItem::One('\n')); // don't match newline in the negated case
            true
        } else {
            false
        };
        loop {
            let (c1, q1) = self.nextrec()?;
            if c1 == ']' && !q1 {
                break;
            }
            if c1 == '-' && !q1 {
                return Err("malformed `[]'".into());
            }
            if self.peek() == Some('-') {
                self.pos += 1; // eat '-'
                let (c2, q2) = self.nextrec()?;
                if c2 == ']' && !q2 {
                    return Err("malformed `[]'".into());
                }
                items.push(ClassItem::Range(c1, c2));
            } else {
                items.push(ClassItem::One(c1));
            }
        }
        self.classes.push(items);
        Ok((self.classes.len() - 1, negate))
    }
}

#[derive(Clone, Copy)]
struct Thread {
    inst: usize,
    se: Rangeset,
}

/// Add a thread to a list unless the instruction is already there; keeps
/// the earlier start when it is.
fn addinst(l: &mut Vec<Thread>, inst: usize, se: &Rangeset) -> bool {
    for p in l.iter_mut() {
        if p.inst == inst {
            if se[0].q0 < p.se[0].q0 {
                p.se = *se;
            }
            return false;
        }
    }
    l.push(Thread { inst, se: *se });
    true
}

fn newmatch(sel: &mut Rangeset, sp: &Rangeset) {
    if sel[0].q0 < 0 || sp[0].q0 < sel[0].q0 || (sp[0].q0 == sel[0].q0 && sp[0].q1 > sel[0].q1) {
        *sel = *sp;
    }
}

fn bnewmatch(sel: &mut Rangeset, sp: &Rangeset) {
    if sel[0].q0 < 0 || sp[0].q0 > sel[0].q1 || (sp[0].q0 == sel[0].q1 && sp[0].q1 < sel[0].q0) {
        for i in 0..NRANGE {
            // note the reversal; q0 <= q1
            sel[i].q0 = sp[i].q1;
            sel[i].q1 = sp[i].q0;
        }
    }
}

const NUL: u32 = 0;

impl Regex {
    /// Compile a pattern (in runes). Errors carry acme's `regexp:` messages.
    pub fn compile(pattern: &[char]) -> Result<Regex, String> {
        let mut c = Compiler {
            expr: pattern,
            pos: 0,
            prog: Vec::new(),
            classes: Vec::new(),
            andstack: Vec::new(),
            atorstack: Vec::new(),
            lastwasand: false,
            cursubid: 0,
            backwards: false,
            nbra: 0,
        };
        let start = c.compile()?;
        c.optimize(0);
        let oprogp = c.prog.len();
        c.backwards = true;
        let bstart = c.compile()?;
        c.optimize(oprogp);
        Ok(Regex { prog: c.prog, classes: c.classes, start, bstart, source: pattern.to_vec() })
    }

    pub fn compile_str(pattern: &str) -> Result<Regex, String> {
        Self::compile(&pattern.chars().collect::<Vec<_>>())
    }

    fn classmatch(&self, k: usize, c: u32, negate: bool) -> bool {
        for item in &self.classes[k] {
            match *item {
                ClassItem::Range(a, b) => {
                    if (a as u32) <= c && c <= (b as u32) {
                        return !negate;
                    }
                }
                ClassItem::One(x) => {
                    if x as u32 == c {
                        return !negate;
                    }
                }
            }
        }
        negate
    }

    /// Search forward from `startp`, not past `eof` (`INFINITY` to search
    /// to the end and then wrap around to `startp`). Returns the leftmost
    /// longest match.
    pub fn execute(&self, t: &dyn Text, startp: usize, eof: usize) -> Option<Rangeset> {
        let nc = t.len();
        let startchar = match self.prog[self.start].op {
            Op::Char(c) => Some(c as u32),
            _ => None,
        };
        let mut sel: Rangeset = Default::default();
        sel[0].q0 = -1;
        let mut lists: [Vec<Thread>; 2] = [Vec::new(), Vec::new()];
        let mut flag = 0usize;
        let mut nnl = 0usize;
        let mut wrapped = 0u32;
        let mut p = startp;
        'outer: loop {
            let c: u32;
            loop {
                if p >= eof || p >= nc {
                    let w = wrapped;
                    wrapped += 1;
                    match w {
                        0 | 2 => {
                            c = NUL; // let the loop run one more click
                            break;
                        }
                        1 => {
                            // expired; wrap to the beginning
                            if sel[0].q0 >= 0 || eof != INFINITY {
                                break 'outer;
                            }
                            lists[0].clear();
                            lists[1].clear();
                            nnl = 0;
                            p = 0;
                            continue;
                        }
                        _ => break 'outer,
                    }
                } else {
                    if ((wrapped > 0 && p >= startp) || sel[0].q0 >= 0) && nnl == 0 {
                        break 'outer;
                    }
                    c = t.char_at(p) as u32;
                    break;
                }
            }
            // fast check for the first char
            if let Some(sc) = startchar {
                if nnl == 0 && c != sc {
                    p += 1;
                    continue;
                }
            }
            let (tl, nl) = {
                let (a, b) = lists.split_at_mut(1);
                if flag == 0 {
                    (&mut a[0], &mut b[0])
                } else {
                    (&mut b[0], &mut a[0])
                }
            };
            flag ^= 1;
            nl.clear();
            if sel[0].q0 < 0 && (wrapped == 0 || p < startp || startp == eof) {
                // add the first instruction to this list
                let mut se: Rangeset = Default::default();
                se[0].q0 = p as i64;
                addinst(tl, self.start, &se);
            }
            // execute the machine until this list is empty
            let mut i = 0;
            while i < tl.len() {
                let mut inst = tl[i].inst;
                let mut se = tl[i].se;
                loop {
                    let ins = self.prog[inst];
                    match ins.op {
                        Op::Char(ch) => {
                            if ch as u32 == c {
                                addinst(nl, ins.next, &se);
                            }
                            break;
                        }
                        Op::Lbra(sub) => {
                            if sub >= 0 {
                                se[sub as usize].q0 = p as i64;
                            }
                            inst = ins.next;
                        }
                        Op::Rbra(sub) => {
                            if sub >= 0 {
                                se[sub as usize].q1 = p as i64;
                            }
                            inst = ins.next;
                        }
                        Op::Any => {
                            if c != '\n' as u32 {
                                addinst(nl, ins.next, &se);
                            }
                            break;
                        }
                        Op::Bol => {
                            if p == 0 || t.char_at(p - 1) == '\n' {
                                inst = ins.next;
                            } else {
                                break;
                            }
                        }
                        Op::Eol => {
                            if c == '\n' as u32 {
                                inst = ins.next;
                            } else {
                                break;
                            }
                        }
                        Op::Class(k) => {
                            if self.classmatch(k, c, false) {
                                addinst(nl, ins.next, &se);
                            }
                            break;
                        }
                        Op::NClass(k) => {
                            if self.classmatch(k, c, true) {
                                addinst(nl, ins.next, &se);
                            }
                            break;
                        }
                        Op::Or => {
                            // evaluate the right choice later
                            addinst(tl, ins.right, &se);
                            // efficiency: advance and re-evaluate
                            inst = ins.next;
                        }
                        Op::Nop => inst = ins.next,
                        Op::End => {
                            se[0].q1 = p as i64;
                            newmatch(&mut sel, &se);
                            break;
                        }
                    }
                }
                i += 1;
            }
            nnl = nl.len();
            p += 1;
        }
        if sel[0].q0 >= 0 {
            Some(sel)
        } else {
            None
        }
    }

    /// Search backward from `startp`, wrapping around the beginning.
    pub fn bexecute(&self, t: &dyn Text, startp: usize) -> Option<Rangeset> {
        let nc = t.len() as i64;
        let startchar = match self.prog[self.bstart].op {
            Op::Char(c) => Some(c as u32),
            _ => None,
        };
        let mut sel: Rangeset = Default::default();
        sel[0].q0 = -1;
        let mut lists: [Vec<Thread>; 2] = [Vec::new(), Vec::new()];
        let mut flag = 0usize;
        let mut nnl = 0usize;
        let mut wrapped = 0u32;
        let startp = startp as i64;
        let mut p: i64 = startp;
        'outer: loop {
            let c: u32;
            loop {
                if p <= 0 {
                    let w = wrapped;
                    wrapped += 1;
                    match w {
                        0 | 2 => {
                            c = NUL;
                            break;
                        }
                        1 => {
                            if sel[0].q0 >= 0 {
                                break 'outer;
                            }
                            lists[0].clear();
                            lists[1].clear();
                            nnl = 0;
                            p = nc;
                            continue;
                        }
                        _ => break 'outer,
                    }
                } else {
                    if ((wrapped > 0 && p <= startp) || sel[0].q0 >= 0) && nnl == 0 {
                        break 'outer;
                    }
                    c = t.char_at((p - 1) as usize) as u32;
                    break;
                }
            }
            if let Some(sc) = startchar {
                if nnl == 0 && c != sc {
                    p -= 1;
                    continue;
                }
            }
            let (tl, nl) = {
                let (a, b) = lists.split_at_mut(1);
                if flag == 0 {
                    (&mut a[0], &mut b[0])
                } else {
                    (&mut b[0], &mut a[0])
                }
            };
            flag ^= 1;
            nl.clear();
            if sel[0].q0 < 0 && (wrapped == 0 || p > startp) {
                // the minus is so the optimisation in addinst works
                let mut se: Rangeset = Default::default();
                se[0].q0 = -p;
                addinst(tl, self.bstart, &se);
            }
            let mut i = 0;
            while i < tl.len() {
                let mut inst = tl[i].inst;
                let mut se = tl[i].se;
                loop {
                    let ins = self.prog[inst];
                    match ins.op {
                        Op::Char(ch) => {
                            if ch as u32 == c {
                                addinst(nl, ins.next, &se);
                            }
                            break;
                        }
                        Op::Lbra(sub) => {
                            if sub >= 0 {
                                se[sub as usize].q0 = p;
                            }
                            inst = ins.next;
                        }
                        Op::Rbra(sub) => {
                            if sub >= 0 {
                                se[sub as usize].q1 = p;
                            }
                            inst = ins.next;
                        }
                        Op::Any => {
                            if c != '\n' as u32 {
                                addinst(nl, ins.next, &se);
                            }
                            break;
                        }
                        Op::Bol => {
                            if c == '\n' as u32 || p == 0 {
                                inst = ins.next;
                            } else {
                                break;
                            }
                        }
                        Op::Eol => {
                            if p < nc && t.char_at(p as usize) == '\n' {
                                inst = ins.next;
                            } else {
                                break;
                            }
                        }
                        Op::Class(k) => {
                            if c > 0 && self.classmatch(k, c, false) {
                                addinst(nl, ins.next, &se);
                            }
                            break;
                        }
                        Op::NClass(k) => {
                            if c > 0 && self.classmatch(k, c, true) {
                                addinst(nl, ins.next, &se);
                            }
                            break;
                        }
                        Op::Or => {
                            addinst(tl, ins.right, &se);
                            inst = ins.next;
                        }
                        Op::Nop => inst = ins.next,
                        Op::End => {
                            se[0].q0 = -se[0].q0; // minus sign
                            se[0].q1 = p;
                            bnewmatch(&mut sel, &se);
                            break;
                        }
                    }
                }
                i += 1;
            }
            nnl = nl.len();
            p -= 1;
        }
        if sel[0].q0 >= 0 {
            Some(sel)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(re: &str, text: &str) -> Option<(usize, usize)> {
        let t: Vec<char> = text.chars().collect();
        let r = Regex::compile_str(re).unwrap();
        r.execute(&t, 0, t.len()).map(|s| (s[0].q0 as usize, s[0].q1 as usize))
    }

    #[test]
    fn leftmost_longest() {
        assert_eq!(find("a|ab", "xab"), Some((1, 3)));
        assert_eq!(find("a*", "aaa"), Some((0, 3)));
        assert_eq!(find("b*", "aaa"), Some((0, 0)));
        assert_eq!(find("(a|b)+", "xxabba"), Some((2, 6)));
    }

    #[test]
    fn classes_and_anchors() {
        assert_eq!(find("[a-c]+", "xxbca"), Some((2, 5)));
        assert_eq!(find("[^a]+", "aaxya"), Some((2, 4)));
        assert_eq!(find("^b", "a\nb"), Some((2, 3)));
        assert_eq!(find("a$", "ba\nb"), Some((1, 2)));
        assert_eq!(find("\\.", "a.b"), Some((1, 2)));
        // `]` right after `[` ends the class (an empty class), as in acme
        assert_eq!(find("[]a]", "x]a]"), None);
        assert_eq!(find("a\\nb", "a\nb"), Some((0, 3)));
        assert_eq!(find(".", "\nx"), Some((1, 2)));
    }

    #[test]
    fn subexpressions() {
        let t: Vec<char> = "hello world".chars().collect();
        let r = Regex::compile_str("(h[a-z]*) (w[a-z]*)").unwrap();
        let s = r.execute(&t, 0, t.len()).unwrap();
        assert_eq!((s[1].q0, s[1].q1), (0, 5));
        assert_eq!((s[2].q0, s[2].q1), (6, 11));
    }

    #[test]
    fn backward() {
        let t: Vec<char> = "ab ab ab".chars().collect();
        let r = Regex::compile_str("ab").unwrap();
        let s = r.bexecute(&t, 8).unwrap();
        assert_eq!((s[0].q0, s[0].q1), (6, 8));
        let s = r.bexecute(&t, 7).unwrap();
        assert_eq!((s[0].q0, s[0].q1), (3, 5));
        // wraps around
        let s = r.bexecute(&t, 1).unwrap();
        assert_eq!((s[0].q0, s[0].q1), (6, 8));
    }

    #[test]
    fn wraparound_forward() {
        let t: Vec<char> = "ab cd".chars().collect();
        let r = Regex::compile_str("ab").unwrap();
        let s = r.execute(&t, 3, INFINITY).unwrap();
        assert_eq!((s[0].q0, s[0].q1), (0, 2));
        assert!(r.execute(&t, 3, t.len()).is_none());
    }

    #[test]
    fn errors() {
        assert!(Regex::compile_str("(a").is_err());
        assert!(Regex::compile_str("a)").is_err());
        assert!(Regex::compile_str("[a").is_err());
        assert!(Regex::compile_str("*").is_err());
        assert!(Regex::compile_str("a|").is_err());
    }
}
