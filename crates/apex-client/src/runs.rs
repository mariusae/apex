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
        self.run_act_now(w, i, act);
        self.settle_snarf(cx);
        cx.notify();
    }

    /// `run_act` where no context is at hand (a rule's ask): the
    /// clipboard follows the snarf buffer in the poll (`settle_snarf`).
    pub fn run_act_now(&mut self, w: WindowId, i: usize, act: RunAct) {
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

/// A command's dot: shown at the end of its prompt's row while the
/// pointer is on that row, the command's handle there. B1 on it selects
/// the command and its output (B2 then, B1 still held, copies them, as
/// B1+B2 copies a sweep); B2 runs it again; B3 opens its output.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RunDot {
    pub window: WindowId,
    /// Its entry in the window's outline (`outline::of`'s order).
    pub entry: usize,
    pub center: gpui::Point<gpui::Pixels>,
    pub failed: bool,
    pub running: bool,
}

/// The dot's size across, and the room a click on it has.
const DOT: f32 = 7.;
const DOT_HIT: f32 = 16.;
/// In from the row's right end.
const DOT_IN: f32 = 12.;

/// A dot's ink: quiet; blue while the command runs, fixed, not the
/// palette's accent (one palette's is a gold, by the orange), as the
/// session's standing has it; the terminal's red, orange in every
/// palette, when it failed. Under a deuteranopia simulation the three
/// are ΔE 39 or more apart, the blue and the orange over 110.
fn dot_ink(failed: bool, running: bool) -> u32 {
    let t = crate::theme::theme();
    if failed {
        t.ansi[1]
    } else if running {
        if crate::theme::is_dark() { 0x539BF5 } else { 0x0969DA }
    } else {
        t.text_dim
    }
}

/// Where a prompt ends on its line (chars): where the command it reads
/// begins -- the whole command, found from the right (a prompt may hold
/// its first word: `~/make % make`), else its first word; else nothing
/// of it is known but the line's start.
pub fn prompt_end(line: &str, cmd: &str) -> usize {
    let cmd = cmd.trim();
    let Some(word) = cmd.split_whitespace().next() else { return 0 };
    let line = line.trim_end();
    match line.rfind(cmd).filter(|&b| b + cmd.len() == line.len()).or_else(|| line.find(word)) {
        Some(b) => line[..b].chars().count(),
        None => 0,
    }
}

impl RunDot {
    pub fn hit(&self, p: gpui::Point<gpui::Pixels>) -> bool {
        let d = gpui::px(DOT_HIT / 2.);
        (p.x - self.center.x).abs() <= d && (p.y - self.center.y).abs() <= d
    }
}

impl Acme {
    /// The dot of the command whose prompt's row `pos` is on, in a
    /// terminal or a text with commands in its outline (win's).
    pub fn run_dot_near(&self, pos: gpui::Point<gpui::Pixels>) -> Option<RunDot> {
        use gpui::px;
        if self.over_overlay(pos) {
            return None;
        }
        for (w, l) in &self.term_layouts {
            if !l.bounds.contains(&pos) || l.scrollbar.contains(&pos) {
                continue;
            }
            let Ok(Body::Term(t)) = self.node.state.window(*w).map(|x| x.body) else { continue };
            let term = self.node.state.terms.get(&t)?;
            let (_, r) = l.cell_at(pos);
            let line = term.top + r as u64;
            let (entry, m) = term.marks.iter().filter(|m| m.started.is_some() || m.cmd.is_some()).enumerate().find(|(_, m)| m.prompt == line)?;
            let right = (l.text_origin.x + l.cell_w * l.cols as f32).min(l.bounds.right());
            let center = gpui::point(right - px(DOT_IN), l.text_origin.y + l.line_height * (r as f32 + 0.5));
            return Some(RunDot { window: *w, entry, center, failed: m.exit.is_some_and(|x| x != 0), running: m.started.is_some() && m.ended.is_none() });
        }
        for (v, l) in &self.layouts {
            let apex_core::ViewId::Body(w) = *v else { continue };
            if !l.bounds.contains(&pos) || l.scrollbar.is_some_and(|b| b.contains(&pos)) {
                continue;
            }
            let o = self.node.window_outline(w)?;
            for (entry, it) in o.items.iter().enumerate().filter(|(_, it)| it.started.is_some()) {
                let Some(at) = l.point_of(it.at) else { continue };
                if pos.y >= at.y && pos.y < at.y + l.line_height {
                    let center = gpui::point(l.bounds.right() - px(DOT_IN), at.y + l.line_height / 2.);
                    return Some(RunDot { window: w, entry, center, failed: it.exit.is_some_and(|x| x != 0), running: it.ended.is_none() });
                }
            }
            return None;
        }
        None
    }

