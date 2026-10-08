//! `apex md`: Markdown as the page Preview shows (WEB.md §3), the
//! default converter for `.md` files: GitHub's flavour (tables,
//! footnotes, task lists), a marker before every block with the source
//! line it starts on (`data-line`, which the preview follows the caret
//! by), front matter left out, mermaid fences drawn, headings with
//! GitHub's ids, and the editor's own stylesheet.

/// The length of a front matter block at the top of `text`: a line of
/// `---` (YAML) or `+++` (TOML), the block, and the same line closing
/// it, newline included; 0 when there is none (an unclosed one is text).
pub fn front_matter_len(text: &str) -> usize {
    let fence = if text.starts_with("---") { "---" } else if text.starts_with("+++") { "+++" } else { return 0 };
    let Some(first_nl) = text.find('\n') else { return 0 };
    if text[fence.len()..first_nl].trim().is_empty() == false {
        return 0;
    }
    let mut at = first_nl + 1;
    while at <= text.len() {
        let end = text[at..].find('\n').map(|i| at + i).unwrap_or(text.len());
        let line = text[at..end].trim_end_matches('\r');
        if line.trim_end() == fence {
            return (end + 1).min(text.len());
        }
        if end >= text.len() {
            break;
        }
        at = end + 1;
    }
    0
}

/// Markdown as a whole page, with the stylesheet Preview pages get.
pub fn markdown_page(text: &str) -> String {
    use pulldown_cmark::{html, Options, Parser};
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_HEADING_ATTRIBUTES);
    // a marker before every block with the source line it starts on, so
    // a preview can follow dot (WEB.md §3.3)
    let line_starts: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let line_at = |offset: usize| line_starts.partition_point(|&s| s <= offset);
    // front matter (a --- or +++ block at the top) is for the tools that
    // read it, not the reader; the line numbers still count it
    let skip = front_matter_len(text);
    let mut events: Vec<pulldown_cmark::Event> = Vec::new();
    // inside a ```mermaid block: its text is a diagram's source, which the
    // client draws (WEB.md §3), not code to show
    let mut mermaid = false;
    for (ev, range) in Parser::new_ext(&text[skip..], opts).into_offset_iter() {
        use pulldown_cmark::{CodeBlockKind, CowStr, Event, Tag, TagEnd};
        if let Event::Start(Tag::Paragraph | Tag::Heading { .. } | Tag::BlockQuote(_) | Tag::CodeBlock(_) | Tag::Item | Tag::Table(_) | Tag::HtmlBlock) = &ev {
            events.push(Event::Html(CowStr::from(format!("<span class=\"apex-line\" data-line=\"{}\"></span>", line_at(range.start + skip)))));
        }
        match &ev {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) if info.split_whitespace().next() == Some("mermaid") => {
                mermaid = true;
                events.push(Event::Html(CowStr::from("<pre class=\"mermaid\">")));
                continue;
            }
            Event::End(TagEnd::CodeBlock) if mermaid => {
                mermaid = false;
                events.push(Event::Html(CowStr::from("</pre>\n")));
                continue;
            }
            _ => {}
        }
        events.push(ev);
    }
    heading_ids(&mut events);
    let mut body = String::new();
    html::push_html(&mut body, events.into_iter());
    format!("<!doctype html>\n<html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><style>{MD_STYLE}\n{MD_PAGE}</style></head><body><article class=\"markdown-body\">\n{body}</article></body></html>\n")
}

/// Every heading an id as GitHub gives it, for a link to `#its-id` to go
/// to: its text in lower case, letters, digits, `-` and `_` kept, spaces
/// made `-`, the rest dropped; a second of the same name `-1`, a third
/// `-2`. One the Markdown names itself (`{#id}`) is kept.
fn heading_ids(events: &mut [pulldown_cmark::Event]) {
    use pulldown_cmark::{CowStr, Event, Tag, TagEnd};
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut i = 0;
    while i < events.len() {
        if let Event::Start(Tag::Heading { id: None, .. }) = &events[i] {
            let mut text = String::new();
            let mut j = i + 1;
            while j < events.len() && !matches!(events[j], Event::End(TagEnd::Heading(_))) {
                if let Event::Text(t) | Event::Code(t) = &events[j] {
                    text.push_str(t);
                }
                j += 1;
            }
            let base = slug(&text);
            let n = seen.entry(base.clone()).or_insert(0);
            let id = if *n == 0 { base.clone() } else { format!("{base}-{n}") };
            *n += 1;
            if let Event::Start(Tag::Heading { id: slot, .. }) = &mut events[i] {
                *slot = Some(CowStr::from(id));
            }
        }
        i += 1;
    }
}

/// A heading's text as GitHub makes it an anchor.
fn slug(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}

/// The editor's own look for Markdown: acme's papers and inks as
/// `--apex-*` variables the client sets to its theme's (light acme's
/// when nothing does), Lucida Grande for text and Menlo for code.
const MD_STYLE: &str = include_str!("apex-markdown.css");
const MD_PAGE: &str = "";

#[cfg(test)]
mod front_matter_tests {
    use super::{front_matter_len, markdown_page};

    #[test]
    fn a_mermaid_fence_is_a_diagram_to_draw_and_other_fences_stay_code() {
        let page = markdown_page("```mermaid\ngraph TD\n  A-->B & C<D\n```\n\n```mermaid title=x\nsequenceDiagram\n```\n\n```rust\nfn main() {}\n```\n");
        // the source, escaped, in a block the client draws
        assert!(page.contains("<pre class=\"mermaid\">graph TD\n  A--&gt;B &amp; C&lt;D\n</pre>"), "{page}");
        assert!(page.contains("<pre class=\"mermaid\">sequenceDiagram\n</pre>"), "{page}");
        assert_eq!(page.matches("<pre class=\"mermaid\">").count(), 2);
        // code stays code, and a mermaid block is no code block
        assert!(page.contains("<pre><code class=\"language-rust\">fn main() {}"), "{page}");
        assert!(!page.contains("language-mermaid"), "{page}");
        // the line marker still comes before it, for following dot
        assert!(page.contains("data-line=\"1\"></span><pre class=\"mermaid\">"), "{page}");
    }

    #[test]
    fn front_matter_is_left_out_and_lines_still_count() {
        let text = "---\ntitle: x\ntags: [a]\n---\n# Head\n\nbody\n";
        assert_eq!(front_matter_len(text), "---\ntitle: x\ntags: [a]\n---\n".len());
        let page = markdown_page(text);
        assert!(!page.contains("title: x"), "{page}");
        assert!(page.contains("<h1 id=\"head\">"), "{page}");
        // the heading is on line 5 of the file: the markers count from 1
        assert!(page.contains("data-line=\"5\""), "{page}");
        assert_eq!(front_matter_len("+++\na = 1\n+++\nrest"), 14);
        assert_eq!(front_matter_len("--- not front matter\n---\n"), 0);
        assert_eq!(front_matter_len("---\nunclosed\n"), 0);
        assert_eq!(front_matter_len("# plain\n"), 0);
    }
}
