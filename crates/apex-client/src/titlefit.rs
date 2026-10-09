//! The title bar fitted to its room: the session's directory and its
//! processes each have a few ways to be shown, from whole to least, and
//! the bar takes the richest of them that leaves the top row the room it
//! needs (`choose`). The directory: its host and every crumb; then its
//! last two crumbs after `…/`; then its last alone, the host gone. The
//! processes: a pill for each name, `Win 5` for five of them; then the
//! pills stacked as cards, the newest on top; then a count alone. What
//! is cut short fans out under the pointer: the whole path, or every
//! process on a pill of its own with its ×, over the top row rather than
//! pushing it along, and folds again when the pointer leaves -- so the
//! bar never reflows under a pointer taking aim. The top row gets its
//! whole text when its caret is in it, and some of it otherwise.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{anchored, deferred, div, point, prelude::*, px, rgb, AnyElement, Bounds, Context, FontWeight, MouseButton, Pixels, Point, Window};

use apex_core::{Seq, ViewId};

use crate::app::Acme;
use crate::text_element::{ground, mix};

/// How much of the directory shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PathFit {
    /// The host and every crumb.
    Full,
    /// The host, `…/` and the last two crumbs.
    Short,
    /// `…/` and the last crumb.
    Last,
}

/// How the processes show.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProcFit {
    /// A pill for each name, counted when there are several.
    Full,
    /// The pills stacked as cards, the newest on top.
    Stack,
    /// How many there are.
    Count,
}

/// What the bar shows, the richest first: the directory gives way first,
/// as the processes' names say more than the folders above the one the
/// session is in.
pub const LADDER: [(PathFit, ProcFit); 5] = [
    (PathFit::Full, ProcFit::Full),
    (PathFit::Short, ProcFit::Full),
    (PathFit::Short, ProcFit::Stack),
    (PathFit::Last, ProcFit::Stack),
    (PathFit::Last, ProcFit::Count),
];

/// The richest step of the ladder whose directory and processes leave
/// `text` of `room` to the top row; the least when none does.
pub fn choose(room: f32, text: f32, path: impl Fn(PathFit) -> f32, procs: impl Fn(ProcFit) -> f32) -> (PathFit, ProcFit) {
    LADDER.iter().copied().find(|&(a, b)| path(a) + procs(b) + text <= room).unwrap_or(LADDER[LADDER.len() - 1])
}

/// The processes by name, in the order each name first ran: the name,
/// and its processes, oldest first.
pub fn groups(procs: &[(Seq, String)]) -> Vec<(String, Vec<Seq>)> {
    let mut out: Vec<(String, Vec<Seq>)> = Vec::new();
    for (id, name) in procs {
        match out.iter_mut().find(|g| g.0 == *name) {
            Some(g) => g.1.push(*id),
            None => out.push((name.clone(), vec![*id])),
        }
    }
    out
}

/// The first of `n` crumbs a directory shows at `fit`: all of them, the
/// last two, or the last.
pub fn first_crumb(n: usize, fit: PathFit) -> usize {
    match fit {
        PathFit::Full => 0,
        PathFit::Short => n.saturating_sub(2),
        PathFit::Last => n.saturating_sub(1),
    }
}

/// The most cards a stack shows.
const STACK: usize = 4;
/// How much of each card under the top one shows.
const STEP: f32 = 7.;
/// The pills' type.
const SIZE: f32 = 12.;
/// A pill's × and the room round it.
const KILL_W: f32 = 14.;
/// Between pills.
const GAP: f32 = 4.;
/// The chevron before the processes, and its room.
const CHEVRON_W: f32 = 16.;
/// The process fan's least distance from the window's edges.
const FAN_MARGIN: f32 = 8.;
/// The top row when its caret is elsewhere: as much of its text as this.
const TEXT_MIN: f32 = 200.;

/// Where the bar's parts were drawn last, for the pointer: its room, the
/// processes and the directory when they would fan out (and the fans
/// when they are out), and each pill.
#[derive(Default)]
pub struct Marks {
    pub room: f32,
    pub procs: Option<Bounds<Pixels>>,
    pub procs_fan: Option<Bounds<Pixels>>,
    pub path: Option<Bounds<Pixels>>,
    pub path_fan: Option<Bounds<Pixels>>,
    pub pills: Vec<(Seq, Bounds<Pixels>)>,
    /// Where the processes and the path were drawn the frame before: what
    /// a fan is placed by, since it is made before this frame draws them
    /// (and records where they are) -- the bar does not move under a
    /// pointer on it.
    pub drawn_procs: Option<Bounds<Pixels>>,
    pub drawn_path: Option<Bounds<Pixels>>,
}

