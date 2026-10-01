//! ⌘O's listing, on the host: every file and directory under a root,
//! found by a walk on a thread of its own and matched against what the
//! user types on another, so that neither the walk nor the matching ever
//! holds up the daemon, and only what is shown crosses the wire.
//!
//! - The walk is breadth first (what is near the root comes first), does
//!   not go into directories whose names begin with a dot (`.git`, `.hg`,
//!   `.sl`), `node_modules`, `__pycache__` or `buck-out`, nor follow a
//!   link to a directory; it stops at `CAP` entries, and says so. What it
//!   finds goes into the index in chunks, which a reader takes a snapshot
//!   of without holding up the walk.
//! - A query (a generation, the text, how many to send) is matched over
//!   the snapshot by `apex_core::fuzzy`, across the cores, best first; a
//!   newer query or the
//!   job's end abandons it part way; a query that only adds to the one
//!   before is matched over that one's matches and what was found since.
//!   While the walk goes on, the query is matched again as entries come,
//!   at most every `TICK`, so the list fills in as the walk goes.
//! - What is sent is the best `limit` (a page; more as the user scrolls),
//!   with how many match, how many are indexed, and whether the walk is
//!   done -- as `ServerMsg::Found`, straight to the connection that asked.
//! - Dropping the job (the picker closed, the connection gone) ends both
//!   threads at once.

use std::collections::{BinaryHeap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use apex_core::fuzzy::{Query, Scorer};
use rayon::prelude::*;

use crate::proto::ServerMsg;

/// The most entries a walk indexes.
pub const CAP: usize = 1_000_000;
/// How often matches are sent again while the walk goes on.
const TICK: Duration = Duration::from_millis(120);
/// Candidates a core matches at a time, between looks for a newer query.
const SLICE: usize = 16384;
/// Entries a chunk of the index holds before it is published.
const CHUNK: usize = 4096;
/// How often a chunk is published anyway, however few it holds.
const FLUSH: Duration = Duration::from_millis(50);

/// Directories the walk does not go into.
fn skipped(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "node_modules" | "__pycache__" | "buck-out")
}

/// A file or directory found: its path relative to the root (a
/// directory's without its slash), and whether it is a directory.
pub struct Entry {
    pub path: Box<str>,
    pub dir: bool,
}

/// What the walk has found so far.
#[derive(Default)]
struct Index {
    chunks: Mutex<Vec<Arc<Vec<Entry>>>>,
    count: AtomicUsize,
    done: AtomicBool,
    capped: AtomicBool,
    error: Mutex<Option<String>>,
}

impl Index {
    fn snapshot(&self) -> Vec<Arc<Vec<Entry>>> {
        self.chunks.lock().unwrap().clone()
    }
}

/// The query to match, and whether it is new since last sent.
struct Want {
    gen: u64,
    query: String,
    limit: usize,
    fresh: bool,
}

struct Shared {
    index: Index,
    cancel: AtomicBool,
    want: Mutex<Want>,
    wake: Condvar,
}

/// A listing under way. Dropped, it stops.
pub struct Job {
    shared: Arc<Shared>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.shared.cancel.store(true, Ordering::Relaxed);
        self.shared.wake.notify_all();
    }
}

impl Job {
    /// List everything under `root`, sending matches for request `id` to
    /// `sink` as queries come.
    pub fn start(id: u64, root: PathBuf, sink: impl Fn(ServerMsg) + Send + 'static) -> Job {
        let shared = Arc::new(Shared { index: Index::default(), cancel: AtomicBool::new(false), want: Mutex::new(Want { gen: 0, query: String::new(), limit: 0, fresh: false }), wake: Condvar::new() });
        let s = shared.clone();
        std::thread::Builder::new().name("apex-find-walk".into()).spawn(move || walk(&s, root)).ok();
        let s = shared.clone();
        std::thread::Builder::new().name("apex-find-match".into()).spawn(move || serve(&s, id, sink)).ok();
        Job { shared }
    }

    /// Match `query` from now on, sending the best `limit`.
    pub fn query(&self, gen: u64, query: &str, limit: usize) {
        let mut w = self.shared.want.lock().unwrap();
        if gen < w.gen {
            return;
        }
        *w = Want { gen, query: query.to_string(), limit: limit.max(1), fresh: true };
        drop(w);
        self.shared.wake.notify_all();
    }
}

