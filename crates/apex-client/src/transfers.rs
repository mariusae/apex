//! Files on their way, as Manifold's sidebar shows them (`Transfers`):
//! every upload going on in the app -- a drop on a session on another
//! host (`drops.rs`) -- listed at the foot of the sidebar with how far it
//! has got, how fast and how long it has to go, and stopped from there.
//! One shows once it has taken a moment (a quick one never does), a
//! finished one stays a few seconds, a failed one until it is put away.
//!
//! The work is a thread's (`upload`): each file sent to the host as
//! `PUT apex-drop:NAME` (`apex_server::drops`), paced so keys are not
//! left behind it, and where it landed there given back.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use apex_server::plane::IoPlane;

/// A transfer shows once it has taken this long.
pub const SHOW_AFTER: Duration = Duration::from_millis(300);
/// A finished one stays this long; a failed one until put away.
pub const KEEP_FINISHED: Duration = Duration::from_secs(6);

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Running,
    Finished,
    Failed(String),
    Stopped,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub id: u64,
    /// What goes ("Screenshot ….png", "3 files").
    pub title: String,
    /// Where to ("to box:~/.apex/drops").
    pub detail: String,
    pub done: u64,
    pub total: u64,
    pub state: State,
    pub started: Instant,
    pub ended: Option<Instant>,
    /// Bytes a second, smoothed.
    pub rate: f64,
    sample: Option<(Instant, u64)>,
    stop: Arc<AtomicBool>,
}

impl Item {
    pub fn running(&self) -> bool {
        self.state == State::Running
    }

    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            return if self.running() { 0. } else { 1. };
        }
        (self.done as f64 / self.total as f64).min(1.) as f32
    }

    /// Seconds left at the rate it goes, once that can be told.
    pub fn left(&self, now: Instant) -> Option<Duration> {
        if !self.running() || self.rate <= 0. || now.duration_since(self.started) < Duration::from_millis(1500) {
            return None;
        }
        Some(Duration::from_secs_f64(self.total.saturating_sub(self.done) as f64 / self.rate))
    }

    /// How it stands, in words: "1.2 of 3.4 MB · 2.1 MB/s · 3 s left",
    /// "Done", "Stopped", or why it failed.
    pub fn said(&self, now: Instant) -> String {
        match &self.state {
            State::Running => {
                let mut s = format!("{} of {}", size(self.done), size(self.total));
                if self.rate > 0. && now.duration_since(self.started) > Duration::from_millis(700) {
                    s.push_str(&format!(" · {}/s", size(self.rate as u64)));
                }
                if let Some(l) = self.left(now) {
                    s.push_str(&format!(" · {} left", span(l)));
                }
                s
            }
            State::Finished => format!("{} · done", size(self.total)),
            State::Failed(why) => why.clone(),
            State::Stopped => "Stopped".into(),
        }
    }
}

/// A size as people read one: "512 bytes", "3.4 KB", "1.2 MB", "2.0 GB".
pub fn size(n: u64) -> String {
    let n = n as f64;
    if n < 1000. {
        format!("{} bytes", n as u64)
    } else if n < 1e6 {
        format!("{:.1} KB", n / 1e3)
    } else if n < 1e9 {
        format!("{:.1} MB", n / 1e6)
    } else {
        format!("{:.1} GB", n / 1e9)
    }
}

/// A time left: "3 s", "2 min", "1 h 5 min".
pub fn span(d: Duration) -> String {
    let s = d.as_secs_f64().ceil() as u64;
    if s < 60 {
        format!("{s} s")
    } else if s < 3600 {
        format!("{} min", s.div_ceil(60))
    } else {
        format!("{} h {} min", s / 3600, (s % 3600) / 60)
    }
}

/// Every transfer in the app.
static ITEMS: Mutex<Vec<Item>> = Mutex::new(Vec::new());

/// A transfer begun: its id, and the flag that stops it.
pub fn begin(title: String, detail: String, total: u64) -> (u64, Arc<AtomicBool>) {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let stop = Arc::new(AtomicBool::new(false));
    ITEMS.lock().unwrap().push(Item { id, title, detail, done: 0, total, state: State::Running, started: Instant::now(), ended: None, rate: 0., sample: None, stop: stop.clone() });
    (id, stop)
}

/// How far it has got; the rate measured anew each half second or so.
pub fn progress(id: u64, done: u64) {
    let now = Instant::now();
    let mut items = ITEMS.lock().unwrap();
    let Some(it) = items.iter_mut().find(|i| i.id == id) else { return };
    it.done = done;
    match it.sample {
        None => it.sample = Some((now, done)),
        Some((t, d)) => {
            let dt = now.duration_since(t).as_secs_f64();
            if dt >= 0.5 {
                let r = done.saturating_sub(d) as f64 / dt;
                it.rate = if it.rate == 0. { r } else { 0.7 * it.rate + 0.3 * r };
                it.sample = Some((now, done));
            }
        }
    }
}

