//! The acme `Edit` command language — sam's command language as acme
//! implements it — and the plan 9 regular expressions it uses.
//!
//! This is a port of plan9port's `src/cmd/acme/{edit.c,ecmd.c,elog.c,regx.c}`.
//! Everything is in *runes* (`char`), never bytes. The crate is pure: it reads
//! a [`Text`], runs a program, and returns a validated list of [`Change`]s in
//! original coordinates plus the new dot, printed output, and any
//! [`Intent`]s for commands with effects (`e r w b B D u < | >`), which the
//! caller carries out.
//!
//! Known, deliberate departures from acme:
//! - the fixed-size tables of the C code (NLIST, NPROG, NSTACK) are dynamic;
//! - `X`/`Y` (loops over files) are not supported: apex has no file menu.

pub mod elog;
pub mod exec;
pub mod parse;
pub mod regx;

pub use elog::Change;
pub use exec::{Edit, Intent, Outcome, PipeKind, Printed};
pub use regx::{Range, Rangeset, Regex};

/// Read-only, rune-indexed text.
pub trait Text {
    /// Length in runes.
    fn len(&self) -> usize;
    /// The rune at `i`; `i < len()`.
    fn char_at(&self, i: usize) -> char;
    /// Runes in `q0..q1`.
    fn read(&self, q0: usize, q1: usize) -> Vec<char> {
        (q0..q1).map(|i| self.char_at(i)).collect()
    }
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Text for Vec<char> {
    fn len(&self) -> usize {
        Vec::len(self)
    }
    fn char_at(&self, i: usize) -> char {
        self[i]
    }
    fn read(&self, q0: usize, q1: usize) -> Vec<char> {
        self[q0..q1].to_vec()
    }
}

impl Text for [char] {
    fn len(&self) -> usize {
        <[char]>::len(self)
    }
    fn char_at(&self, i: usize) -> char {
        self[i]
    }
    fn read(&self, q0: usize, q1: usize) -> Vec<char> {
        self[q0..q1].to_vec()
    }
}

/// An `Edit` error, with acme's message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Error(pub String);

impl Error {
    pub fn new(s: impl Into<String>) -> Self {
        Error(s.into())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Apply changes (as returned in [`Outcome::changes`]) to a text.
pub fn apply(text: &mut Vec<char>, changes: &[Change]) {
    for c in changes {
        let q0 = c.q0.min(text.len());
        let q1 = (c.q0 + c.nd).min(text.len());
        text.splice(q0..q1, c.text.iter().copied());
    }
}
