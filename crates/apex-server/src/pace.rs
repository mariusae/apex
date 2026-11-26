//! Pacing the I/O plane's bulk streams (a file on its way to the host),
//! after Manifold's `Mux`: one connection carries everything -- keys,
//! screens, a file -- and a key must not wait behind megabytes of a file
//! queued below us (ssh's window, the sockets' buffers).
//!
//! So a paced stream's bytes go out only as the link takes them: no more
//! of them on the way, not yet acknowledged by the daemon (`IoAck`, the
//! paced bytes it has taken in all), than a budget. The budget is steered
//! by the queue the round trips show, as LEDBAT steers: the shortest
//! round trip seen lately is the link's own; anything over it is a queue,
//! and the budget grows while that queue is under a target (15 ms, or a
//! fifth of the round trip) and the budget is what holds bytes back, and
//! shrinks while it is over -- within round trips, so a link that slows
//! (Wi-Fi, other traffic) is followed at once.
//!
//! `Pace` is the arithmetic, on a clock it is given (tests run it on a
//! link simulated); `Pacer` is it shared with the threads that send and
//! the reader that hears the acknowledgments.

use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// The least a piece carries, and so the least the budget is (two).
pub const LEAST_PIECE: usize = 2 << 10;
/// The most a piece carries: small enough that frames interleave.
pub const MOST_PIECE: usize = 64 << 10;
/// Where the budget starts.
const START: usize = 16 << 10;
/// The most it grows to.
const MOST: usize = 32 << 20;
/// The queue aimed for: enough that the link is kept busy, little enough
/// that a key behind it is hardly slowed.
const QUEUE_TARGET: Duration = Duration::from_millis(15);
/// How much of itself the budget may grow, or shrink, in a round trip.
const GROW: f64 = 0.5;
const SHRINK: f64 = 0.25;
/// How long a round trip stays the link's own (the base): a link whose
/// path changes is learnt again in this long.
const BASE_KEPT: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub struct Pace {
    /// Paced bytes sent, and acknowledged as taken, in all.
    sent: u64,
    acked: u64,
    budget: usize,
    /// Each piece on its way: where it ended in `sent`, when it went.
    marks: std::collections::VecDeque<(u64, Instant)>,
    /// The round trips measured, for the base (the least of the last
    /// minute's) and the current (the least of the last round trip's).
    rtts: std::collections::VecDeque<(Instant, Duration)>,
    /// The last wait was the budget's: it may grow only then.
    limited: bool,
}

impl Default for Pace {
    fn default() -> Pace {
        Pace { sent: 0, acked: 0, budget: START, marks: Default::default(), rtts: Default::default(), limited: false }
    }
}

impl Pace {
    pub fn budget(&self) -> usize {
        self.budget
    }

    pub fn in_flight(&self) -> u64 {
        self.sent - self.acked
    }

    /// How big the next piece may be (of what is left to send): a part of
    /// the budget, so several are on the way at once.
    pub fn piece(&self, left: usize) -> usize {
        left.min((self.budget / 4).clamp(LEAST_PIECE, MOST_PIECE))
    }

    /// Whether a piece of `n` bytes may go now. One always may when
    /// nothing is on the way, so a budget smaller than a piece never
    /// stops it for good.
    pub fn may_send(&mut self, n: usize) -> bool {
        let ok = self.in_flight() == 0 || self.in_flight() + n as u64 <= self.budget as u64;
        if !ok {
            self.limited = true;
        }
        ok
    }

    /// A piece of `n` bytes went at `now`.
    pub fn sent(&mut self, n: usize, now: Instant) {
        self.sent += n as u64;
        self.marks.push_back((self.sent, now));
    }

    /// The daemon has taken `total` paced bytes in all: round trips
    /// measured, the budget steered.
    pub fn acked(&mut self, total: u64, now: Instant) {
        if total <= self.acked {
            return;
        }
        let total = total.min(self.sent);
        let newly = total - self.acked;
        self.acked = total;
        while let Some(&(end, at)) = self.marks.front() {
            if end > total {
                break;
            }
            self.marks.pop_front();
            self.rtts.push_back((now, now.saturating_duration_since(at).max(Duration::from_micros(500))));
        }
        while self.rtts.front().is_some_and(|(t, _)| now.saturating_duration_since(*t) > BASE_KEPT) {
            self.rtts.pop_front();
        }
        let Some(base) = self.rtts.iter().map(|(_, r)| *r).min() else { return };
        let span = base.max(Duration::from_millis(50));
        let Some(current) = self.rtts.iter().filter(|(t, _)| now.saturating_duration_since(*t) <= span).map(|(_, r)| *r).min() else { return };
        let queue = current.saturating_sub(base).as_secs_f64();
        let target = QUEUE_TARGET.as_secs_f64().max(0.2 * base.as_secs_f64());
        let off = ((target - queue) / target).clamp(-1., 1.);
        let gain = if off > 0. { GROW } else { SHRINK };
        if off < 0. || self.limited {
            let by = gain * off * newly as f64;
            self.budget = (self.budget as f64 + by).clamp((2 * LEAST_PIECE) as f64, MOST as f64) as usize;
        }
        self.limited = false;
    }
}

/// `Pace` shared: threads wait in `room` for the budget, the reader says
/// the acknowledgments, and a link gone ends every wait.
#[derive(Default)]
pub struct Pacer {
    state: Mutex<(Pace, bool)>,
    changed: Condvar,
}

