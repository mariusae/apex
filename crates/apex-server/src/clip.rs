//! The clipboard for programs in a session's terminals, on any host:
//! the app's -- the Mac's -- read and written over the I/O plane, so a
//! program on another host that pastes an image (Claude Code's ^V, which
//! runs `xclip -selection clipboard -t image/png -o`) has the one on the
//! Mac. apex stands in for xclip, xsel, wl-paste and wl-copy on such a
//! host (`apex-cli`'s `clip`, links on the session's PATH), each asking
//! the daemon in the words below; the daemon asks the app (`ClipboardAsk`,
//! answered on a paced stream of its own, so an image keeps no key
//! waiting) and passes on what it says.
//!
//! The clipboard is read only just after a key in one of the session's
//! terminals (`FRESH`): a program can read it when the user pastes into
//! it, not behind their back while nobody is there.
//!
//! What is asked for (`want`): `TARGETS` (what there is, a line each, of
//! `image/png`, `image/jpeg` and `text`), `text`, `image/png` (any image
//! the Mac has but a JPEG made a PNG), `image/jpeg`.

use std::time::Duration;

/// How long after a key in a terminal the clipboard may be read.
pub const FRESH: Duration = Duration::from_secs(5);
/// How long the app has to answer.
pub const ANSWER_WAIT: Duration = Duration::from_secs(20);

/// The kinds there are.
pub const WANTS: [&str; 4] = ["TARGETS", "text", "image/png", "image/jpeg"];

pub fn valid_want(want: &str) -> bool {
    WANTS.contains(&want)
}

/// The tools apex stands in for, by the names programs run them by.
pub const TOOLS: [&str; 4] = ["xclip", "xsel", "wl-paste", "wl-copy"];

#[cfg(test)]
mod tests {
    #[test]
    fn the_links_are_made_and_made_anew_for_a_newer_apex() {
        let home = std::env::temp_dir().join(format!("apex-clip-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let (old, new) = (home.join("old-apex"), home.join("apex"));
        let dir = crate::daemon::clip_links(&home, &old).unwrap();
        assert_eq!(dir, home.join(".apex/clip"));
        for t in super::TOOLS {
            assert_eq!(std::fs::read_link(dir.join(t)).unwrap(), old);
        }
        crate::daemon::clip_links(&home, &new).unwrap();
        for t in super::TOOLS {
            assert_eq!(std::fs::read_link(dir.join(t)).unwrap(), new, "{t}");
        }
        let _ = std::fs::remove_dir_all(&home);
    }
}