/// What is fanned out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fan {
    Path,
    Procs,
}

/// An element that records where it lands.
fn mark(f: impl Fn(Bounds<Pixels>) + 'static) -> AnyElement {
    div().absolute().top(px(0.)).left(px(0.)).size_full().child(gpui::canvas(move |b, _, _| f(b), |_, _, _, _| {}).size_full()).into_any_element()
}

/// A chevron pointing right, faint: what comes next is in what came
/// before.
fn chevron(ink: gpui::Hsla) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let c = b.center();
            let mut p = gpui::PathBuilder::stroke(px(1.25));
            p.move_to(point(c.x - px(1.5), c.y - px(3.5)));
            p.line_to(point(c.x + px(2.), c.y));
            p.line_to(point(c.x - px(1.5), c.y + px(3.5)));
            if let Ok(path) = p.build() {
                window.paint_path(path, ink);
            }
        },
    )
    .flex_none()
    .w(px(CHEVRON_W))
    .h(px(12.))
}

fn ui_w(window: &Window, s: &str, size: f32, weight: FontWeight) -> f32 {
    if s.is_empty() {
        return 0.;
    }
    let mut font = gpui::font(crate::fonts::ui());
    font.weight = weight;
    let run = gpui::TextRun { len: s.len(), font, color: gpui::black(), background_color: None, underline: None, strikethrough: None };
    f32::from(window.text_system().shape_line(s.to_string().into(), px(size), &[run], None).width).ceil()
}

/// A pill's width: its name, its count when it has one, its ×.
fn pill_w(window: &Window, name: &str, n: usize) -> f32 {
    let count = if n > 1 { 4. + ui_w(window, &n.to_string(), SIZE - 1., FontWeight::MEDIUM) } else { 0. };
    8. + ui_w(window, name, SIZE, FontWeight::NORMAL) + count + KILL_W
}

impl Acme {
    /// The processes running, as the title bar has them: id and name,
    /// oldest first.
    fn title_procs(&self) -> Vec<(Seq, String)> {
        self.running_procs().into_iter().map(|p| (p.id, p.name)).collect()
    }

    /// What the bar shows in the room it had last frame.
    pub fn title_fit(&self, window: &Window) -> (PathFit, ProcFit) {
        let room = self.title_marks.borrow().room;
        let meta = &self.node.state.meta;
        let parts = crate::cwdbar::crumbs(&meta.cwd);
        let host = ui_w(window, &meta.host, 12., FontWeight::NORMAL) + 5.;
        let crumbs = |from: usize| -> f32 {
            let last = parts.len().saturating_sub(1);
            parts.iter().enumerate().skip(from).map(|(i, (p, _))| ui_w(window, p, 13., if i == last { FontWeight::MEDIUM } else { FontWeight::NORMAL })).sum::<f32>()
                + if from > 0 { ui_w(window, "…/", 13., FontWeight::NORMAL) } else { 0. }
        };
        let picking = self.cwd_picker.is_some();
        let path = |fit: PathFit| -> f32 {
            if meta.cwd.is_empty() {
                return 0.;
            }
            // the folder being picked in is shown whole, being typed in
            let fit = if picking { PathFit::Full } else { fit };
            let host = if fit == PathFit::Last { 0. } else { host };
            host + crumbs(first_crumb(parts.len(), fit)) + 3.
        };
        let groups = groups(&self.title_procs());
        let procs = |fit: ProcFit| -> f32 {
            if groups.is_empty() {
                return 0.;
            }
            CHEVRON_W
                + match fit {
                    ProcFit::Full => groups.iter().map(|(name, ids)| pill_w(window, name, ids.len()) + GAP).sum(),
                    ProcFit::Stack => {
                        let (name, _) = &groups[groups.len() - 1];
                        let total: usize = groups.iter().map(|g| g.1.len()).sum();
                        STEP * (groups.len().min(STACK) - 1) as f32 + pill_w(window, name, total) + GAP
                    }
                    ProcFit::Count => self.count_w(window, groups.iter().map(|g| g.1.len()).sum()) + GAP,
                }
        };
        // the top row: its whole text while the caret is in it
        let text = self.view_text(ViewId::Top).trim_end().to_string();
        let fs = crate::text_element::font_for(false);
        let run = gpui::TextRun { len: text.len(), font: fs.font.clone(), color: gpui::black(), background_color: None, underline: None, strikethrough: None };
        let whole = if text.is_empty() { 0. } else { f32::from(window.text_system().shape_line(text.clone().into(), fs.size, &[run], None).width) + 24. };
        let typing = self.node.seltext == Some(ViewId::Top);
        let need = if typing { whole } else { whole.min(TEXT_MIN) };
        choose(room, need, path, procs)
    }

