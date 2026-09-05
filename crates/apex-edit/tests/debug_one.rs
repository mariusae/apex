mod common;
#[test]
#[ignore]
fn one() {
    let text = std::env::var("T").unwrap_or_default().replace("\\n", "\n");
    let program = std::env::var("P").unwrap_or_default().replace("\\n", "\n");
    let s = common::sam(&text, &program);
    let o = common::ours(&text, (0, 0), &program);
    eprintln!("sam:  {s:?}\nours: {o:?}");
}
