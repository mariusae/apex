//! The font sets (Settings ▸ Fonts), one choice for what text windows
//! and tags are set in, what mono windows and terminals are, what the
//! sidebar, sheets and menus are, and what pages from buffers
//! (previews, apex diff) are.
//!
//! - Apex (what was Classic, and keeps its word): Lucida Grande and
//!   Menlo, apex's as it was.
//! - System: SF Pro, with its high legibility set and tabular figures,
//!   and SF Mono -- Terminal's own copy, which is the whole family, at
//!   its Medium weight, since the Regular draws thin at 12.
//! - Humanist (what was Lucida): Lucida Grande and Bigelow & Holmes's
//!   Lucida Grande Mono, its W1G cut -- offered only where that is
//!   installed; not to be bundled. Or, switched to (`intel_one`), with
//!   Intel One Mono installed, that for its mono.
//!
//! The bundled faces go to gpui at launch (`install`) and to pages by
//! `@font-face` from `apexfile://localhost/.apex-font/FILE`, which the
//! page's own scheme handler answers from these bytes (`serve`), so a
//! page, whose web view is another process, has them too.

use std::sync::atomic::{AtomicBool, AtomicI8, AtomicU8, Ordering};

use gpui::{px, App, FontWeight, Pixels};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Set {
    /// Apex's own (`classic`, its word as it was).
    Classic,
    System,
    /// Humanist (`lucida`, its word as it was).
    Lucida,
}

pub const ALL: [Set; 3] = [Set::Classic, Set::System, Set::Lucida];

/// The font set until one is chosen: Apex's (its place in `ALL`), and
/// what one chosen before, and gone since, comes back as.
const DEFAULT_SET: u8 = 0;

impl Set {
    pub fn title(self) -> &'static str {
        match self {
            Set::Classic => "Apex",
            Set::System => "System",
            Set::Lucida => "Humanist",
        }
    }
    fn word(self) -> &'static str {
        match self {
            Set::Classic => "classic",
            Set::System => "system",
            Set::Lucida => "lucida",
        }
    }
    /// Whether it can be had here: Humanist only with Lucida Grande
    /// Mono W1G installed (`install` looks).
    pub fn available(self) -> bool {
        match self {
            Set::Lucida => LUCIDA_MONO.get().is_some(),
            _ => true,
        }
    }
}

/// The sets offered here, in their order.
pub fn offered() -> Vec<Set> {
    ALL.into_iter().filter(|s| s.available()).collect()
}

static SET: AtomicU8 = AtomicU8::new(DEFAULT_SET);

pub fn current() -> Set {
    let s = ALL[SET.load(Ordering::Relaxed) as usize % ALL.len()];
    // Humanist chosen, and its mono not installed (since gone, or
    // another Mac): Apex's
    if INSTALLED.load(Ordering::Relaxed) && !s.available() {
        Set::Classic
    } else {
        s
    }
}

/// `install` has looked at what is installed.
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Chosen: kept in the `fonts` state file, beside the theme's.
pub fn set(s: Set) {
    SET.store(ALL.iter().position(|x| *x == s).map_or(DEFAULT_SET, |i| i as u8), Ordering::Relaxed);
    let p = crate::shell::state_file().with_file_name("fonts");
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, format!("{}\n", s.word()));
}

/// The choice of last time, and the size.
pub fn load() {
    let word = std::fs::read_to_string(crate::shell::state_file().with_file_name("fonts")).unwrap_or_default();
    if let Some(i) = ALL.iter().position(|s| s.word() == word.trim()) {
        SET.store(i as u8, Ordering::Relaxed);
    }
    let step = std::fs::read_to_string(crate::shell::state_file().with_file_name("fontsize")).unwrap_or_default();
    STEP.store(step.trim().parse::<i8>().unwrap_or(0).clamp(MIN_STEP, MAX_STEP), Ordering::Relaxed);
    let mono = std::fs::read_to_string(crate::shell::state_file().with_file_name("humanist-mono")).unwrap_or_default();
    INTEL_ONE.store(mono.trim() == "intel", Ordering::Relaxed);
}

/// `--fontsize N`: the step for this launch, clamped like any other;
/// not written back to the state file, so the flag is not sticky.
pub fn set_step(step: i8) {
    STEP.store(step.clamp(MIN_STEP, MAX_STEP), Ordering::Relaxed);
}

