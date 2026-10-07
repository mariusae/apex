//! Where this window stands with its session, said where it is seen. A
//! window leads its session (it holds the leases: what is typed takes),
//! watches it (another client leads, and nothing typed here takes), or
//! has lost it: stalled (the daemon has stopped answering, the link
//! still open), offline (the link closed, or an attach failed), or
//! coming back. Anything but leading is said by a chip after the
//! session's name -- a mark and a word, never colour alone -- and by a
//! banner under the title bar saying what it means and what to do:
//! watching names the client that leads and offers Take over, which is
//! never done for the user (two clients each taking the lead back is
//! worse than one asking). A lost link is attached again by itself,
//! sooner and then less often, and at once after the Mac wakes; but a
//! link that is only stalled while edits made here are not yet in the
//! daemon is waited for, as attaching again would drop them, and the
//! banner says which windows they are in. Typing while watching shows
//! the banner again rather than doing nothing silently, and the carets
//! are drawn hollow.

use std::time::{Duration, Instant};

use gpui::{div, prelude::*, px, rgb, AnyElement, Context, FontWeight, MouseButton, Window};

use apex_core::{Shard, SERVER};

use crate::app::{Acme, Backend};
use crate::pool::{Pool, State};
use crate::text_element::{ground, mix};

/// Where the window stands.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Standing {
    /// It leads its session (or runs it in process): nothing to say.
    Leading,
    /// Another client leads: its attachment's name; none when nobody
    /// does (the leases let go and not taken).
    Watching { leader: Option<String> },
    /// The daemon has not answered for a while; the link is open.
    Stalled,
    /// No link: it closed, or attaching failed, and why.
    Offline { why: Option<String> },
    /// A link is being made.
    Coming,
}

impl Standing {
    /// The word the chip, the tabs and the sidebar say.
    pub fn word(&self) -> Option<&'static str> {
        match self {
            Standing::Leading => None,
            Standing::Watching { .. } => Some("watching"),
            Standing::Stalled => Some("stalled"),
            Standing::Offline { .. } => Some("offline"),
            Standing::Coming => Some("connecting…"),
        }
    }
}

/// When the next attach is tried, and how many have been.
#[derive(Clone, Copy, Debug)]
pub struct Retry {
    pub at: Instant,
    pub step: usize,
}

/// Seconds between attaches tried by themselves: soon, then less often.
const BACKOFF: [u64; 6] = [1, 2, 5, 10, 20, 30];
/// A stalled link given up on, this long after the daemon last answered.
const STALL_GIVE_UP: Duration = Duration::from_secs(15);
/// The tick not run for this long: the Mac slept.
const WAKE_GAP: Duration = Duration::from_secs(5);
/// After waking, an answer this soon or the link is taken for dead.
const WAKE_PROBE: Duration = Duration::from_secs(2);
/// How long typing while watching shows the banner's flash.
const FLASH: Duration = Duration::from_millis(700);
/// How long the note of edits a reconnect dropped stays.
const LOST_NOTE: Duration = Duration::from_secs(20);

/// The next try, `step` tries in.
pub fn backoff(step: usize) -> Duration {
    Duration::from_secs(BACKOFF[step.min(BACKOFF.len() - 1)])
}

/// An attach that failed for a reason trying again will not mend: a
/// daemon of another build, or a tab let go.
pub fn hopeless(why: &str) -> bool {
    why.contains("speaks apex protocol") || why.starts_with("let go")
}

impl Acme {
    /// Where this window stands with its session.
    pub fn standing(&self, cx: &gpui::App) -> Standing {
        if self.waiting.is_some() {
            return match Pool::state(cx, self.tab) {
                State::Down(why) => Standing::Offline { why: Some(why) },
                _ => Standing::Coming,
            };
        }
        if !matches!(self.backend, Backend::Remote(_)) {
            return Standing::Leading;
        }
        if !self.connected {
            return if self.link_closed { Standing::Offline { why: None } } else { Standing::Stalled };
        }
        if self.fenced() {
            return Standing::Watching { leader: self.leader() };
        }
        Standing::Leading
    }

