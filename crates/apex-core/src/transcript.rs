//! Reading a shell transcript (a terminal's text, a win window) for what
//! the last command printed: `Snarfout`. Prompts are found by a
//! heuristic, since nothing marks them: the last line of the transcript
//! is the prompt waiting for input, and the lines that begin like it are
//! the earlier prompts, each followed by the command typed there.

/// The output of the last command: the lines between the last two
/// prompts, the command line itself left out. `None` when no earlier
/// prompt can be told apart.
pub fn last_output(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    // the prompt: the last line with anything on it
    let last = lines.iter().rposition(|l| !l.trim().is_empty())?;
    let prompt = lines[last];
    let marker = prompt.trim_end().chars().last()?;
    let first = prompt.split_whitespace().next()?;
    let is_prompt = |l: &str| {
        if l == prompt {
            return true;
        }
        let head = l.trim_end();
        if !head.starts_with(first) {
            return false;
        }
        // "PROMPT command": the marker, then a space, somewhere after the head
        let needle = format!("{marker} ");
        match head.find(&needle) {
            Some(i) => {
                // the command follows the marker; a bare prompt is one too
                i + needle.len() <= head.len()
            }
            None => head.ends_with(marker),
        }
    };
    let prev = (0..last).rev().find(|&i| is_prompt(lines[i]))?;
    let out: Vec<&str> = lines[prev + 1..last].to_vec();
    let mut s = out.join("\n");
    if !out.is_empty() {
        s.push('\n');
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lines_between_the_last_two_prompts() {
        let t = "% ls\nfoo\nbar\n% echo hi\nhi\n% ";
        assert_eq!(last_output(t).as_deref(), Some("hi\n"));
        let t = "marius@airy apex % ls\nfoo\nbar\nmarius@airy src % cat x\none\ntwo\nmarius@airy src % ";
        assert_eq!(last_output(t).as_deref(), Some("one\ntwo\n"));
        // nothing printed
        assert_eq!(last_output("$ true\n$ ").as_deref(), Some(""));
        // the prompt line may be followed by blank rows (a terminal's grid)
        assert_eq!(last_output("$ echo a\na\n$ \n\n\n").as_deref(), Some("a\n"));
        // one prompt only: nothing to tell
        assert_eq!(last_output("$ "), None);
        assert_eq!(last_output(""), None);
    }
}
