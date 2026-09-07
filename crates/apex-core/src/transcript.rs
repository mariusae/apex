//! Reading a shell transcript (a terminal's text, a win window) for what
//! the last command printed: `Snarfout`. Prompts are found by a
//! heuristic, since nothing marks them: the last line of the transcript
//! is the prompt waiting for input, and the lines that begin like it are
//! the earlier prompts, each followed by the command typed there.

/// The last command and its output, the prompt normalised to `$ `: the
/// command line as `$ cmd`, then the lines between it and the prompt
/// that followed. `None` when no earlier prompt can be told apart.
pub fn last_output(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    // the prompt: the last line with anything on it
    let last = lines.iter().rposition(|l| !l.trim().is_empty())?;
    let prompt = lines[last];
    let marker = prompt.trim_end().chars().last()?;
    let first = prompt.split_whitespace().next()?;
    let needle = format!("{marker} ");
    // where the command starts on a prompt line, if it is one
    let command_at = |l: &str| -> Option<usize> {
        if l == prompt {
            return Some(l.len());
        }
        let head = l.trim_end();
        if !head.starts_with(first) {
            return None;
        }
        match head.find(&needle) {
            Some(i) => Some(i + needle.len()),
            None if head.ends_with(marker) => Some(head.len()),
            None => None,
        }
    };
    let (prev, at) = (0..last).rev().find_map(|i| command_at(lines[i]).map(|at| (i, at)))?;
    let command = lines[prev][at.min(lines[prev].len())..].trim();
    let mut s = format!("$ {command}\n");
    for l in &lines[prev + 1..last] {
        s.push_str(l);
        s.push('\n');
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_command_and_its_output_with_the_prompt_normalised() {
        let t = "% ls\nfoo\nbar\n% echo hi\nhi\n% ";
        assert_eq!(last_output(t).as_deref(), Some("$ echo hi\nhi\n"));
        let t = "marius@airy apex % ls\nfoo\nbar\nmarius@airy src % cat x\none\ntwo\nmarius@airy src % ";
        assert_eq!(last_output(t).as_deref(), Some("$ cat x\none\ntwo\n"));
        // nothing printed
        assert_eq!(last_output("$ true\n$ ").as_deref(), Some("$ true\n"));
        // the prompt line may be followed by blank rows (a terminal's grid)
        assert_eq!(last_output("$ echo a\na\n$ \n\n\n").as_deref(), Some("$ echo a\na\n"));
        // tabs in the output stay
        assert_eq!(last_output("% ls\nA\tB\nC\tD\n% ").as_deref(), Some("$ ls\nA\tB\nC\tD\n"));
        // one prompt only: nothing to tell
        assert_eq!(last_output("$ "), None);
        assert_eq!(last_output(""), None);
    }
}