    /// Who leads this session when this window does not: the name of the
    /// attachment holding the layout's lease.
    fn leader(&self) -> Option<String> {
        let l = self.log.lease(Shard::Layout)?;
        if l.released.is_some() || l.holder == SERVER {
            return None;
        }
        Some(self.node.state.meta.attachments.get(&l.holder).map(|a| a.name.clone()).unwrap_or_else(|| format!("client {}", l.holder)))
    }

    /// What this window wrote that the daemon has not yet said it has:
    /// for each shard it leads, how many entries; named as the user
    /// knows them.
    pub fn unconfirmed(&self) -> Vec<(String, u64)> {
        let Backend::Remote(link) = &self.backend else { return Vec::new() };
        let me = self.node.attachment;
        let mut out = Vec::new();
        for (shard, l) in &self.node.state.meta.leases {
            if l.holder != me || l.released.is_some() {
                continue;
            }
            let ahead = self.log.last_seq(*shard).saturating_sub(link.acked.get(shard).copied().unwrap_or(0));
            if ahead == 0 {
                continue;
            }
            let name = match shard {
                Shard::Buffer(b) => self.node.state.buffer(*b).ok().map(|b| b.name.rsplit('/').next().unwrap_or(&b.name).to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| "a window".into()),
                Shard::Window(w) => self.node.window_path(*w).rsplit('/').next().unwrap_or("a window").to_string(),
                _ => "the layout".into(),
            };
            out.push((name, ahead));
        }
        out
    }

    /// The windows in `unconfirmed`, said once each.
    fn unconfirmed_said(&self) -> Option<String> {
        let mut names: Vec<String> = self.unconfirmed().into_iter().map(|(n, _)| n).collect();
        names.dedup();
        if names.is_empty() {
            return None;
        }
        let more = names.len().saturating_sub(3);
        names.truncate(3);
        Some(if more > 0 { format!("{} and {more} more", names.join(", ")) } else { names.join(", ") })
    }

