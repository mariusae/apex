# apex-edit

The sam/acme `Edit` command language and plan 9 regular expressions, as a
pure Rust library. A line-by-line port of plan9port's acme implementation:

| plan9port source (`src/cmd/acme`) | here |
|---|---|
| `regx.c` | `src/regx.rs` — leftmost-longest regexps, forward and backward machines, wrap-around |
| `edit.c` | `src/parse.rs` — addresses, commands, text/regexp arguments, `{ }` |
| `ecmd.c` | `src/exec.rs` — command semantics and address evaluation |
| `elog.c` | `src/elog.rs` — the change log: changes are collected against the original text and applied at the end, merged exactly as acme merges them |

Everything is in runes (`char`); byte offsets never appear.

```rust
use apex_edit::{Edit, apply};

let mut text: Vec<char> = "hello world\n".chars().collect();
let out = Edit::new().run(&text, (0, 0), None, ",x/o/ c/0/")?;
apply(&mut text, &out.changes);           // "hell0 w0rld\n"
assert_eq!(out.dot, (4, 8));              // acme's rule: the merged change is selected
```

`run` returns the changes in application order, the new dot, what `p` and
`=` printed (as a list, `output_string()` joins it the way acme's `+Errors`
shows it), acme's warnings, and *intents* for commands that have effects
(`u w r e f b B D < | >`), which the caller carries out.

## Tests

- `tests/golden.rs`: feature-by-feature acceptance tests with acme's error
  messages.
- `tests/sam_diff.rs`: differential tests against plan9port's `sam -d`, the
  reference implementation of the language: a corpus of texts × programs,
  then generated programs and regular expressions over random texts.
  Requires sam (`$SAM`, `$PLAN9/bin/sam`, or `~/.local/plan9/bin/sam`); it
  builds on macOS with `./INSTALL -b` and nothing but clang.
- `cargo bench`: `x/re/ c/../`, `s///g`, line loops, apply, regexp search
  and line seeking over a million-rune text.

## Where acme and sam differ

Both are plan 9 programs but their Edit implementations diverged. This crate
follows **acme**. The harness knows these divergences:

- **Dot after a change.** acme adjusts the selection through its change log
  (`textinsert`/`textdelete` rules, merged changes select the merged span;
  `m` and `t` don't move dot). sam sets dot per command. Dot is compared
  with sam only for programs that don't modify the text.
- **Changes out of sequence.** acme's checks are weaker than sam's: it
  warns (or, for some overlaps, says nothing) and applies the log anyway;
  sam refuses. We reproduce acme and report the warning when it gives one.
- **`$` at the end of a range.** sam's machines look at the character just
  past the end of the range being searched (so `$` matches when a newline
  follows it, and sam's backward machine matches `$` at the very end of the
  text); acme's `regx.c` feeds a NUL beyond the range and never matches `$`
  there. Command regexps in the generated tests avoid `$` for that reason;
  whole-text searches, where the engines agree, cover it.
- **Moving a range into itself.** acme's `m` errors ("move overlaps
  itself") unless source and destination are identical; sam's only rejects
  a destination that ends inside the source, so `,m$` on a short text is a
  no-op there and an error here.
- **The bare newline command** prints dot in `sam -d`; acme selects it.

## Departures from acme

- The fixed tables of the C code (`NLIST` threads, `NPROG` instructions,
  `NSTACK` nesting) are dynamic; acme fails with "regexp list overflow" or
  "expression too long" past them.
- `X` and `Y` (loops over files) are not supported: apex has no file menu.
- File addresses (`"regexp"`) and the mark (`'`) are errors, as `'` is in acme.
