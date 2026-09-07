//! LSP positions (line, UTF-16 unit) ⇄ apex's char offsets.

use serde_json::{json, Value};

use apex_core::text::Text;

/// The LSP position of char offset `q`.
pub fn position(t: &Text, q: usize) -> Value {
    let q = q.min(t.len());
    let line = t.line_of(q);
    let start = t.line_start(line);
    let col: usize = t.slice(start, q).encode_utf16().count();
    json!({ "line": line, "character": col })
}

/// The char offset of an LSP position, clamped into the text.
pub fn offset(t: &Text, p: &Value) -> usize {
    let line = p["line"].as_u64().unwrap_or(0) as usize;
    let ch = p["character"].as_u64().unwrap_or(0) as usize;
    if line >= t.line_count() {
        return t.len();
    }
    let Some((s, e)) = t.line_range(line) else { return t.len() };
    let text = t.slice(s, e);
    let mut units = 0;
    let mut chars = 0;
    for c in text.chars() {
        if units >= ch {
            break;
        }
        units += c.len_utf16();
        chars += 1;
    }
    s + chars
}

/// The text after LSP `TextEdit`s, applied from the end so earlier
/// offsets hold.
pub fn apply_edits(t: &Text, edits: &[Value]) -> String {
    let mut spans: Vec<(usize, usize, String)> = edits.iter().map(|e| (offset(t, &e["range"]["start"]), offset(t, &e["range"]["end"]), e["newText"].as_str().unwrap_or("").to_string())).collect();
    spans.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    let mut out = t.clone();
    for (q0, q1, text) in spans {
        out.replace(q0, q1.saturating_sub(q0), &text);
    }
    out.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_round_trip_with_utf16() {
        let t = Text::new("ab\nc😀d\nefg");
        // 😀 is one char, two UTF-16 units
        assert_eq!(position(&t, 0), json!({"line": 0, "character": 0}));
        assert_eq!(position(&t, 3), json!({"line": 1, "character": 0}));
        assert_eq!(position(&t, 5), json!({"line": 1, "character": 3}));
        assert_eq!(offset(&t, &json!({"line": 1, "character": 3})), 5);
        assert_eq!(offset(&t, &json!({"line": 2, "character": 1})), 8);
        assert_eq!(offset(&t, &json!({"line": 9, "character": 0})), t.len());
    }

    #[test]
    fn edits_apply_from_the_end() {
        let t = Text::new("one two three\n");
        let edits = vec![
            json!({"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 3}}, "newText": "1"}),
            json!({"range": {"start": {"line": 0, "character": 8}, "end": {"line": 0, "character": 13}}, "newText": "3"}),
        ];
        assert_eq!(apply_edits(&t, &edits), "1 two 3\n");
    }
}