    /// Attach this window again (Reconnect, ⌘⇧R, the banner's buttons):
    /// asked first when that would drop edits a link that is still open
    /// may yet deliver; said after when a closed one has dropped them.
    pub fn ask_reconnect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let lost = self.unconfirmed_said();
        if lost.is_some() && !self.link_closed && !self.confirm_drop && self.waiting.is_none() {
            self.confirm_drop = true;
            cx.notify();
            return;
        }
        self.reconnect_now(lost, window, cx);
    }

    /// Take the lead from the client that has it: attach again, as the
    /// daemon lets the latest UI lead. Only ever asked for.
    pub fn take_over(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::shell::log_line(&format!("{}: taking over from {}", self.url, self.leader().unwrap_or_else(|| "nobody".into())));
        self.reconnect_now(None, window, cx);
    }

    fn reconnect_now(&mut self, lost: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_drop = false;
        if let Some(names) = lost {
            self.lost_note = Some((format!("Edits in {names} had not reached the daemon and were dropped; look them over."), Instant::now()));
        }
        self.reconnect(window, cx);
        cx.notify();
    }

    /// The tick's part (every 100 ms): a wake noticed, a lost link
    /// attached again when its time comes. Whether to draw again.
    pub fn link_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.wake.is_none() {
            return false; // in process: no link to lose
        }
        let now = Instant::now();
        let mut changed = false;
        // the tick has not run for a while: the Mac slept, and the link
        // may be dead without saying so. Asked now, not at the next beat
        let slept = self.last_tick.is_some_and(|t| now.duration_since(t) > WAKE_GAP);
        self.last_tick = Some(now);
        if slept && self.connected && self.waiting.is_none() {
            if let Backend::Remote(link) = &mut self.backend {
                link.send(&apex_server::proto::ClientMsg::Ping { t: 0 });
            }
            self.last_ping = Some(now);
            self.woke = Some(now);
        }
        let pong = match &self.backend {
            Backend::Remote(link) => link.last_pong,
            _ => None,
        };
        if let Some(w) = self.woke {
            if pong.is_some_and(|p| p >= w) {
                self.woke = None;
            } else if now.duration_since(w) > WAKE_PROBE {
                // no answer after waking: dead; attached again at once
                self.woke = None;
                self.connected = false;
                if self.unconfirmed().is_empty() || self.link_closed {
                    self.retry = Some(Retry { at: now, step: 0 });
                }
                changed = true;
            }
        }
        let standing = self.standing(cx);
        match &standing {
            Standing::Leading | Standing::Watching { .. } => {
                if self.retry.take().is_some() {
                    changed = true;
                }
            }
            Standing::Coming => {}
            Standing::Stalled => {
                let quiet = pong.or(self.last_ping).is_some_and(|p| now.duration_since(p) > STALL_GIVE_UP);
                if quiet && self.retry.is_none() && self.unconfirmed().is_empty() {
                    self.retry = Some(Retry { at: now, step: 0 });
                }
            }
            Standing::Offline { why } => {
                if why.as_deref().is_some_and(hopeless) {
                    self.retry = None;
                } else if self.retry.is_none() {
                    self.retry = Some(Retry { at: now + backoff(0), step: 0 });
                    changed = true;
                }
            }
        }
        if let Some(r) = self.retry {
            if now >= r.at && standing != Standing::Coming {
                let lost = if self.waiting.is_none() { self.unconfirmed_said() } else { None };
                crate::shell::log_line(&format!("{}: attaching again by itself (try {})", self.url, r.step + 1));
                self.reconnect_now(lost, window, cx);
                self.retry = Some(Retry { at: now + backoff(r.step + 1), step: r.step + 1 });
                changed = true;
            }
        }
        // the countdown, said in seconds, goes on being said
        if self.retry.is_some() {
            changed = true;
        }
        // asked whether to drop edits that have since arrived: no question
        if self.confirm_drop && (self.connected || self.unconfirmed().is_empty()) {
            self.confirm_drop = false;
            changed = true;
        }
        if self.fence_flash.is_some_and(|f| now.duration_since(f) < FLASH + Duration::from_millis(200)) {
            changed = true;
        }
        if self.lost_note.as_ref().is_some_and(|(_, at)| now.duration_since(*at) > LOST_NOTE) {
            self.lost_note = None;
            changed = true;
        }
        changed
    }

    /// Typing or clicking while watching: the banner says so again.
    pub fn flash_watching(&mut self, cx: &mut Context<Self>) {
        if self.fenced() {
            self.fence_flash = Some(Instant::now());
            cx.notify();
        }
    }

    /// When the next try is, said: "in 4 s", or "now".
    fn retry_said(&self) -> Option<String> {
        let r = self.retry?;
        let left = r.at.saturating_duration_since(Instant::now()).as_secs_f32().ceil() as u64;
        Some(if left == 0 { "now".into() } else { format!("in {left} s") })
    }

    /// The chip after the session's name: a mark and the word, while the
    /// window does anything but lead. A click opens its card.
    pub fn standing_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let standing = self.standing(cx);
        let word = standing.word()?;
        let t = crate::theme::theme();
        let (mark, tint) = self.standing_look(&standing);
        let under = ground(&t);
        Some(
            div()
                .id("standing-chip")
                .flex_none()
                .h(px(20.))
                .px(px(7.))
                .rounded(px(10.))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.))
                .bg(rgb(mix(tint, under, 0.82)))
                .text_size(px(12.))
                .font_weight(crate::fonts::weight(FontWeight::MEDIUM))
                .text_color(rgb(t.text))
                .cursor_default()
                .child(mark)
                .child(word)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.standing_card = !this.standing_card;
                        cx.notify();
                        cx.stop_propagation();
                    }),
                )
                .into_any_element(),
        )
    }

    /// A standing's mark and its colour: an eye in blue for watching, a
    /// broken link in gold for a lost one -- the blue-yellow axis, which
    /// red-green colour blindness keeps apart -- and the spinner while
    /// one comes.
    fn standing_look(&self, s: &Standing) -> (AnyElement, u32) {
        let t = crate::theme::theme();
        // fixed, not the palette's accent (one palette's accent is a gold);
        // checked under a deuteranopia simulation: the marks ΔE ≈ 130 apart,
        // the chips and banners 20 to 30, the text on them 4:1 or better
        let dark = crate::theme::is_dark();
        let (blue, gold) = if dark { (0x539BF5, 0xE6B422) } else { (0x0969DA, 0xB88A00) };
        match s {
            Standing::Watching { .. } => (eye(blue).into_any_element(), blue),
            Standing::Coming => (spinner(t.text_dim).into_any_element(), t.text_dim),
            _ => (broken(gold).into_any_element(), gold),
        }
    }

    /// The banner under the title bar: what the standing means and what
    /// to do about it; or, after a reconnect dropped edits, which.
    pub fn standing_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.waiting.is_some() {
            return None; // the page says it, whole
        }
        let standing = self.standing(cx);
        let (mark, tint) = self.standing_look(&standing);
        let lost = self.unconfirmed_said();
        let (say, buttons): (String, Vec<(&'static str, Action)>) = if self.confirm_drop {
            (format!("Attaching again drops edits the daemon has not yet had, in {}.", lost.clone().unwrap_or_else(|| "this window".into())), vec![("Reconnect anyway", Action::ReconnectAnyway), ("Wait", Action::Wait)])
        } else {
            match &standing {
                Standing::Leading | Standing::Coming => {
                    let (note, _) = self.lost_note.as_ref()?;
                    let (mark, tint) = self.standing_look(&Standing::Offline { why: None });
                    return Some(self.banner(mark, tint, note.clone(), vec![("Dismiss", Action::Dismiss)], false, cx));
                }
                Standing::Watching { leader } => (
                    match leader {
                        Some(who) => format!("Watching: {who} leads this session. What is typed here does not take."),
                        None => "Watching: nobody leads this session. What is typed here does not take.".into(),
                    },
                    vec![("Take over", Action::TakeOver)],
                ),
                Standing::Stalled => {
                    let mut s = "The daemon is not answering.".to_string();
                    match (&lost, self.retry_said()) {
                        (Some(names), _) => s.push_str(&format!(" Waiting for it: edits in {names} have not reached it yet.")),
                        (None, Some(when)) => s.push_str(&format!(" Attaching again {when}.")),
                        (None, None) => s.push_str(" Waiting for it."),
                    }
                    (s, vec![("Reconnect", Action::Reconnect)])
                }
                Standing::Offline { .. } => {
                    let mut s = "Offline: the link to the daemon closed.".to_string();
                    if let Some(when) = self.retry_said() {
                        s.push_str(&format!(" Attaching again {when}."));
                    }
                    if let Some(names) = &lost {
                        s.push_str(&format!(" Edits in {names} did not reach it."));
                    }
                    (s, vec![("Reconnect now", Action::Reconnect)])
                }
            }
        };
        let flash = self.fence_flash.is_some_and(|f| f.elapsed() < FLASH) && matches!(standing, Standing::Watching { .. });
        Some(self.banner(mark, tint, say, buttons, flash, cx))
    }

    fn banner(&self, mark: AnyElement, tint: u32, say: String, buttons: Vec<(&'static str, Action)>, flash: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = crate::theme::theme();
        let bg = mix(tint, t.body_bg, if flash { 0.62 } else { 0.86 });
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .max_w(px(760.))
            .px(px(12.))
            .py(px(6.))
            .rounded(px(9.))
            .bg(rgb(bg))
            .border_1()
            .border_color(rgb(mix(tint, t.body_bg, 0.55)))
            .shadow(vec![gpui::BoxShadow { color: gpui::hsla(0., 0., 0., 0.14), offset: gpui::point(px(0.), px(2.)), blur_radius: px(8.), spread_radius: px(0.), inset: false }])
            .font_family(crate::fonts::ui())
            .text_size(px(13.))
            .text_color(rgb(t.text))
            .child(self.overlay_mark_by(px(0.)))
            .child(mark)
            .child(div().flex_1().min_w_0().child(say));
        for (i, (label, act)) in buttons.into_iter().enumerate() {
            let primary = i == 0;
            row = row.child(
                div()
                    .id(("standing-button", i))
                    .flex_none()
                    .h(px(24.))
                    .px(px(10.))
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .font_weight(crate::fonts::weight(if primary { FontWeight::SEMIBOLD } else { FontWeight::NORMAL }))
                    .when(primary, |d| d.bg(rgb(t.accent)).text_color(rgb(0xFFFFFF)))
                    .when(!primary, |d| d.border_1().border_color(rgb(mix(tint, t.body_bg, 0.4))))
                    .cursor_default()
                    .child(label)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.standing_act(act, window, cx);
                            cx.stop_propagation();
                        }),
                    ),
            );
        }
        div().absolute().top(px(crate::title_h() + 6.)).left(px(0.)).w_full().flex().justify_center().child(row).into_any_element()
    }

    fn standing_act(&mut self, act: Action, window: &mut Window, cx: &mut Context<Self>) {
        match act {
            Action::TakeOver => self.take_over(window, cx),
            Action::Reconnect => self.ask_reconnect(window, cx),
            Action::ReconnectAnyway => {
                let lost = self.unconfirmed_said();
                self.reconnect_now(lost, window, cx);
            }
            Action::Wait => self.confirm_drop = false,
            Action::Dismiss => self.lost_note = None,
            Action::RestartDaemon => self.restart_server(window, cx),
        }
        self.standing_card = false;
        cx.notify();
    }

    /// The chip's card: the session, its daemon, how it answers, who
    /// leads, and what can be done.
    pub fn standing_card_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.standing_card {
            return None;
        }
        let standing = self.standing(cx);
        let t = crate::theme::theme();
        let dim = |s: String| div().text_color(rgb(t.panel_dim)).child(s);
        let meta = &self.node.state.meta;
        let mut card = div()
            .id("standing-card")
            .absolute()
            .top(px(crate::title_h() - 4.))
            .left(px(0.))
            .w(px(320.))
            .p(px(10.))
            .rounded(px(9.))
            .bg(rgb(t.panel_bg))
            .border_1()
            .border_color(rgb(t.panel_border))
            .shadow(vec![gpui::BoxShadow { color: gpui::hsla(0., 0., 0., 0.18), offset: gpui::point(px(0.), px(6.)), blur_radius: px(18.), spread_radius: px(0.), inset: false }])
            .flex()
            .flex_col()
            .gap(px(4.))
            .text_size(px(12.5))
            .text_color(rgb(t.panel_text))
            .child(self.overlay_mark())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().font_weight(crate::fonts::weight(FontWeight::SEMIBOLD)).child(self.url.describe()))
            .when(!meta.host.is_empty(), |d| d.child(dim(format!("daemon on {}", meta.host))));
        let state = match &standing {
            Standing::Leading => "Leading: this window's edits take.".to_string(),
            Standing::Watching { leader: Some(who) } => format!("Watching: {who} leads."),
            Standing::Watching { leader: None } => "Watching: nobody leads.".into(),
            Standing::Stalled => "Stalled: the daemon is not answering.".into(),
            Standing::Offline { why: Some(why) } => format!("Offline: {why}"),
            Standing::Offline { why: None } => "Offline: the link closed.".into(),
            Standing::Coming => "Attaching…".into(),
        };
        card = card.child(div().child(state));
        if let Some(ms) = self.ping_ms.filter(|_| self.connected) {
            card = card.child(dim(format!("answers in {ms} ms")));
        }
        if let Some(when) = self.retry_said() {
            card = card.child(dim(format!("attaching again {when}")));
        }
        if let Some(names) = self.unconfirmed_said() {
            card = card.child(dim(format!("not yet in the daemon: edits in {names}")));
        }
        let mut acts: Vec<(&'static str, Action)> = Vec::new();
        if matches!(standing, Standing::Watching { .. }) {
            acts.push(("Take over", Action::TakeOver));
        }
        if self.wake.is_some() {
            acts.push(("Reconnect", Action::Reconnect));
        }
        if matches!(&standing, Standing::Offline { why: Some(w) } if w.contains("speaks apex protocol")) {
            acts.push(("Restart daemon", Action::RestartDaemon));
        }
        let mut row = div().flex().flex_row().gap(px(6.)).mt(px(6.));
        for (i, (label, act)) in acts.into_iter().enumerate() {
            row = row.child(
                div()
                    .id(("standing-card-button", i))
                    .h(px(24.))
                    .px(px(10.))
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .when(i == 0, |d| d.bg(rgb(t.accent)).text_color(rgb(0xFFFFFF)).font_weight(crate::fonts::weight(FontWeight::SEMIBOLD)))
                    .when(i > 0, |d| d.border_1().border_color(rgb(t.panel_border)))
                    .cursor_default()
                    .child(label)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.standing_act(act, window, cx);
                            cx.stop_propagation();
                        }),
                    ),
            );
        }
        Some(gpui::deferred(card.child(row)).with_priority(2).into_any_element())
    }

    /// The page shown while there is no link: under what it says, when
    /// the next try is and the buttons -- Reconnect now, and Restart
    /// daemon for one of another build.
    pub fn waiting_actions(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Standing::Offline { why } = self.standing(cx) else { return None };
        let t = crate::theme::theme();
        let wrong_build = why.as_deref().is_some_and(|w| w.contains("speaks apex protocol"));
        let mut acts = vec![("Reconnect now", Action::Reconnect)];
        if wrong_build {
            acts.push(("Restart daemon", Action::RestartDaemon));
        }
        let mut row = div().flex().flex_row().gap(px(8.)).items_center().font_family(crate::fonts::ui()).text_size(px(13.));
        if let Some(when) = self.retry_said() {
            row = row.child(div().text_color(rgb(t.text_dim)).child(format!("Trying again {when}.")));
        }
        for (i, (label, act)) in acts.into_iter().enumerate() {
            row = row.child(
                div()
                    .id(("waiting-button", i))
                    .h(px(26.))
                    .px(px(12.))
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .when(i == 0, |d| d.bg(rgb(t.accent)).text_color(rgb(0xFFFFFF)).font_weight(crate::fonts::weight(FontWeight::SEMIBOLD)))
                    .when(i > 0, |d| d.border_1().border_color(rgb(t.body_border)).text_color(rgb(t.text)))
                    .cursor_default()
                    .child(label)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.standing_act(act, window, cx);
                            cx.stop_propagation();
                        }),
                    ),
            );
        }
        Some(row.into_any_element())
    }
}

