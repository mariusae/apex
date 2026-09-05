//! Performance of the core: apply throughput, view adjustment with many
//! windows on one buffer, hashing, snapshots, and replay.

use apex_core::*;
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn session() -> (Log, Node, ColumnId) {
    let mut log = Log::new();
    let (a, _) = log.attach(AttachmentKind::Ui, "bench");
    let mut node = Node::new(a);
    node.catch_up(&log).unwrap();
    let col = node.init_session(&mut log).unwrap();
    (log, node, col)
}

fn big_text(lines: usize) -> String {
    (0..lines).map(|i| format!("line {i}: the quick brown fox jumps over the lazy dog\n")).collect()
}

fn bench(c: &mut Criterion) {
    let mut g = c.benchmark_group("core");
    g.sample_size(20);

    g.bench_function("type 10k keystrokes (edit+select entries)", |b| {
        b.iter(|| {
            let (mut log, mut node, col) = session();
            let w = node.new_window(&mut log, col, "x", "").unwrap();
            let v = ViewId::Body(w);
            for i in 0..10_000 {
                node.insert(&mut log, v, if i % 40 == 39 { "\n" } else { "a" }).unwrap();
            }
            black_box(log.len())
        })
    });

    g.bench_function("edit with 100 views on a 1M-rune buffer", |b| {
        let (mut log, mut node, col) = session();
        let w = node.new_window(&mut log, col, "big", &big_text(20_000)).unwrap();
        for _ in 0..99 {
            node.zerox(&mut log, w).unwrap();
        }
        let v = ViewId::Body(w);
        b.iter(|| {
            node.select(&mut log, v, 500_000, 500_000).unwrap();
            node.insert(&mut log, v, "x").unwrap();
            black_box(node.state.buffers.len())
        })
    });

    g.bench_function("hash state with a 1M-rune buffer", |b| {
        let (mut log, mut node, col) = session();
        node.new_window(&mut log, col, "big", &big_text(20_000)).unwrap();
        b.iter(|| black_box(node.state.hash()))
    });

    g.bench_function("snapshot round trip, 1M-rune buffer", |b| {
        let (mut log, mut node, col) = session();
        node.new_window(&mut log, col, "big", &big_text(20_000)).unwrap();
        b.iter(|| {
            let bytes = node.state.to_snapshot();
            let s = State::from_snapshot(&bytes).unwrap();
            black_box(s.buffers.len())
        })
    });

    g.bench_function("replay 20k entries into a follower", |b| {
        let (mut log, mut node, col) = session();
        let w = node.new_window(&mut log, col, "x", "").unwrap();
        let v = ViewId::Body(w);
        for _ in 0..10_000 {
            node.insert(&mut log, v, "a").unwrap();
        }
        b.iter(|| {
            let mut follower = Node::new(AttachmentId(99));
            follower.catch_up(&log).unwrap();
            black_box(follower.state.hash())
        })
    });

    g.bench_function("Edit x/fox/ c/cat/ lowered over 1M runes", |b| {
        b.iter(|| {
            let (mut log, mut node, col) = session();
            let w = node.new_window(&mut log, col, "big", &big_text(20_000)).unwrap();
            node.run_edit(&mut log, w, ",x/fox/ c/cat/").unwrap();
            black_box(log.len())
        })
    });
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
