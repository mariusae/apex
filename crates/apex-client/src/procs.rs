//! The session's processes (`Meta::procs`), as the UI shows them: a pill
//! each in the session's tag, and rows in the sidebar. The × on one ends
//! the process (`Kill` by its pid); B1 on it goes to its output (its
//! directory's errors window, or the window whose text it replaces); B3
//! to the window it was run from. The pointer on a pill brings a card
//! under it: the whole command line, the pid, the directory, when it
//! started and where from. A terminal's shell is its window's, and is
//! not among them (`apex ps` lists it).

use gpui::prelude::*;
use gpui::{anchored, deferred, div, point, px, rgb, AnyElement, Bounds, Context, MouseButton, Pixels};

use apex_core::entry::{ProcKind, ProcOut};
use apex_core::Seq;
use apex_core::state::Proc;
use apex_core::{Body, ExecCtx, Loc, Pos, WindowId};
use apex_server::Proposal;

use crate::app::Acme;
use crate::text_element::{Atom, Head};

impl Acme {
    /// The processes running, oldest first, but for terminals' shells
    /// (seen in their windows).
    pub fn running_procs(&self) -> Vec<Proc> {
        self.node.state.meta.procs.iter().filter(|p| p.running() && p.kind != ProcKind::Term).cloned().collect()
    }

    /// The pill under the pointer at `pos`, and where it is drawn: taken
    /// as the pointer moves, from the layouts as they are then.
    pub fn pill_at(&self, pos: gpui::Point<Pixels>) -> Option<(Seq, Bounds<Pixels>)> {
        let l = self.layouts.get(&apex_core::ViewId::Top)?;
        let id = match l.atom_at(pos)? {
            Atom::Proc(id) | Atom::ProcKill(id) => id,
            _ => return None,
        };
        let (a, b) = (l.atom_bounds(Atom::Proc(id))?, l.atom_bounds(Atom::ProcKill(id))?);
        Some((id, Bounds::from_corners(a.origin, b.bottom_right())))
    }

    /// The card under the pill the pointer is on: the process's whole
    /// command line, its pid and directory, when it started and where
    /// it was run from.
    pub fn proc_card(&self) -> Option<AnyElement> {
        let (id, at) = self.proc_hover?;
        let p = self.proc(id).filter(|p| p.running())?;
        let t = crate::theme::theme();
        let from = match p.origin {
            ExecCtx::Window(w) if self.node.state.window(w).is_ok() => format!("from {}", crate::sidebar::names(&self.node, w).0),
            ExecCtx::Window(_) => "from a window since closed".into(),
            ExecCtx::Column(_) => "from a column's tag".into(),
            ExecCtx::Top if p.kind == ProcKind::Adopted => "announced itself".into(),
            ExecCtx::Top => "from the session's tag".into(),
        };
        let dim = |s: String| div().text_color(rgb(t.panel_dim)).child(s);
        let shadow = gpui::BoxShadow { color: gpui::hsla(0., 0., 0., 0.2), offset: gpui::point(px(0.), px(4.)), blur_radius: px(14.), spread_radius: px(0.), inset: false };
        let card = div()
            .max_w(px(520.))
            .px(px(10.))
            .py(px(8.))
            .rounded(px(8.))
            .bg(rgb(t.panel_bg))
            .border_1()
            .border_color(rgb(t.panel_border))
            .shadow(vec![shadow])
            .font_family(crate::fonts::ui())
            .text_size(px(12.))
            .text_color(rgb(t.panel_text))
            .flex()
            .flex_col()
            .gap(px(3.))
            .child(self.overlay_mark())
            // the command as the shell got it, in the fixed face
            .child(div().font_family(crate::text_element::font_for(true).font.family.clone()).child(p.cmd.clone()))
            .child(dim(format!("pid {}  ·  {}", p.pid, crate::sidebar::shown(&p.dir, &self.node.state.meta.cwd))))
            .child(dim(format!("started {}  ·  {from}", apex_server::when(p.started))));
        Some(deferred(anchored().position(point(at.left(), at.bottom() + px(4.))).child(card)).with_priority(3).into_any_element())
    }

    /// The session's tag's head: a pill for each process running, after
    /// a chevron from the directory when the title bar shows it.
    pub fn top_head(&self) -> Head {
        let procs: Vec<(Seq, String)> = self.running_procs().into_iter().map(|p| (p.id, p.name)).collect();
        Head::procs(&procs, !self.node.state.meta.cwd.is_empty())
    }

    /// The top row after the directory, with nothing for its square to
    /// say (the leases held, no notification to take): no room kept for
    /// the square, so the chevron follows the directory as the others do.
    pub fn top_bare(&self) -> bool {
        !self.node.state.meta.cwd.is_empty() && !self.fenced() && self.notification_head().is_none()
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

    /// End process `id`, as `Kill` its pid would -- asked of the server
    /// directly, as `apex kill` asks, not run as a command: a click on a
    /// × is not an exec of the session's.
    pub fn kill_proc(&mut self, id: Seq, cx: &mut Context<Self>) {
        let Some(p) = self.proc(id).filter(|p| p.running()) else { return };
        let target = p.pid.to_string();
        match &mut self.backend {
            crate::app::Backend::Local(server) => {
                server.kill(&target);
            }
            crate::app::Backend::Remote(link) => link.send(&apex_server::proto::ClientMsg::Kill { targets: vec![target] }),
        }
        self.after();
        cx.notify();
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