/// ⌘+ and ⌘−: the text a pixel bigger or smaller a step, the mono faces
/// in proportion; ⌘0 back to the set's own. Kept in the `fontsize`
/// state file.
static STEP: AtomicI8 = AtomicI8::new(0);
const MIN_STEP: i8 = -5;
const MAX_STEP: i8 = 14;

/// A step bigger (1), smaller (-1), or back to the set's size (0).
/// Whether that changed anything.
pub fn resize(by: i8) -> bool {
    let was = STEP.load(Ordering::Relaxed);
    let now = if by == 0 { 0 } else { (was + by).clamp(MIN_STEP, MAX_STEP) };
    if now == was {
        return false;
    }
    STEP.store(now, Ordering::Relaxed);
    let _ = std::fs::write(crate::shell::state_file().with_file_name("fontsize"), format!("{now}\n"));
    true
}

/// A face's size and line height scaled as the text's is: the text's
/// size up or down by the step, the rest by the same ratio; line
/// heights whole pixels (acme's tiling counts in them), sizes to the
/// half pixel.
fn sized(mut s: Spec, text_size: f32) -> Spec {
    let step = STEP.load(Ordering::Relaxed);
    if step == 0 {
        return s;
    }
    let k = (text_size + step as f32).max(6.) / text_size;
    s.size = px((f32::from(s.size) * k * 2.).round() / 2.);
    s.line_height = px((f32::from(s.line_height) * k).round());
    s
}

/// A face as the text is set in it: the family, its size and line
/// height (whole pixels: acme's tiling counts in them), its weight, and
/// the OpenType features it takes beyond apex's own (no letters joined,
/// a slashed zero).
pub struct Spec {
    pub family: &'static str,
    pub size: Pixels,
    pub line_height: Pixels,
    pub weight: FontWeight,
    pub features: &'static [(&'static str, u32)],
}

/// SF Pro as an editor sets it for code: high legibility (an I with
/// bars, an l with a tail, a 1 with a flag) and tabular figures.
const LEGIBLE: &[(&str, u32)] = &[("ss06", 1), ("tnum", 1)];

/// Terminal's SF Mono was found and loaded (`install`).
static SF_MONO: AtomicBool = AtomicBool::new(false);
/// The system's own copy of it, `.SF NS Mono`, was, where Terminal's is not.
static SF_NS_MONO: AtomicBool = AtomicBool::new(false);
/// Lucida Grande Mono as it is installed, by the family name it gives
/// (`install` looks: its W1G cut, or another): the Lucida set's mono,
/// else Menlo.
static LUCIDA_MONO: std::sync::OnceLock<String> = std::sync::OnceLock::new();
/// Intel One Mono is installed (`install` looks): Humanist's other mono.
static INTEL_ONE_MONO: AtomicBool = AtomicBool::new(false);
/// Humanist with Intel One Mono for its mono: chosen, kept in the
/// `humanist-mono` state file.
static INTEL_ONE: AtomicBool = AtomicBool::new(false);
const INTEL_ONE_FAMILY: &str = "Intel One Mono";

/// Whether Intel One Mono can be had here: installed.
pub fn intel_one_available() -> bool {
    INTEL_ONE_MONO.load(Ordering::Relaxed)
}

/// Whether Humanist's mono is Intel One Mono: chosen (whether or not it
/// is installed here).
pub fn intel_one() -> bool {
    INTEL_ONE.load(Ordering::Relaxed)
}

/// Humanist's mono switched to Intel One Mono, or back.
pub fn set_intel_one(on: bool) {
    INTEL_ONE.store(on, Ordering::Relaxed);
    let _ = std::fs::write(crate::shell::state_file().with_file_name("humanist-mono"), if on { "intel\n" } else { "lucida\n" });
}

/// Humanist's mono as it is here: Intel One Mono, chosen and installed;
/// else Lucida Grande Mono; else Menlo.
fn humanist_mono() -> &'static str {
    if intel_one() && intel_one_available() {
        INTEL_ONE_FAMILY
    } else {
        LUCIDA_MONO.get().map(String::as_str).unwrap_or("Menlo")
    }
}