    /// The dot the pointer is on, if it is on one.
    pub fn run_dot_at(&self, pos: gpui::Point<gpui::Pixels>) -> Option<RunDot> {
        self.run_dot_near(pos).filter(|d| d.hit(pos))
    }

    /// The dot of the prompt row the pointer is on, drawn in its ink
    /// (`dot_ink`), ringed with the pointer on it.
    pub fn run_dot_element(&self) -> Option<gpui::AnyElement> {
        use gpui::{div, prelude::*, px, rgb};
        let pos = self.last_mouse;
        let d = self.run_dot_near(pos)?;
        let t = crate::theme::theme();
        let ink = dot_ink(d.failed, d.running);
        let on = d.hit(pos);
        let size = if on { DOT + 3. } else { DOT };
        Some(
            div()
                .absolute()
                .left(d.center.x - px(size / 2.))
                .top(d.center.y - px(size / 2.))
                .size(px(size))
                .rounded_full()
                .bg(rgb(ink))
                .when(on, |e| e.border(px(1.5)).border_color(rgb(crate::text_element::ground(&t))))
                .into_any_element(),
        )
    }

    /// A button down on a command's dot: what it means there. True when
    /// it was on one.
    pub fn run_dot_down(&mut self, pos: gpui::Point<gpui::Pixels>, button: gpui::MouseButton, cx: &mut Context<Self>) -> bool {
        use gpui::MouseButton;
        // B2 with B1 held from a dot: the command and its output copied
        if let Some((w, i)) = self.dot_held {
            if button == MouseButton::Middle {
                self.dot_chorded = true;
                self.run_act(w, i, RunAct::Copy, cx);
                return true;
            }
        }
        let Some(d) = self.run_dot_at(pos) else { return false };
        self.dot_button(d.window, d.entry, button, false, cx)
    }

    /// A button on command `i`'s dot in window `w`, wherever the dot is
    /// drawn (its prompt's row, a list's row): B1 selects the command and
    /// its output, B2 with B1 held from it copies them, B2 runs it again,
    /// B3 opens it. A dot in a list (`listed`: the outline's, ⌘P's) is
    /// away from its command: B1 there goes to it too, and ⌘P goes, as
    /// choosing in it does. True when the button means something there.
    pub fn dot_button(&mut self, w: WindowId, i: usize, button: gpui::MouseButton, listed: bool, cx: &mut Context<Self>) -> bool {
        use gpui::MouseButton;
        if listed && self.finder.is_some() {
            self.close_finder(cx);
        }
        if button == MouseButton::Middle && self.dot_held.is_some() {
            let (hw, hi) = self.dot_held.unwrap_or((w, i));
            self.dot_chorded = true;
            self.run_act(hw, hi, RunAct::Copy, cx);
            return true;
        }
        match button {
            MouseButton::Left => {
                if listed {
                    self.outline_go(w, i);
                }
                self.run_select(w, i);
                self.dot_held = Some((w, i));
                self.dot_chorded = false;
            }
            MouseButton::Middle => self.run_again(w, i, cx),
            MouseButton::Right => self.run_act(w, i, RunAct::Open, cx),
            _ => return false,
        }
        cx.notify();
        true
    }