/// It ended, so.
pub fn end(id: u64, state: State) {
    let mut items = ITEMS.lock().unwrap();
    if let Some(it) = items.iter_mut().find(|i| i.id == id) {
        if state == State::Finished {
            it.done = it.total;
        }
        it.state = state;
        it.ended = Some(Instant::now());
    }
}

/// One transfer as it stands, while it is kept.
pub fn item(id: u64) -> Option<Item> {
    ITEMS.lock().unwrap().iter().find(|i| i.id == id).cloned()
}

/// Stop it (from its × while it runs).
pub fn stop(id: u64) {
    if let Some(it) = ITEMS.lock().unwrap().iter().find(|i| i.id == id) {
        it.stop.store(true, Ordering::Relaxed);
    }
}

/// Put it away (its × once it ended).
pub fn dismiss(id: u64) {
    ITEMS.lock().unwrap().retain(|i| i.id != id || i.running());
}

/// What the sidebar shows: those going long enough to be worth it, and
/// those that ended and have not gone yet (a failure until put away). The
/// rest are forgotten.
pub fn visible(now: Instant) -> Vec<Item> {
    let mut items = ITEMS.lock().unwrap();
    items.retain(|i| match (&i.state, i.ended) {
        (State::Running, _) => true,
        (State::Failed(_), _) => true,
        (_, Some(e)) => now.duration_since(e) < KEEP_FINISHED,
        _ => false,
    });
    items.iter().filter(|i| !i.running() || now.duration_since(i.started) >= SHOW_AFTER).cloned().collect()
}

/// Whether anything is shown, or soon will be, that changes as it goes:
/// the sidebar is drawn again while so.
pub fn live() -> bool {
    ITEMS.lock().unwrap().iter().any(|i| i.running() || i.ended.is_some_and(|e| e.elapsed() < KEEP_FINISHED))
}

/// A file's key among the drops (`apex_server::drops`): of its name, size
/// and when it changed, so the same file dropped again is the one there.
/// Not of its contents: a file changed with its size and modified time
/// put back (`cp -p`, `rsync -t`, `touch -r`) is taken for the one there
/// and not sent again. Known and kept: hashing the contents would read
/// the whole file before a drop could start (a second a gigabyte).
pub fn key(path: &Path, len: u64) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.file_name().hash(&mut h);
    len.hash(&mut h);
    if let Ok(t) = std::fs::metadata(path).and_then(|m| m.modified()) {
        t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0).hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

/// What an upload comes to: where each file landed on the host, in the
/// order given; or why not (stopped, or failed).
pub type Landed = Result<Vec<String>, String>;