    fn count_w(&self, window: &Window, n: usize) -> f32 {
        16. + ui_w(window, &n.to_string(), SIZE, FontWeight::MEDIUM)
    }

    /// The room the bar's middle has, measured where it is laid out: a
    /// change is drawn again with the fit it makes.
    pub fn title_room_mark(&self, me: &gpui::Entity<Acme>) -> AnyElement {
        let me = me.clone();
        div()
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .size_full()
            .child(
                gpui::canvas(
                    move |b, _, cx| {
                        let w = f32::from(b.size.width);
                        me.update(cx, |acme, cx| {
                            let mut m = acme.title_marks.borrow_mut();
                            if (m.room - w).abs() > 0.5 {
                                m.room = w;
                                cx.notify();
                            }
                        });
                    },
                    |_, _, _, _| {},
                )
                .size_full(),
            )
            .into_any_element()
    }

    /// The fan the pointer at `pos` is on, by where the bar's parts were
    /// drawn last.
    pub fn title_fan_at(&self, pos: Point<Pixels>) -> Option<Fan> {
        let m = self.title_marks.borrow();
        let on = |b: Option<Bounds<Pixels>>| b.is_some_and(|b| b.contains(&pos));
        if on(m.procs_fan) || (on(m.procs) && self.title_fan != Some(Fan::Path)) {
            Some(Fan::Procs)
        } else if on(m.path_fan) || on(m.path) {
            Some(Fan::Path)
        } else {
            None
        }
    }

    /// The pill under the pointer at `pos`, and where it is drawn.
    pub fn pill_at(&self, pos: Point<Pixels>) -> Option<(Seq, Bounds<Pixels>)> {
        self.title_marks.borrow().pills.iter().rev().find(|(_, b)| b.contains(&pos)).copied()
    }