/// Breadth first from `root`, into the index, until done, capped or
/// cancelled.
fn walk(s: &Shared, root: PathBuf) {
    let ix = &s.index;
    let mut queue: VecDeque<String> = VecDeque::from([String::new()]);
    let mut batch: Vec<Entry> = Vec::with_capacity(CHUNK);
    let mut last = Instant::now();
    let publish = |batch: &mut Vec<Entry>| {
        if batch.is_empty() {
            return;
        }
        let n = batch.len();
        ix.chunks.lock().unwrap().push(Arc::new(std::mem::replace(batch, Vec::with_capacity(CHUNK))));
        ix.count.fetch_add(n, Ordering::Relaxed);
        s.wake.notify_all();
    };
    let mut first = true;
    'walk: while let Some(rel) = queue.pop_front() {
        let dir = if rel.is_empty() { root.clone() } else { root.join(&rel) };
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) => {
                if first {
                    *ix.error.lock().unwrap() = Some(format!("{}: {e}", root.display()));
                }
                continue;
            }
        };
        first = false;
        // a directory's entries in a steady order, as ls lists them -- a
        // chunk at a time, so a directory of a great many (on a slow disk,
        // say) shows as it is read rather than once it all has been
        let mut here: Vec<(String, bool)> = Vec::new();
        let mut rd = rd.flatten().peekable();
        while rd.peek().is_some() {
            here.clear();
            for e in rd.by_ref().take(CHUNK) {
                if s.cancel.load(Ordering::Relaxed) {
                    break 'walk;
                }
                let name = e.file_name().to_string_lossy().to_string();
                // a link is listed, not followed
                let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                here.push((name, is_dir));
            }
            here.sort_by(|a, b| a.0.cmp(&b.0));
            for (name, is_dir) in here.drain(..) {
                if is_dir && skipped(&name) {
                    continue;
                }
                let path = if rel.is_empty() { name } else { format!("{rel}/{name}") };
                if is_dir {
                    queue.push_back(path.clone());
                }
                batch.push(Entry { path: path.into_boxed_str(), dir: is_dir });
                if ix.count.load(Ordering::Relaxed) + batch.len() >= CAP {
                    ix.capped.store(true, Ordering::Relaxed);
                    break 'walk;
                }
                if batch.len() >= CHUNK || last.elapsed() >= FLUSH {
                    publish(&mut batch);
                    last = Instant::now();
                }
            }
        }
        if last.elapsed() >= FLUSH {
            publish(&mut batch);
            last = Instant::now();
        }
    }
    publish(&mut batch);
    ix.done.store(true, Ordering::Relaxed);
    s.wake.notify_all();
}

/// A match: its score, its place in the index (earlier wins a tie), and
/// what to send.
struct Hit {
    score: f64,
    at: u32,
}

