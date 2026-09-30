//! The session's processes (`Meta::procs`), as the UI shows them: a pill
//! each in the session's tag, and rows in the sidebar. The × on one ends
//! the process (`Kill` by its pid); B1 on it goes to its output (its
//! directory's errors window, or the window whose text it replaces); B3
//! to the window it was run from.

use gpui::{Context, MouseButton};

use apex_core::entry::ProcOut;
use apex_core::Seq;
use apex_core::state::Proc;
use apex_core::{Body, ExecCtx, Loc, Pos, WindowId};
use apex_server::Proposal;

use crate::app::Acme;
use crate::text_element::{Atom, Head};

impl Acme {
    /// The processes running, oldest first.
    pub fn running_procs(&self) -> Vec<Proc> {
        self.node.state.meta.procs.iter().filter(|p| p.running()).cloned().collect()
    }

    /// The session's tag's head: a pill for each process running.
    pub fn top_head(&self) -> Head {
        let procs: Vec<(Seq, String)> = self.running_procs().into_iter().map(|p| (p.id, p.name)).collect();
        Head::procs(&procs)
    }

    fn proc(&self, id: Seq) -> Option<Proc> {
        self.node.state.meta.procs.iter().find(|p| p.id == id).cloned()
    }

    /// A press on a pill: its × ends the process, B1 on its name goes to
    /// its output, B3 to where it was run from.
    pub fn press_proc(&mut self, atom: Atom, button: MouseButton, cx: &mut Context<Self>) {
        match (atom, button) {
            (Atom::ProcKill(id), MouseButton::Left | MouseButton::Middle) => self.kill_proc(id, cx),
            (Atom::Proc(id), MouseButton::Left) => self.proc_output(id, cx),
            (Atom::Proc(id), MouseButton::Right) => self.proc_origin(id, cx),
            _ => {}
        }
        cx.notify();
    }

    /// End process `id`, as `Kill` its pid would.
    pub fn kill_proc(&mut self, id: Seq, cx: &mut Context<Self>) {
        let Some(p) = self.proc(id).filter(|p| p.running()) else { return };
        self.execute(ExecCtx::Top, &format!("Kill {}", p.pid), cx);
        self.after();
    }

    /// Go to where process `id`'s output goes: its directory's errors
    /// window (made if it has none yet, as its output would make it), or
    /// the window whose text it replaces; else where it was run from.
    pub fn proc_output(&mut self, id: Seq, cx: &mut Context<Self>) {
        let Some(p) = self.proc(id) else { return };
        let w = match &p.out {
            ProcOut::Errors { dir } => self.node.errors_window(dir.as_deref()).or_else(|| self.node.errors(&mut self.log, dir.as_deref(), "").ok()),
            ProcOut::Buffer(b) => self.node.state.windows.values().find(|w| w.body == Body::Text(*b)).map(|w| w.id),
            ProcOut::None => None,
        };
        match w {
            Some(w) => self.go_to_window(w, cx),
            None => self.proc_origin(id, cx),
        }
    }

    /// Go to the window process `id` was run from, if it was run from one
    /// that is still there.
    pub fn proc_origin(&mut self, id: Seq, cx: &mut Context<Self>) {
        let Some(p) = self.proc(id) else { return };
        if let ExecCtx::Window(w) = p.origin {
            if self.node.state.window(w).is_ok() {
                self.go_to_window(w, cx);
            }
        }
    }

    /// Land on window `w` by its id (its path may be another's too), as a
    /// jump: where we were goes on the back stack.
    pub fn go_to_window(&mut self, w: WindowId, cx: &mut Context<Self>) {
        let loc = Loc { session: None, name: w.0.to_string(), pos: Pos::Keep };
        let _ = apex_server::proposal::apply(&mut self.node, &mut self.log, Proposal::Goto { loc });
        self.sync();
        self.after();
        cx.notify();
    }
}