/// What text windows and tags are set in.
pub fn text() -> Spec {
    let s = text_as_set();
    let size = f32::from(s.size);
    sized(s, size)
}

fn text_as_set() -> Spec {
    match current() {
        Set::System => Spec { family: ".SystemUIFont", size: px(14.), line_height: px(20.), weight: FontWeight::NORMAL, features: LEGIBLE },
        Set::Classic | Set::Lucida => Spec { family: "Lucida Grande", size: px(13.), line_height: px(17.), weight: FontWeight::NORMAL, features: &[] },
    }
}

/// The weight to ask for to get `w`: what it is (a set whose faces said
/// their weights otherwise, H&Co's, is gone).
pub fn weight(w: FontWeight) -> FontWeight {
    w
}

/// What mono windows and terminals are set in.
pub fn mono() -> Spec {
    sized(mono_as_set(), f32::from(text_as_set().size))
}

fn mono_as_set() -> Spec {
    match current() {
        Set::System => {
            let family = if SF_MONO.load(Ordering::Relaxed) {
                "SF Mono"
            } else if SF_NS_MONO.load(Ordering::Relaxed) {
                ".SF NS Mono"
            } else {
                "Menlo"
            };
            Spec { family, size: px(12.), line_height: px(16.), weight: FontWeight::MEDIUM, features: &[] }
        }
        Set::Classic => Spec { family: "Menlo", size: px(12.), line_height: px(16.), weight: FontWeight::NORMAL, features: &[] },
        // Intel One Mono's x-height is 12% under Lucida's and its
        // extenders long: at 13 its x-height is 0.88 of Lucida 13's (at
        // 12, 0.81) and its cells 8px, 11% wider than Menlo's (at 14 they
        // would be 8.6, an 80-column terminal 688px); its extenders want
        // 18px lines. Regular: Medium measures as dark as Lucida but at 13
        // reads as bold.
        Set::Lucida if humanist_mono() == INTEL_ONE_FAMILY => Spec { family: INTEL_ONE_FAMILY, size: px(13.), line_height: px(18.), weight: FontWeight::NORMAL, features: &[] },
        Set::Lucida => Spec { family: humanist_mono(), size: px(12.), line_height: px(16.), weight: FontWeight::NORMAL, features: &[] },
    }
}

/// Set `s`'s text and mono families, whichever is chosen: what Settings
/// names each set in.
pub fn families(s: Set) -> (String, String) {
    match s {
        Set::Classic => ("Lucida Grande".into(), "Menlo".into()),
        Set::System => {
            let mono = if SF_MONO.load(Ordering::Relaxed) {
                "SF Mono"
            } else if SF_NS_MONO.load(Ordering::Relaxed) {
                ".SF NS Mono"
            } else {
                "Menlo"
            };
            (".SystemUIFont".into(), mono.into())
        }
        Set::Lucida => ("Lucida Grande".into(), humanist_mono().into()),
    }
}

/// What the sidebar, sheets and menus are set in.
pub fn ui() -> &'static str {
    match current() {
        Set::System => ".AppleSystemUIFont",
        Set::Classic | Set::Lucida => "Lucida Grande",
    }
}

/// A bundled face: the file a page asks for, its bytes, and how a page
/// names it (`@font-face`).
pub struct Face {
    pub file: &'static str,
    pub bytes: &'static [u8],
    pub family: &'static str,
    pub weight: u16,
    pub italic: bool,
}


/// None bundled now (the sets that had them, Go's and Geist's, are
/// gone); the way a page has them stays, for a set that brings some.
pub const FACES: &[Face] = &[];