    /// How command `i` of window `w` stands, for its dot: failed, running.
    pub fn run_state(&self, w: WindowId, i: usize) -> Option<(bool, bool)> {
        match self.node.state.window(w).ok()?.body {
            Body::Term(t) => {
                let m = self.node.state.terms.get(&t)?.marks.iter().filter(|m| m.started.is_some() || m.cmd.is_some()).nth(i)?;
                Some((m.exit.is_some_and(|x| x != 0), m.started.is_some() && m.ended.is_none()))
            }
            _ => {
                let it = self.node.window_outline(w)?.items.get(i).filter(|it| it.started.is_some())?;
                Some((it.exit.is_some_and(|x| x != 0), it.ended.is_none()))
            }
        }
    }

    /// Command `i`'s dot as an element of a list (the outline's by the
    /// scrollbar, ⌘P's): always shown, its buttons the dot's anywhere.
    pub fn dot_view(&self, w: WindowId, i: usize, cx: &Context<Self>) -> Option<gpui::Stateful<gpui::Div>> {
        use gpui::{div, prelude::*, px, rgb, MouseButton};
        let (failed, running) = self.run_state(w, i)?;
        let t = crate::theme::theme();
        let ink = dot_ink(failed, running);
        let hit = |b: MouseButton| {
            cx.listener(move |acme: &mut Acme, _: &gpui::MouseDownEvent, _, cx| {
                cx.stop_propagation();
                acme.dot_button(w, i, b, true, cx);
            })
        };
        Some(
            div()
                .id(("run-dot", i))
                .flex_none()
                .size(px(DOT_HIT))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_default()
                .hover(move |s| s.bg(rgb(t.panel_hover)))
                .child(div().size(px(DOT)).rounded_full().bg(rgb(ink)))
                .on_mouse_down(MouseButton::Left, hit(MouseButton::Left))
                .on_mouse_down(MouseButton::Middle, hit(MouseButton::Middle))
                .on_mouse_down(MouseButton::Right, hit(MouseButton::Right)),
        )
    }

    /// A command and its output selected: a text's span, from its prompt
    /// to its output's end; a terminal's rows. (A double-click on its
    /// prompt selects so too, as one by a bracket selects to its match.)
    pub fn run_select(&mut self, w: WindowId, i: usize) {
        match self.node.state.window(w).map(|x| x.body) {
            Ok(Body::Term(t)) => {
                let Some(term) = self.node.state.terms.get(&t) else { return };
                let Some(m) = term.marks.iter().filter(|m| m.started.is_some() || m.cmd.is_some()).nth(i) else { return };
                let last = match m.end {
                    Some(e) => e.saturating_sub(1).max(m.prompt),
                    None => term.total.saturating_sub(1).max(m.prompt),
                };
                self.term_sel = Some((w, (0, m.prompt), (term.cols as usize, last)));
            }
            Ok(_) => {
                let Some(it) = self.node.window_outline(w).and_then(|o| o.items.get(i)).cloned() else { return };
                let len = self.text_of(apex_core::ViewId::Body(w)).map_or(0, |t| t.len());
                let end = match it.output {
                    Some((_, Some(z))) => z,
                    _ => len,
                };
                let _ = self.node.select(&mut self.log, apex_core::ViewId::Body(w), it.at.min(len), end.min(len));
                self.node.seltext = Some(apex_core::ViewId::Body(w));
                self.sync();
            }
            Err(_) => {}
        }
    }
}