impl Pacer {
    /// Wait until a piece of up to `left` bytes may go: how big it may be
    /// (the caller sends what it has of that, and says so with `sent`).
    /// None when the link has gone, or `stop` says to stop.
    pub fn room(&self, left: usize, stop: &dyn Fn() -> bool) -> Option<usize> {
        let mut g = self.state.lock().unwrap();
        loop {
            if g.1 || stop() {
                return None;
            }
            let n = g.0.piece(left);
            if g.0.may_send(n) {
                return Some(n);
            }
            // a stop is looked at again now and then
            g = self.changed.wait_timeout(g, Duration::from_millis(100)).unwrap().0;
        }
    }

    /// A piece of `n` bytes went.
    pub fn sent(&self, n: usize) {
        self.state.lock().unwrap().0.sent(n, Instant::now());
    }

    pub fn acked(&self, total: u64) {
        self.state.lock().unwrap().0.acked(total, Instant::now());
        self.changed.notify_all();
    }

    /// The link went: every wait ends.
    pub fn close(&self) {
        self.state.lock().unwrap().1 = true;
        self.changed.notify_all();
    }

    pub fn budget(&self) -> usize {
        self.state.lock().unwrap().0.budget()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A link of `rate` bytes a second and `rtt` its own round trip,
    /// simulated: what is sent queues behind what was, and is taken (and
    /// acknowledged, half a round trip later) as the rate lets it go.
    /// Paced pieces only, sent as fast as the budget allows, for `secs`.
    /// The worst queue a key behind them met, and the bytes that went.
    fn run(rate: f64, rtt: Duration, secs: f64) -> (Duration, u64, usize) {
        let t0 = Instant::now();
        let mut p = Pace::default();
        // the link: when it is free next, and the acks on their way
        let mut free = t0;
        let mut acks: std::collections::VecDeque<(Instant, u64)> = Default::default();
        let mut now = t0;
        let mut worst = Duration::ZERO;
        let step = Duration::from_micros(200);
        let end = t0 + Duration::from_secs_f64(secs);
        while now < end {
            while acks.front().is_some_and(|(t, _)| *t <= now) {
                let (_, total) = acks.pop_front().unwrap();
                p.acked(total, now);
            }
            loop {
                let n = p.piece(usize::MAX);
                if !p.may_send(n) {
                    break;
                }
                p.sent(n, now);
                let start = free.max(now + rtt / 2);
                free = start + Duration::from_secs_f64(n as f64 / rate);
                acks.push_back((free + rtt / 2, p.sent));
            }
            // a key sent now waits for what is queued ahead of it
            let queued = free.saturating_duration_since(now + rtt / 2);
            if now > t0 + Duration::from_secs(2) {
                worst = worst.max(queued);
            }
            now += step;
        }
        (worst, p.acked, p.budget())
    }

    #[test]
    fn a_key_never_waits_long_behind_a_file() {
        // a slow link far off, a fast one near, and one between
        for (rate, rtt) in [(250e3, 120), (5e6, 30), (50e6, 2)] {
            let (worst, went, budget) = run(rate, Duration::from_millis(rtt), 8.);
            // at most the queue aimed for, one piece going out, and a
            // little over while the budget settles
            let target = Duration::from_millis(15).max(Duration::from_millis(rtt) / 5);
            let piece = Duration::from_secs_f64((budget / 4).clamp(LEAST_PIECE, MOST_PIECE) as f64 / rate);
            let bound = target + piece + Duration::from_millis(25);
            assert!(worst < bound, "{rate} B/s, {rtt} ms: a key waited {worst:?}, over {bound:?} (budget {budget})");
            // and the link is kept busy: most of what it can carry goes
            let could = rate * 8.;
            assert!(went as f64 > 0.8 * could, "{rate} B/s, {rtt} ms: only {went} of {could} (budget {budget})");
        }
    }

    #[test]
    fn one_piece_always_goes_and_the_budget_stays_in_bounds() {
        let mut p = Pace::default();
        let now = Instant::now();
        assert!(p.may_send(MOST_PIECE), "nothing on the way: a piece goes whatever the budget");
        p.sent(MOST_PIECE, now);
        assert!(!p.may_send(LEAST_PIECE), "over the budget: none more");
        // acknowledged after a long queue: shrunk, never under its floor
        for i in 1..50u64 {
            p.sent(LEAST_PIECE, now);
            p.acked(p.sent - LEAST_PIECE as u64, now + Duration::from_millis(5 + 40 * i));
        }
        assert!(p.budget() >= 2 * LEAST_PIECE && p.budget() <= MOST);
        // an old acknowledgment changes nothing
        let b = p.budget();
        p.acked(1, now + Duration::from_secs(5));
        assert_eq!(p.budget(), b);
    }

    #[test]
    fn a_closed_pacer_ends_a_wait() {
        let pacer = std::sync::Arc::new(Pacer::default());
        // fill the budget, then wait for room from another thread
        let mut filled = 0;
        while filled < pacer.budget() {
            let n = pacer.room(MOST_PIECE, &|| false).unwrap();
            pacer.sent(n);
            filled += n;
        }
        let p2 = pacer.clone();
        let waiter = std::thread::spawn(move || p2.room(MOST_PIECE, &|| false));
        std::thread::sleep(Duration::from_millis(50));
        pacer.close();
        assert_eq!(waiter.join().unwrap(), None);
    }
}