/// The bundled faces to gpui, and SF Mono: Terminal's copy, the whole
/// family (not a font the system lets be named, nor one to bundle),
/// else the system's `.SF NS Mono`.
pub fn install(cx: &mut App) {
    let bundled: Vec<_> = FACES.iter().map(|f| std::borrow::Cow::Borrowed(f.bytes)).collect();
    if !bundled.is_empty() {
        if let Err(e) = cx.text_system().add_fonts(bundled) {
            eprintln!("apex-ui: the bundled fonts: {e}");
        }
    }
    let dir = std::path::Path::new("/System/Applications/Utilities/Terminal.app/Contents/Resources/Fonts");
    let terminal: Vec<std::borrow::Cow<'static, [u8]>> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("SF-Mono-"))
        .filter_map(|e| std::fs::read(e.path()).ok())
        .map(std::borrow::Cow::Owned)
        .collect();
    // Lucida Grande Mono W1G: Humanist's mono, and whether Humanist is
    // offered at all
    let names = cx.text_system().all_font_names();
    if let Some(name) = names.iter().find(|n| n.as_str() == "Lucida Grande Mono W1G") {
        let _ = LUCIDA_MONO.set(name.clone());
    }
    if names.iter().any(|n| n.as_str() == INTEL_ONE_FAMILY) {
        INTEL_ONE_MONO.store(true, Ordering::Relaxed);
    }
    INSTALLED.store(true, Ordering::Relaxed);
    if !terminal.is_empty() && cx.text_system().add_fonts(terminal).is_ok() {
        SF_MONO.store(true, Ordering::Relaxed);
    } else if let Ok(bytes) = std::fs::read("/System/Library/Fonts/SFNSMono.ttf") {
        if cx.text_system().add_fonts(vec![std::borrow::Cow::Owned(bytes)]).is_ok() {
            SF_NS_MONO.store(true, Ordering::Relaxed);
        }
    }
}

/// The path a page asks for a bundled face at, under its own scheme.
pub const PAGE_PATH: &str = "/.apex-font/";

/// A bundled face by the file a page asks for.
pub fn serve(path: &str) -> Option<(&'static [u8], &'static str)> {
    let file = path.strip_prefix(PAGE_PATH)?;
    let f = FACES.iter().find(|f| f.file == file)?;
    Some((f.bytes, if file.ends_with(".otf") { "font/otf" } else { "font/ttf" }))
}

/// For pages: the bundled faces as `@font-face`s (a page loads only
/// those it uses), and the set's families and features as variables --
/// `--apex-font`, `--apex-mono`, `--apex-font-features`,
/// `--apex-mono-features` -- which a page's stylesheet sets itself in.
pub fn page_css() -> String {
    let mut css = String::new();
    for f in FACES {
        css.push_str(&format!(
            "@font-face{{font-family:\"{}\";src:url(\"apexfile://localhost{}{}\");font-weight:{};font-style:{}}}",
            f.family,
            PAGE_PATH,
            f.file,
            f.weight,
            if f.italic { "italic" } else { "normal" }
        ));
    }
    let (sans, mono, sans_features, mono_features) = match current() {
        Set::System => ("-apple-system, BlinkMacSystemFont, sans-serif", "ui-monospace, \"SF Mono\", Menlo, monospace", "\"ss06\", \"tnum\"", "\"zero\""),
        Set::Classic => ("\"Lucida Grande\", \"Lucida Sans Unicode\", sans-serif", "Menlo, monospace", "normal", "normal"),
        Set::Lucida if humanist_mono() == INTEL_ONE_FAMILY => ("\"Lucida Grande\", \"Lucida Sans Unicode\", sans-serif", "\"Intel One Mono\", Menlo, monospace", "normal", "normal"),
        Set::Lucida => ("\"Lucida Grande\", \"Lucida Sans Unicode\", sans-serif", "\"Lucida Grande Mono W1G\", Menlo, monospace", "normal", "normal"),
    };
    css.push_str(&format!(":root{{--apex-font:{sans};--apex-mono:{mono};--apex-font-features:{sans_features};--apex-mono-features:{mono_features}}}"));
    css
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_set_is_apex() {
        assert_eq!(ALL[DEFAULT_SET as usize], Set::Classic);
        assert_eq!(Set::Classic.title(), "Apex");
        assert_eq!(current(), Set::Classic, "until one is chosen");
    }

    #[test]
    fn every_face_a_page_asks_for_is_served_and_named() {
        for f in FACES {
            let (bytes, mime) = serve(&format!("{PAGE_PATH}{}", f.file)).expect(f.file);
            assert!(bytes.len() > 10_000, "{}", f.file);
            assert!(mime.starts_with("font/"));
        }
        assert!(serve("/.apex-font/nothing.ttf").is_none());
        assert!(serve("/etc/passwd").is_none(), "only the bundled faces, nothing else by that path");
        let css = page_css();
        assert!(css.contains("--apex-mono:"));
    }
}
