//! A shell's commands as things to act on, wherever they are listed (the
//! outline by a scrollbar): a terminal's, as its prompt marks say them,
//! or a win window's, as its outline has them (`apex_server::outline::
//! run_of`). What the buttons always mean, done to the command: B2 runs
//! it again (typed to the shell, as B2 on an old command line types it);
//! B3 opens its output in a window of its own; ⌘C copies the command and
//! its output -- `$ ` and the command, then the output, whatever the
//! shell's prompt was. A terminal's output is in its history, which only
//! the host has: read back first (`TermRead`), as Snarfout reads it.

use apex_core::{Body, ExecCtx, Op, LayoutOp, Shard, TermId, WindowId};
use apex_server::outline::{run_of, transcript, RunOutput};
use apex_server::proto::ClientMsg;
use apex_server::Proposal;
use gpui::Context;

use crate::app::{Acme, Backend};

/// What is done with a command and its output once it is had.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RunAct {
    /// Into the snarf buffer and the clipboard.
    Copy,
    /// A window of its own, beside the one it ran in.
    Open,
}

/// A terminal's text asked of the host for a command: for whom, in the
/// order asked (Snarfout's asks share the order: `Acme::read_seq`).
pub struct RunRead {
    pub seq: u64,
    pub term: TermId,
    pub window: WindowId,
    pub cmd: String,
    pub act: RunAct,
}

impl Acme {
    /// The `i`th entry of window `w`'s outline, if it is a command: its
    /// command and output, copied or opened.
    pub fn run_act(&mut self, w: WindowId, i: usize, act: RunAct, cx: &mut Context<Self>) {
        let Some(run) = run_of(&self.node, w, i) else { return };
        match run.output {
            RunOutput::Text(out) => self.run_done(w, &run.cmd, &out, act),
            RunOutput::None => self.run_done(w, &run.cmd, "", act),
            RunOutput::Rows { term, from, to } => match &mut self.backend {
                Backend::Local(server) => {
                    let text = server.term(term).map(|h| h.text((0, from), (h.cols, to))).unwrap_or_default();
                    self.run_done(w, &run.cmd, &text, act);
                }
                Backend::Remote(link) => {
                    self.read_seq += 1;
                    self.run_reads.push(RunRead { seq: self.read_seq, term, window: w, cmd: run.cmd, act });
                    link.send(&ClientMsg::TermRead { term, from, to });
                }
            },
        }
        self.settle_snarf(cx);
        cx.notify();
    }

    /// The `i`th entry of window `w`'s outline run again: typed to its
    /// shell, a terminal's with a return after it; win's as B2 on an old
    /// command line types it (its rule takes the window's commands).
    pub fn run_again(&mut self, w: WindowId, i: usize, cx: &mut Context<Self>) {
        let Some(run) = run_of(&self.node, w, i) else { return };
        if run.cmd.trim().is_empty() {
            return;
        }
        match self.node.state.window(w).map(|x| x.body) {
            Ok(Body::Term(t)) => self.term_type(t, format!("{}\r", run.cmd)),
            Ok(_) => self.execute(ExecCtx::Window(w), &run.cmd, cx),
            Err(_) => {}
        }
        cx.notify();
    }

    /// A terminal's text came back: the command it was read for, done.
    /// False when no command asked for it (Snarfout's, then).
    /// (The clipboard follows the snarf buffer in the poll, `settle_snarf`.)
    pub fn run_read(&mut self, term: TermId, text: &str) -> bool {
        let Some(i) = self.run_reads.iter().position(|r| r.term == term) else { return false };
        let r = self.run_reads.remove(i);
        self.run_done(r.window, &r.cmd, text, r.act);
        true
    }

    /// The command and its output, had: copied, or a window of their own.
    fn run_done(&mut self, w: WindowId, cmd: &str, output: &str, act: RunAct) {
        let text = transcript(cmd, output);
        match act {
            RunAct::Copy => {
                // into the snarf buffer, and the clipboard once it lands
                self.snarf_wanted = Some(self.node.state.layout.snarf.clone());
                let _ = self.node.append(&mut self.log, Shard::Layout, Op::Layout(LayoutOp::Snarf { text }));
                self.sync();
            }
            RunAct::Open => {
                // a scratch window in the column it ran in, named as acme
                // names what a command made (`dir/+name`), the command its
                // label
                let path = self.node.window_path(w);
                let dir = match path.rfind('/') {
                    Some(k) => path[..=k].to_string(),
                    None => String::new(),
                };
                let word = cmd.split_whitespace().next().unwrap_or("Output").rsplit('/').next().unwrap_or("Output").to_string();
                let Some(col) = self.node.state.layout.column_of(w).or(self.node.activecol).or_else(|| self.node.state.layout.cols.last().map(|c| c.id)) else { return };
                let made = apex_server::perform(&mut self.node, &mut self.log, vec![Proposal::NewWindow { col, name: format!("{dir}+{word}"), label: Some(cmd.to_string()), scratch: true, diagnostic: false }]);
                if let Some(nw) = made {
                    if let Some(b) = self.node.state.window(nw).ok().and_then(|x| x.body_buffer()) {
                        let version = self.node.state.buffer(b).map(|x| x.version).unwrap_or_default();
                        apex_server::perform(&mut self.node, &mut self.log, vec![Proposal::Insert { buffer: b, version, at: 0, text, follow: false }]);
                    }
                    self.show(nw);
                }
                self.after();
            }
        }
    }
}