    /// The processes, as `fit` has them, after a chevron; and, while the
    /// pointer is on them and something of them is folded away, every
    /// one on a pill of its own, fanned out over the top row.
    pub fn procs_strip(&self, fit: ProcFit, width: Pixels, cx: &mut Context<Self>) -> Option<AnyElement> {
        let procs = self.title_procs();
        if procs.is_empty() {
            return None;
        }
        let t = crate::theme::theme();
        let groups = groups(&procs);
        let folded = fit != ProcFit::Full || groups.len() < procs.len();
        let total = procs.len();
        let body: AnyElement = match fit {
            ProcFit::Full => div().flex().flex_row().items_center().gap(px(GAP)).children(groups.iter().map(|(name, ids)| self.pill(name, *ids.last().unwrap(), ids.len(), true, cx))).into_any_element(),
            ProcFit::Stack => {
                // the newest names on top, each card under it peeking out
                // at its left; the top one says how many there are in all
                let shown: Vec<&(String, Vec<Seq>)> = groups.iter().rev().take(STACK).rev().collect();
                let k = shown.len();
                let mut stack = div().relative().flex_none().flex().flex_row().items_center();
                for (i, (name, ids)) in shown[..k - 1].iter().enumerate() {
                    stack = stack.child(div().absolute().top(px(0.)).left(px(STEP * i as f32)).child(self.pill(name, *ids.last().unwrap(), ids.len(), false, cx)));
                }
                let (name, ids) = shown[k - 1];
                stack.child(div().flex_none().ml(px(STEP * (k - 1) as f32)).child(self.pill(name, *ids.last().unwrap(), total, false, cx))).into_any_element()
            }
            ProcFit::Count => div()
                .flex_none()
                .h(px(20.))
                .px(px(7.))
                .rounded(px(10.))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.))
                .bg(rgb(mix(t.text_dim, ground(&t), 0.9)))
                .text_color(rgb(t.text_dim))
                .child(div().size(px(6.)).rounded(px(3.)).bg(rgb(t.accent)))
                .child(div().font_weight(crate::fonts::weight(FontWeight::MEDIUM)).child(total.to_string()))
                .into_any_element(),
        };
        let marks = self.title_marks.clone();
        let strip = div()
            .flex_none()
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .font_family(crate::fonts::ui())
            .text_size(px(SIZE))
            .child(chevron(rgb(mix(t.text_dim, ground(&t), 0.4)).into()))
            .child(body)
            .child(div().flex_none().w(px(GAP)))
            .when(folded, |d| d.child(mark(move |b| marks.borrow_mut().procs = Some(b))));
        let fan = (folded && self.title_fan == Some(Fan::Procs)).then(|| self.procs_fan(&procs, width, cx));
        Some(div().flex_none().child(strip).children(fan).into_any_element())
    }

    /// Every process on a pill of its own, in a card over the bar from
    /// where the processes begin, rightwards: as wide as the window has
    /// room for there (the pills wrapping to more rows), and moved left
    /// only as far as it must to stay in the window (`width` across).
    fn procs_fan(&self, procs: &[(Seq, String)], width: Pixels, cx: &mut Context<Self>) -> AnyElement {
        let t = crate::theme::theme();
        let at = self.title_marks.borrow().drawn_procs.map(|b| b.origin).unwrap_or_default();
        let room = (width - at.x - px(FAN_MARGIN)).clamp(px(240.), px(720.));
        let marks = self.title_marks.clone();
        let card = div()
            .relative()
            .flex()
            .flex_row()
            .flex_wrap()
            .max_w(room)
            .items_center()
            .gap(px(GAP))
            .p(px(4.))
            .pl(px(CHEVRON_W))
            .rounded(px(12.))
            .bg(rgb(ground(&t)))
            .border_1()
            .border_color(rgb(t.body_border))
            .shadow(vec![gpui::BoxShadow { color: gpui::hsla(0., 0., 0., 0.18), offset: point(px(0.), px(2.)), blur_radius: px(8.), spread_radius: px(0.), inset: false }])
            .font_family(crate::fonts::ui())
            .text_size(px(SIZE))
            .child(self.overlay_mark_by(px(0.)))
            .child(mark(move |b| marks.borrow_mut().procs_fan = Some(b)))
            .children(procs.iter().map(|(id, name)| self.pill(name, *id, 1, true, cx)));
        // (anchored's own way, past the window's edge, is to hang the card
        // the other way from the point -- leftwards over the path and the
        // window's buttons; snapped, it moves only as far as it must)
        deferred(anchored().snap_to_window_with_margin(px(FAN_MARGIN)).position(point(at.x, at.y - px(5.))).child(card)).with_priority(2).into_any_element()
    }

    /// A pill: the name, how many when there are `n` (`id` the newest),
    /// and the × that ends a single one (`kill`; several are ended from
    /// the fan, one by one). B1 goes to its output, B3 to where it was
    /// run from.
    fn pill(&self, name: &str, id: Seq, n: usize, kill: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::theme::theme();
        let under = ground(&t);
        let marks = self.title_marks.clone();
        let x = div()
            .id(("pill-kill", id as usize))
            .flex_none()
            .w(px(KILL_W))
            .h(px(16.))
            .rounded(px(8.))
            .flex()
            .items_center()
            .justify_center()
            .text_color(rgb(mix(t.text_dim, under, 0.3)))
            .hover(move |s| s.text_color(rgb(t.text)))
            .child("×")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.kill_proc(id, cx);
                    cx.stop_propagation();
                }),
            );
        div()
            .id(("pill", id as usize))
            .flex_none()
            .relative()
            .h(px(20.))
            .pl(px(8.))
            .rounded(px(10.))
            .flex()
            .flex_row()
            .items_center()
            .bg(rgb(mix(t.text_dim, under, 0.9)))
            .border_1()
            .border_color(rgb(under))
            .text_color(rgb(t.text_dim))
            .cursor_default()
            .child(mark(move |b| marks.borrow_mut().pills.push((id, b))))
            .child(div().flex_none().child(name.to_string()))
            .when(n > 1, |d| d.child(div().flex_none().ml(px(4.)).text_size(px(SIZE - 1.)).font_weight(crate::fonts::weight(FontWeight::MEDIUM)).text_color(rgb(mix(t.text_dim, under, 0.35))).child(n.to_string())))
            .when(kill && n == 1, |d| d.child(x))
            .when(!(kill && n == 1), |d| d.child(div().flex_none().w(px(KILL_W - 4.))))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.proc_output(id, cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, _, cx| {
                    this.press_proc(crate::text_element::Atom::Proc(id), MouseButton::Right, cx);
                    cx.stop_propagation();
                }),
            )
    }

    /// The directory, cut short as `fit` has it; and, while the pointer is
    /// on a short one, the whole of it over the bar, its end where the
    /// short one's is.
    pub fn path_strip(&self, fit: crate::titlefit::PathFit, cx: &mut Context<Self>) -> Option<AnyElement> {
        let fit = if self.cwd_picker.is_some() { PathFit::Full } else { fit };
        let short = self.cwd_bar(fit, cx)?;
        let marks = self.title_marks.clone();
        // (at the least, still too long, it is cut at its start)
        let strip = div().flex_shrink(1.).min_w_0().overflow_hidden().relative().flex().flex_row().items_center().child(short).when(fit != PathFit::Full, |d| d.child(mark(move |b| marks.borrow_mut().path = Some(b))));
        let fan = (fit != PathFit::Full && self.title_fan == Some(Fan::Path)).then(|| {
            let t = crate::theme::theme();
            let at = self.title_marks.borrow().drawn_path.unwrap_or_default();
            let marks = self.title_marks.clone();
            let card = div()
                .relative()
                .flex()
                .flex_row()
                .items_center()
                .h(px(26.))
                .px(px(8.))
                .rounded(px(8.))
                .bg(rgb(ground(&t)))
                .border_1()
                .border_color(rgb(t.body_border))
                .shadow(vec![gpui::BoxShadow { color: gpui::hsla(0., 0., 0., 0.18), offset: point(px(0.), px(2.)), blur_radius: px(8.), spread_radius: px(0.), inset: false }])
                .child(self.overlay_mark_by(px(0.)))
                .child(mark(move |b| marks.borrow_mut().path_fan = Some(b)))
                .children(self.cwd_bar(PathFit::Full, cx));
            deferred(anchored().anchor(gpui::Anchor::TopRight).position(point(at.right() + px(9.), at.top() + (at.size.height - px(26.)) / 2.)).child(card)).with_priority(2).into_any_element()
        });
        Some(div().flex_shrink(1.).min_w_0().flex().child(strip).children(fan).into_any_element())
    }
}