impl PartialEq for Hit {
    fn eq(&self, o: &Hit) -> bool {
        self.cmp(o) == std::cmp::Ordering::Equal
    }
}
impl Eq for Hit {}
impl PartialOrd for Hit {
    fn partial_cmp(&self, o: &Hit) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Hit {
    /// Better is greater: a higher score, then an earlier place.
    fn cmp(&self, o: &Hit) -> std::cmp::Ordering {
        self.score.partial_cmp(&o.score).unwrap_or(std::cmp::Ordering::Equal).then(o.at.cmp(&self.at))
    }
}

/// The last query's matches, every one, for the next query that only
/// adds to it.
struct Last {
    query: String,
    scanned: usize,
    matches: Vec<u32>,
}

/// Match queries as they come and the index as it grows; send.
fn serve(s: &Shared, id: u64, sink: impl Fn(ServerMsg)) {
    let mut last: Option<Last> = None;
    // what was last sent: the generation, how much was indexed, done,
    // and when -- the index growing is matched again a tick after that
    let mut sent: Option<(u64, usize, bool, Instant)> = None;
    loop {
        // wait for a new query, or for the index to have grown since
        let (gen, query, limit) = {
            let mut w = s.want.lock().unwrap();
            loop {
                if s.cancel.load(Ordering::Relaxed) {
                    return;
                }
                let count = s.index.count.load(Ordering::Relaxed);
                let done = s.index.done.load(Ordering::Relaxed);
                let stale = sent.is_some_and(|(g, c, d, _)| g == w.gen && (c != count || d != done));
                let due = sent.map(|(_, _, _, at)| at + TICK).unwrap_or_else(Instant::now);
                let now = Instant::now();
                if w.gen > 0 && (w.fresh || (stale && (now >= due || done))) {
                    w.fresh = false;
                    break (w.gen, w.query.clone(), w.limit);
                }
                let wait = if stale && due > now { due - now } else { TICK };
                w = s.wake.wait_timeout(w, wait).unwrap().0;
            }
        };
        let done = s.index.done.load(Ordering::Relaxed);
        let chunks = s.index.snapshot();
        // the snapshot as one run, an entry by its place in it
        let flat: Vec<&Entry> = chunks.iter().flat_map(|c| c.iter()).collect();
        let count = flat.len();
        let entry = |at: usize| -> &Entry { flat[at] };
        let newer = || s.cancel.load(Ordering::Relaxed) || s.want.lock().unwrap().gen != gen;
        let q = Query::new(query.trim());
        let (items, matched) = if q.is_empty() {
            // nothing typed: what the walk found, in its order
            let n = count.min(limit);
            ((0..n).map(|i| entry(i)).map(|e| (e.path.to_string(), e.dir)).collect(), count)
        } else {
            // only what could match: the last query's matches and what was
            // found since, when this query adds to it; else everything
            let narrowing = last.as_ref().filter(|l| !l.query.is_empty() && query.trim().starts_with(l.query.as_str()) && l.scanned <= count);
            let candidates: Vec<u32> = match narrowing {
                Some(l) => l.matches.iter().copied().chain(l.scanned as u32..count as u32).collect(),
                None => (0..count as u32).collect(),
            };
            // across the cores, a slice each with a scorer of its own, each
            // slice's best and matches in order; a newer query stops them
            let abandoned = AtomicBool::new(false);
            let parts: Vec<(Vec<u32>, Vec<Hit>)> = candidates
                .par_chunks(SLICE)
                .map_init(Scorer::default, |scorer, part| {
                    if abandoned.load(Ordering::Relaxed) || newer() {
                        abandoned.store(true, Ordering::Relaxed);
                        return (Vec::new(), Vec::new());
                    }
                    let mut heap: BinaryHeap<std::cmp::Reverse<Hit>> = BinaryHeap::with_capacity(limit + 1);
                    let mut all = Vec::new();
                    for &at in part {
                        if let Some(score) = scorer.score(&q, &flat[at as usize].path) {
                            all.push(at);
                            heap.push(std::cmp::Reverse(Hit { score, at }));
                            if heap.len() > limit {
                                heap.pop();
                            }
                        }
                    }
                    (all, heap.into_iter().map(|r| r.0).collect())
                })
                .collect();
            if abandoned.load(Ordering::Relaxed) {
                continue;
            }
            let mut all: Vec<u32> = Vec::new();
            let mut hits: Vec<Hit> = Vec::new();
            for (a, h) in parts {
                all.extend(a);
                hits.extend(h);
            }
            hits.sort_by(|a, b| b.cmp(a));
            hits.truncate(limit);
            let matched = all.len();
            last = Some(Last { query: query.trim().to_string(), scanned: count, matches: all });
            (hits.into_iter().map(|h| entry(h.at as usize)).map(|e| (e.path.to_string(), e.dir)).collect(), matched)
        };
        if newer() {
            continue;
        }
        sink(ServerMsg::Found {
            id,
            gen,
            items,
            matched: matched as u64,
            indexed: count as u64,
            done,
            capped: s.index.capped.load(Ordering::Relaxed),
            error: s.index.error.lock().unwrap().clone(),
        });
        sent = Some((gen, count, done, Instant::now()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn tree() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("apex-find-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        for f in ["src/main.rs", "src/lib.rs", "src/find/walk.rs", "README.md", "node_modules/x/index.js", ".git/HEAD", "docs/guide.md"] {
            let p = d.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        }
        d
    }

    /// The next `Found` for generation `gen` once the walk is done.
    fn settled(rx: &mpsc::Receiver<ServerMsg>, gen: u64) -> (Vec<(String, bool)>, u64, u64) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(ServerMsg::Found { gen: g, items, matched, indexed, done: true, .. }) = rx.recv_timeout(Duration::from_millis(200)) {
                if g == gen {
                    return (items, matched, indexed);
                }
            }
        }
        panic!("no settled answer for generation {gen}");
    }

    #[test]
    fn a_listing_walks_breadth_first_and_leaves_out_what_is_not_wanted() {
        let d = tree();
        let (tx, rx) = mpsc::channel();
        let job = Job::start(7, d.clone(), move |m| {
            let _ = tx.send(m);
        });
        job.query(1, "", 100);
        let (items, matched, indexed) = settled(&rx, 1);
        let paths: Vec<&str> = items.iter().map(|(p, _)| p.as_str()).collect();
        // the root's entries first, in order; nothing under node_modules
        // or .git, though node_modules itself is there to open
        assert_eq!(&paths[..3], &["README.md", "docs", "src"]);
        assert!(!paths.iter().any(|p| p.contains("index.js") || p.contains(".git")), "{paths:?}");
        assert!(paths.contains(&"src/find/walk.rs"));
        assert_eq!((matched, indexed), (paths.len() as u64, paths.len() as u64));
        assert!(items.iter().any(|(p, dir)| p == "src/find" && *dir));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_query_is_matched_best_first_and_a_longer_one_narrows_it() {
        let d = tree();
        let (tx, rx) = mpsc::channel();
        let job = Job::start(7, d.clone(), move |m| {
            let _ = tx.send(m);
        });
        job.query(1, "m", 100);
        let (_, many, _) = settled(&rx, 1);
        job.query(2, "main", 100);
        let (items, matched, _) = settled(&rx, 2);
        assert_eq!(items.first().map(|(p, _)| p.as_str()), Some("src/main.rs"));
        assert!(matched >= 1 && matched < many);
        // a page: no more than asked for, the count all the same
        job.query(3, "s", 1);
        let (items, matched, _) = settled(&rx, 3);
        assert_eq!(items.len(), 1);
        assert!(matched > 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// On a real, large tree (APEX_FIND_ROOT, or ~/src): how soon the
    /// first answer comes, how soon a query is answered while the walk
    /// goes on, and how the walk ends. Run by hand: cargo test -p
    /// apex-server --lib -- --ignored --nocapture find::tests::large
    #[test]
    #[ignore]
    fn large() {
        let root = std::env::var("APEX_FIND_ROOT").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap()).join("src"));
        let (tx, rx) = mpsc::channel();
        let t0 = Instant::now();
        let job = Job::start(1, root, move |m| {
            let _ = tx.send(m);
        });
        job.query(1, "", 200);
        let first = rx.recv_timeout(Duration::from_secs(10)).expect("a first answer");
        eprintln!("first answer after {:?}", t0.elapsed());
        if let ServerMsg::Found { indexed, .. } = first {
            eprintln!("  {indexed} indexed then");
        }
        let t1 = Instant::now();
        job.query(2, "mainrs", 200);
        loop {
            match rx.recv_timeout(Duration::from_secs(60)).expect("answers") {
                ServerMsg::Found { gen: 2, matched, indexed, done, items, .. } => {
                    eprintln!("query answered after {:?}: {matched} of {indexed} indexed, done {done}, best {:?}", t1.elapsed(), items.first());
                    if done {
                        break;
                    }
                }
                _ => {}
            }
        }
        eprintln!("walk done after {:?}", t0.elapsed());
        let t2 = Instant::now();
        job.query(3, "mainrsx", 200);
        loop {
            if let ServerMsg::Found { gen: 3, matched, .. } = rx.recv_timeout(Duration::from_secs(60)).expect("answers") {
                eprintln!("narrowed query answered after {:?}: {matched}", t2.elapsed());
                break;
            }
        }
        let t3 = Instant::now();
        job.query(4, "e", 200);
        loop {
            if let ServerMsg::Found { gen: 4, matched, .. } = rx.recv_timeout(Duration::from_secs(60)).expect("answers") {
                eprintln!("a broad query over everything answered after {:?}: {matched}", t3.elapsed());
                break;
            }
        }
    }

    #[test]
    fn a_job_dropped_stops_and_sends_nothing_more() {
        let d = tree();
        let (tx, rx) = mpsc::channel();
        let job = Job::start(7, d.clone(), move |m| {
            let _ = tx.send(m);
        });
        job.query(1, "", 10);
        let _ = settled(&rx, 1);
        drop(job);
        std::thread::sleep(Duration::from_millis(300));
        while rx.try_recv().is_ok() {}
        std::thread::sleep(Duration::from_millis(300));
        assert!(rx.try_recv().is_err(), "nothing after the end");
        let _ = std::fs::remove_dir_all(&d);
    }
}
