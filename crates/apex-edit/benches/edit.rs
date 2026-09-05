//! Performance of the Edit language over large texts.

use apex_edit::{apply, Edit, Regex};
use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};

fn big_text(lines: usize) -> Vec<char> {
    let mut s = String::new();
    for i in 0..lines {
        s.push_str(&format!("line {i}: the quick brown fox jumps over the lazy dog {}\n", i % 7));
    }
    s.chars().collect()
}

fn bench(c: &mut Criterion) {
    let text = big_text(20_000); // ~1.1M runes
    let mut g = c.benchmark_group("edit");
    g.throughput(Throughput::Elements(text.len() as u64));
    g.sample_size(20);

    g.bench_function("x/fox/ c/cat/ over 1M runes", |b| {
        b.iter(|| {
            let o = Edit::new().run(&text, (0, 0), None, ",x/fox/ c/cat/").unwrap();
            black_box(o.changes.len())
        })
    });
    g.bench_function("s/dog/cat/g over 1M runes", |b| {
        b.iter(|| {
            let o = Edit::new().run(&text, (0, 0), None, ",s/dog/cat/g").unwrap();
            black_box(o.changes.len())
        })
    });
    g.bench_function("x/.*\\n/ g/3$/ d (line loop)", |b| {
        b.iter(|| {
            let o = Edit::new().run(&text, (0, 0), None, ",x/.*\\n/ g/3$/ d").unwrap();
            black_box(o.changes.len())
        })
    });
    g.bench_function("apply 20k changes", |b| {
        let o = Edit::new().run(&text, (0, 0), None, ",x/fox/ c/cat/").unwrap();
        b.iter(|| {
            let mut t = text.clone();
            apply(&mut t, &o.changes);
            black_box(t.len())
        })
    });
    g.bench_function("regexp search: a|ab|abc leftmost-longest", |b| {
        let re = Regex::compile_str("(quick|lazy) (brown|dog)").unwrap();
        b.iter(|| {
            let mut p = 0;
            let mut n = 0;
            while let Some(s) = re.execute(&text, p, text.len()) {
                n += 1;
                p = (s[0].q1 as usize).max(p + 1);
                if p >= text.len() {
                    break;
                }
            }
            black_box(n)
        })
    });
    g.bench_function("address 10000 (line seek)", |b| {
        b.iter(|| {
            let o = Edit::new().run(&text, (0, 0), None, "10000=#").unwrap();
            black_box(o.output_string().len())
        })
    });
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