/// The bar's marks, cleared each frame before its parts record them (the
/// last frame's kept for the fans, `Marks::drawn_procs`).
pub fn new_marks() -> Rc<RefCell<Marks>> {
    Rc::new(RefCell::new(Marks::default()))
}

pub fn clear(m: &Rc<RefCell<Marks>>) {
    let mut m = m.borrow_mut();
    m.drawn_procs = m.procs;
    m.drawn_path = m.path;
    m.procs = None;
    m.procs_fan = None;
    m.path = None;
    m.path_fan = None;
    m.pills.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fan_is_placed_by_where_the_bar_was_drawn_last() {
        // a frame clears the marks before the bar records them again, and
        // the fans are made in between: they go by the last frame's
        let m = new_marks();
        let b = Bounds::new(point(px(400.), px(6.)), gpui::size(px(120.), px(20.)));
        m.borrow_mut().procs = Some(b);
        m.borrow_mut().path = Some(b);
        clear(&m);
        assert_eq!((m.borrow().procs, m.borrow().drawn_procs, m.borrow().drawn_path), (None, Some(b), Some(b)));
        // a frame without them: nothing to place a fan by
        clear(&m);
        assert_eq!(m.borrow().drawn_procs, None);
    }

    #[test]
    fn the_bar_gives_way_path_first_then_the_pills() {
        let path = |f: PathFit| match f {
            PathFit::Full => 400.,
            PathFit::Short => 200.,
            PathFit::Last => 80.,
        };
        let procs = |f: ProcFit| match f {
            ProcFit::Full => 300.,
            ProcFit::Stack => 120.,
            ProcFit::Count => 40.,
        };
        assert_eq!(choose(2000., 200., path, procs), (PathFit::Full, ProcFit::Full));
        assert_eq!(choose(750., 200., path, procs), (PathFit::Short, ProcFit::Full));
        assert_eq!(choose(560., 200., path, procs), (PathFit::Short, ProcFit::Stack));
        assert_eq!(choose(420., 200., path, procs), (PathFit::Last, ProcFit::Stack));
        assert_eq!(choose(330., 200., path, procs), (PathFit::Last, ProcFit::Count));
        // no room at all: the least there is
        assert_eq!(choose(10., 200., path, procs), (PathFit::Last, ProcFit::Count));
    }

    #[test]
    fn processes_are_grouped_by_name_in_the_order_they_first_ran() {
        let p = |i: Seq, n: &str| (i, n.to_string());
        let g = groups(&[p(1, "lsp"), p(2, "Win"), p(3, "agent"), p(4, "Win"), p(5, "Win")]);
        assert_eq!(g, vec![("lsp".into(), vec![1]), ("Win".into(), vec![2, 4, 5]), ("agent".into(), vec![3])]);
    }

    #[test]
    fn a_short_path_keeps_its_last_crumbs() {
        assert_eq!(first_crumb(7, PathFit::Full), 0);
        assert_eq!(first_crumb(7, PathFit::Short), 5);
        assert_eq!(first_crumb(7, PathFit::Last), 6);
        assert_eq!(first_crumb(1, PathFit::Short), 0);
    }
}