/// What a banner's or a card's button does.
#[derive(Clone, Copy, Debug)]
enum Action {
    TakeOver,
    Reconnect,
    ReconnectAnyway,
    Wait,
    Dismiss,
    RestartDaemon,
}

/// An eye, 14 across: watching.
fn eye(ink: u32) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let c = b.center();
            let ink: gpui::Hsla = rgb(ink).into();
            let mut p = gpui::PathBuilder::stroke(px(1.4));
            p.move_to(gpui::point(c.x - px(6.), c.y));
            p.curve_to(gpui::point(c.x + px(6.), c.y), gpui::point(c.x, c.y - px(7.)));
            p.curve_to(gpui::point(c.x - px(6.), c.y), gpui::point(c.x, c.y + px(7.)));
            if let Ok(path) = p.build() {
                window.paint_path(path, ink);
            }
            let r = px(2.);
            window.paint_quad(gpui::fill(gpui::Bounds::new(gpui::point(c.x - r, c.y - r), gpui::size(r * 2., r * 2.)), ink).corner_radii(r));
        },
    )
    .flex_none()
    .size(px(14.))
}

/// Two links of a chain apart, 14 across: the link lost.
fn broken(ink: u32) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let c = b.center();
            let ink: gpui::Hsla = rgb(ink).into();
            for (dx, dy) in [(-3.0f32, 2.0f32), (3.0, -2.0)] {
                let o = gpui::Bounds::new(gpui::point(c.x + px(dx) - px(3.5), c.y + px(dy) - px(2.5)), gpui::size(px(7.), px(5.)));
                window.paint_quad(gpui::quad(o, px(2.5), gpui::transparent_black(), px(1.4), ink, gpui::BorderStyle::Solid));
            }
        },
    )
    .flex_none()
    .size(px(14.))
}

/// The spinner, small: a link on its way.
fn spinner(ink: u32) -> impl IntoElement {
    gpui::canvas(|_, _, _| {}, move |b, _, window, _| crate::text_element::paint_spinner(window, b.center(), 5., 1.5, rgb(ink).into())).flex_none().size(px(14.))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tries_come_soon_then_less_often() {
        assert_eq!(backoff(0), Duration::from_secs(1));
        assert!(backoff(1) > backoff(0) && backoff(3) > backoff(2));
        assert_eq!(backoff(99), Duration::from_secs(30));
    }

    #[test]
    fn a_daemon_of_another_build_is_not_tried_again() {
        assert!(hopeless("the daemon speaks apex protocol 47 (build x), this is protocol 48 (build y); …"));
        assert!(hopeless("let go: 8 sessions are as many as stay attached"));
        assert!(!hopeless("connection refused"));
    }

    #[test]
    fn only_leading_says_nothing() {
        assert_eq!(Standing::Leading.word(), None);
        assert_eq!(Standing::Watching { leader: None }.word(), Some("watching"));
        assert_eq!(Standing::Offline { why: None }.word(), Some("offline"));
    }
}