/// Send `files` (this Mac's) to the host `plane` reaches, to its drops
/// (`apex_server::drops`), on a thread, as a transfer the sidebar shows.
/// Its id, and where the outcome comes.
pub fn upload(plane: IoPlane, files: Vec<PathBuf>, host: &str) -> (u64, Receiver<Landed>) {
    let lens: Vec<u64> = files.iter().map(|f| std::fs::metadata(f).map(|m| m.len()).unwrap_or(0)).collect();
    let total = lens.iter().sum();
    let title = match files.as_slice() {
        [one] => one.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        many => format!("{} files", many.len()),
    };
    crate::shell::log_line(&format!("drop: {title} ({}) to {host}", size(total)));
    let (id, stop) = begin(title, format!("to {host}:~/.apex/drops"), total);
    let (tx, rx) = channel();
    let t0 = Instant::now();
    let said = move |how: &str| crate::shell::log_line(&format!("drop {id}: {how} after {} ms", t0.elapsed().as_millis()));
    std::thread::spawn(move || {
        let mut base = 0u64;
        let mut landed = Vec::new();
        for (f, len) in files.iter().zip(&lens) {
            let key = key(f, *len);
            match plane.drop_file(f, *len, &key, &stop, &mut |n| progress(id, base + n)) {
                Ok(at) => landed.push(at),
                Err(e) => {
                    let stopped = stop.load(Ordering::Relaxed);
                    said(&if stopped { "stopped".to_string() } else { format!("failed: {e}") });
                    end(id, if stopped { State::Stopped } else { State::Failed(e.clone()) });
                    let _ = tx.send(Err(if stopped { "stopped".into() } else { e }));
                    return;
                }
            }
            base += len;
            progress(id, base);
        }
        said(&format!("landed at {}", landed.join(" ")));
        end(id, State::Finished);
        let _ = tx.send(Ok(landed));
    });
    (id, rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_spans_read_as_people_say_them() {
        assert_eq!(size(512), "512 bytes");
        assert_eq!(size(3_400), "3.4 KB");
        assert_eq!(size(1_234_567), "1.2 MB");
        assert_eq!(size(2_000_000_000), "2.0 GB");
        assert_eq!(span(Duration::from_millis(2300)), "3 s");
        assert_eq!(span(Duration::from_secs(90)), "2 min");
        assert_eq!(span(Duration::from_secs(3900)), "1 h 5 min");
    }

    #[test]
    fn a_transfer_shows_after_a_moment_and_goes_after_a_while() {
        let (id, stop) = begin("t.bin".into(), "to box:~/.apex/drops".into(), 1000);
        let now = Instant::now();
        assert!(!visible(now).iter().any(|i| i.id == id), "a quick one never shows");
        assert!(visible(now + SHOW_AFTER).iter().any(|i| i.id == id && i.running()));
        progress(id, 400);
        assert!(visible(now + SHOW_AFTER).iter().any(|i| i.id == id && i.done == 400 && (i.fraction() - 0.4).abs() < 1e-6));
        stop_and_check(id, &stop);
        end(id, State::Finished);
        let later = Instant::now();
        assert!(visible(later).iter().any(|i| i.id == id && i.state == State::Finished && i.done == 1000));
        assert!(!visible(later + KEEP_FINISHED + Duration::from_secs(1)).iter().any(|i| i.id == id), "gone a while after");
        // a failure stays until put away
        let (id, _) = begin("f.bin".into(), String::new(), 10);
        end(id, State::Failed("no room".into()));
        assert!(visible(Instant::now() + Duration::from_secs(3600)).iter().any(|i| i.id == id));
        dismiss(id);
        assert!(!visible(Instant::now()).iter().any(|i| i.id == id));
    }

    fn stop_and_check(id: u64, flag: &AtomicBool) {
        stop(id);
        assert!(flag.load(Ordering::Relaxed));
    }

    #[test]
    fn the_same_file_has_the_same_key_and_another_another() {
        let d = std::env::temp_dir().join(format!("apex-key-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let (a, b) = (d.join("a.png"), d.join("b.png"));
        std::fs::write(&a, b"one").unwrap();
        std::fs::write(&b, b"one").unwrap();
        assert_eq!(key(&a, 3), key(&a, 3));
        assert_ne!(key(&a, 3), key(&b, 3), "another name");
        assert_ne!(key(&a, 3), key(&a, 4), "another length");
        assert!(apex_server::drops::valid_key(&key(&a, 3)));
        let _ = std::fs::remove_dir_all(&d);
    }
}

use gpui::{div, prelude::*, px, rgb, Context, FontWeight, MouseButton};

impl crate::app::Acme {
    /// The transfers going on, at the foot of the sidebar's card: each
    /// its name and an × (stopping it while it goes, putting it away once
    /// it ended), where to, a bar for how far it has got and how it
    /// stands in words. Nothing while there are none.
    pub fn transfers_foot(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let now = Instant::now();
        let items = visible(now);
        if items.is_empty() {
            return None;
        }
        let t = crate::theme::theme();
        let mut foot = div().flex_none().flex().flex_col().gap(px(8.)).mx(px(6.)).mt(px(6.)).pt(px(8.)).px(px(8.)).border_t_1().border_color(rgb(t.border));
        for it in items {
            let id = it.id;
            let running = it.running();
            let failed = matches!(it.state, State::Failed(_));
            let x = div()
                .id(("transfer-x", id))
                .flex_none()
                .size(px(16.))
                .rounded(px(4.))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(13.))
                .text_color(rgb(t.text_dim))
                .hover(move |s| s.bg(rgb(crate::theme::step(t.strip, 2))).text_color(rgb(t.text)))
                .child("×")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |_, _, _, cx| {
                        if running {
                            stop(id);
                        } else {
                            dismiss(id);
                        }
                        cx.notify();
                        cx.stop_propagation();
                    }),
                );
            let bar = div()
                .h(px(3.))
                .w_full()
                .rounded(px(2.))
                .bg(rgb(crate::theme::step(t.strip, 2)))
                .child(div().h_full().rounded(px(2.)).w(gpui::relative(it.fraction())).bg(rgb(if failed { t.text_dim } else { t.accent })));
            foot = foot.child(
                div()
                    .id(("transfer", id))
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.))
                            .child(div().flex_none().text_size(px(12.)).text_color(rgb(t.text_dim)).child("↑"))
                            .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_size(px(12.)).font_weight(crate::fonts::weight(FontWeight::MEDIUM)).text_color(rgb(t.text)).child(it.title.clone()))
                            .child(x),
                    )
                    .child(div().overflow_hidden().text_ellipsis().whitespace_nowrap().text_size(px(11.)).text_color(rgb(t.text_dim)).child(it.detail.clone()))
                    .child(bar)
                    .child(div().overflow_hidden().text_ellipsis().whitespace_nowrap().text_size(px(11.)).text_color(rgb(if failed { t.text } else { t.text_dim })).child(it.said(now))),
            );
        }
        Some(foot.into_any_element())
    }
}