impl Acme {
    /// The command whose prompt a double-click at `off` in window `w`'s
    /// text is on (its characters, or the start of its line): the
    /// command and its output are the delimited thing there.
    pub fn run_at_prompt_text(&self, w: WindowId, off: usize) -> Option<usize> {
        let text = self.text_of(apex_core::ViewId::Body(w))?;
        let o = self.node.window_outline(w)?;
        o.items.iter().enumerate().filter(|(_, it)| it.started.is_some()).find_map(|(i, it)| {
            let line_end = (it.at..text.len()).find(|&q| text.char_at(q) == '\n').unwrap_or(text.len());
            let line = text.slice(it.at, line_end);
            let end = it.at + prompt_end(&line, &it.title);
            (off >= it.at && off <= end && off <= line_end).then_some(i)
        })
    }

    /// The command whose prompt a double-click at column `col` of
    /// terminal row `row` (of the view) is on.
    pub fn run_at_prompt_term(&self, w: WindowId, col: usize, row: usize) -> Option<usize> {
        let Ok(Body::Term(t)) = self.node.state.window(w).map(|x| x.body) else { return None };
        let term = self.node.state.terms.get(&t)?;
        let line = term.top + row as u64;
        let (i, m) = term.marks.iter().filter(|m| m.started.is_some() || m.cmd.is_some()).enumerate().find(|(_, m)| m.prompt == line)?;
        let text = self.term_layouts.get(&w).and_then(|l| l.rows.get(row)).cloned().unwrap_or_default();
        let end = prompt_end(&text, m.cmd.as_deref().unwrap_or(""));
        (col <= end).then_some(i)
    }

    /// The command the pointer is in, in window `w`: from its prompt to
    /// where its output ends (or the next prompt); else the last.
    fn run_under(&self, w: WindowId, pos: gpui::Point<gpui::Pixels>) -> Option<usize> {
        match self.node.state.window(w).ok()?.body {
            Body::Term(t) => {
                let term = self.node.state.terms.get(&t)?;
                let marks: Vec<_> = term.marks.iter().filter(|m| m.started.is_some() || m.cmd.is_some()).collect();
                let line = self.term_layouts.get(&w).filter(|l| l.bounds.contains(&pos)).map(|l| term.top + l.cell_at(pos).1 as u64);
                let here = line.and_then(|line| marks.iter().rposition(|m| m.prompt <= line));
                here.or(marks.len().checked_sub(1))
            }
            _ => {
                let o = self.node.window_outline(w)?;
                let runs: Vec<usize> = o.items.iter().enumerate().filter(|(_, it)| it.started.is_some()).map(|(i, _)| i).collect();
                let off = self.layouts.get(&apex_core::ViewId::Body(w)).filter(|l| l.bounds.contains(&pos)).map(|l| l.offset_at(pos));
                let here = off.and_then(|off| runs.iter().rev().find(|&&i| o.items[i].at <= off).copied());
                here.or(runs.last().copied())
            }
        }
    }

    /// `Snarfcmd` (B4, a rule's): the command the pointer is in (else the
    /// window's last) and its output, copied as `$ ` and the command, then
    /// the output -- what its dot copies.
    pub fn snarfcmd(&mut self, id: u64, w: WindowId) {
        let result = match self.run_under(w, self.last_mouse) {
            Some(i) => {
                self.run_act_now(w, i, RunAct::Copy);
                Ok(None)
            }
            None => Err("Snarfcmd: no command the shell marked here".to_string()),
        };
        self.send(ClientMsg::Applied { id, result });
    }
}

#[cfg(test)]
mod tests {
    use super::prompt_end;

    #[test]
    fn a_prompt_ends_where_its_command_begins() {
        assert_eq!(prompt_end("% make test", "make test"), 2);
        assert_eq!(prompt_end("marius@airy ~/src/apex % git status", "git status"), 25);
        // its first word found, whatever the rest became (rc's lines joined)
        assert_eq!(prompt_end("% for(i in a){", "for(i in a){ echo $i }"), 2);
        // not found (the line scrolled, or another's): only the line's start
        assert_eq!(prompt_end("garbage", "make"), 0);
        assert_eq!(prompt_end("% ", ""), 0);
        // the prompt holding the command's word: the command at the end
        assert_eq!(prompt_end("~/make % make", "make"), 9);
    }
}
