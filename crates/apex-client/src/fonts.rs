//! The font sets (the modern-mac branch, View ▸ Font), to live with a
//! while and choose among: what text windows and tags are set in, what
//! mono windows and terminals are, what the sidebar, sheets and menus
//! are, and what pages from buffers (previews, apex diff) are, all from
//! one choice.
//!
//! - System: SF Pro, with its high legibility set and tabular figures,
//!   and SF Mono -- Terminal's own copy, which is the whole family, at
//!   its Medium weight, since the Regular draws thin at 12.
//! - Classic: Lucida Grande and Menlo, apex's as it was.
//! - Go: Go and Go Mono (bundled).
//! - Nova: as Panic's Nova is set, SF (with its high legibility set and
//!   tabular figures, as System's) for text and the interface, and Menlo
//!   for code.
//! - Mona: Mona Sans and Monaspace Xenon (bundled), Xenon set as
//!   Manifold sets it: texture healing (`calt`) and stylistic sets 2, 3,
//!   7 and 8, and in pages Radon for its italics.
//! - H&Co: Hoefler & Co.'s Ideal Sans (screen smart) and Operator Mono,
//!   not to be bundled: the system's copies, or Operator Mono's files in
//!   the user's iCloud Drive `Fonts` folder where it is not installed
//!   (`install`, which pages are served them from as the bundled ones).
//!
//! The bundled faces go to gpui at launch (`install`) and to pages by
//! `@font-face` from `apexfile://localhost/.apex-font/FILE`, which the
//! page's own scheme handler answers from these bytes (`serve`), so a
//! page, whose web view is another process, has them too.

use std::sync::atomic::{AtomicBool, AtomicI8, AtomicU8, Ordering};

use gpui::{px, App, FontWeight, Pixels};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Set {
    System,
    Classic,
    Go,
    Mona,
    Nova,
    Hco,
}

pub const ALL: [Set; 6] = [Set::System, Set::Classic, Set::Go, Set::Mona, Set::Nova, Set::Hco];

impl Set {
    pub fn title(self) -> &'static str {
        match self {
            Set::System => "System",
            Set::Classic => "Classic",
            Set::Go => "Go",
            Set::Mona => "Mona",
            Set::Nova => "Nova",
            Set::Hco => "H&Co",
        }
    }

    fn word(self) -> &'static str {
        match self {
            Set::System => "system",
            Set::Classic => "classic",
            Set::Go => "go",
            Set::Mona => "mona",
            Set::Nova => "nova",
            Set::Hco => "hco",
        }
    }
}

static SET: AtomicU8 = AtomicU8::new(0);

pub fn current() -> Set {
    ALL[SET.load(Ordering::Relaxed) as usize % ALL.len()]
}

/// Chosen: kept in the `fonts` state file, beside the theme's.
pub fn set(s: Set) {
    SET.store(ALL.iter().position(|x| *x == s).unwrap_or(0) as u8, Ordering::Relaxed);
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

/// Monaspace Xenon as Manifold sets it: texture healing, and stylistic
/// sets 2, 3, 7 and 8. Each is a substitution of one glyph for another
/// in a monospaced font, so every character keeps its cell and a click
/// still lands between any two.
const XENON: &[(&str, u32)] = &[("calt", 1), ("ss02", 1), ("ss03", 1), ("ss07", 1), ("ss08", 1)];
/// SF Pro as an editor sets it for code: high legibility (an I with
/// bars, an l with a tail, a 1 with a flag) and tabular figures.
const LEGIBLE: &[(&str, u32)] = &[("ss06", 1), ("tnum", 1)];

/// Terminal's SF Mono was found and loaded (`install`).
static SF_MONO: AtomicBool = AtomicBool::new(false);
/// The system's own copy of it, `.SF NS Mono`, was, where Terminal's is not.
static SF_NS_MONO: AtomicBool = AtomicBool::new(false);

/// What text windows and tags are set in.
pub fn text() -> Spec {
    let s = text_as_set();
    let size = f32::from(s.size);
    sized(s, size)
}

fn text_as_set() -> Spec {
    match current() {
        Set::System => Spec { family: ".SystemUIFont", size: px(14.), line_height: px(20.), weight: FontWeight::NORMAL, features: LEGIBLE },
        Set::Classic => Spec { family: "Lucida Grande", size: px(13.), line_height: px(17.), weight: FontWeight::NORMAL, features: &[] },
        Set::Go => Spec { family: "Go", size: px(14.), line_height: px(20.), weight: FontWeight::NORMAL, features: &[] },
        Set::Mona => Spec { family: "Mona Sans", size: px(15.), line_height: px(21.), weight: FontWeight::NORMAL, features: &[] },
        Set::Nova => Spec { family: ".SystemUIFont", size: px(14.), line_height: px(20.), weight: FontWeight::NORMAL, features: LEGIBLE },
        Set::Hco => Spec { family: IDEAL, size: px(14.), line_height: px(20.), weight: FontWeight::NORMAL, features: &[] },
    }
}

/// Hoefler & Co.'s families, as their screen-smart cuts name themselves.
const IDEAL: &str = "Ideal Sans SSm";
const OPERATOR: &str = "Operator Mono SSm";

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
        Set::Go => Spec { family: "Go Mono", size: px(12.), line_height: px(16.), weight: FontWeight::NORMAL, features: &[] },
        Set::Mona => Spec { family: "Monaspace Xenon", size: px(12.), line_height: px(16.), weight: FontWeight::NORMAL, features: XENON },
        Set::Nova => Spec { family: "Menlo", size: px(12.), line_height: px(16.), weight: FontWeight::NORMAL, features: &[] },
        Set::Hco => Spec { family: OPERATOR, size: px(12.), line_height: px(16.), weight: FontWeight::NORMAL, features: &[] },
    }
}

/// What the sidebar, sheets and menus are set in.
pub fn ui() -> &'static str {
    match current() {
        Set::System => ".AppleSystemUIFont",
        Set::Classic => "Lucida Grande",
        Set::Go => "Go",
        Set::Mona => "Mona Sans",
        Set::Nova => ".AppleSystemUIFont",
        Set::Hco => IDEAL,
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

macro_rules! face {
    ($dir:literal, $file:literal, $family:literal, $weight:literal, $italic:literal) => {
        Face { file: $file, bytes: include_bytes!(concat!("../assets/fonts/", $dir, "/", $file)), family: $family, weight: $weight, italic: $italic }
    };
}

pub const FACES: &[Face] = &[
    face!("go", "Go-Regular.ttf", "Go", 400, false),
    face!("go", "Go-Bold.ttf", "Go", 700, false),
    face!("go", "Go-Italic.ttf", "Go", 400, true),
    face!("go", "Go-Bold-Italic.ttf", "Go", 700, true),
    face!("go", "Go-Mono.ttf", "Go Mono", 400, false),
    face!("go", "Go-Mono-Bold.ttf", "Go Mono", 700, false),
    face!("go", "Go-Mono-Italic.ttf", "Go Mono", 400, true),
    face!("go", "Go-Mono-Bold-Italic.ttf", "Go Mono", 700, true),
    face!("mona", "MonaSans-Regular.otf", "Mona Sans", 400, false),
    face!("mona", "MonaSans-Medium.otf", "Mona Sans", 500, false),
    face!("mona", "MonaSans-Bold.otf", "Mona Sans", 700, false),
    face!("mona", "MonaSans-Italic.otf", "Mona Sans", 400, true),
    face!("mona", "MonaSans-BoldItalic.otf", "Mona Sans", 700, true),
    face!("monaspace", "MonaspaceXenon-Regular.otf", "Monaspace Xenon", 400, false),
    face!("monaspace", "MonaspaceXenon-Bold.otf", "Monaspace Xenon", 700, false),
    // Xenon's italics are Radon's, the handwritten one, as Manifold's are
    face!("monaspace", "MonaspaceRadon-Italic.otf", "Monaspace Xenon", 400, true),
    face!("monaspace", "MonaspaceRadon-BoldItalic.otf", "Monaspace Xenon", 700, true),
];

/// Faces found on this machine rather than bundled (H&Co's where they
/// are not installed), loaded at launch: served to pages as the bundled
/// ones are.
static FOUND: std::sync::OnceLock<Vec<Face>> = std::sync::OnceLock::new();

/// Every face a page may be served: the bundled ones, and those found.
fn faces() -> impl Iterator<Item = &'static Face> {
    FACES.iter().chain(FOUND.get().into_iter().flatten())
}

/// H&Co's families not installed: their files from the user's iCloud
/// Drive `Fonts` folder (the family's folder, or loose there), by the
/// weight and slant each file's name says.
fn hco_faces(installed: &[String]) -> Vec<Face> {
    let Some(home) = std::env::var_os("HOME") else { return Vec::new() };
    let fonts = std::path::Path::new(&home).join("Library/Mobile Documents/com~apple~CloudDocs/Fonts");
    let mut out = Vec::new();
    for (family, prefix, dirs) in [(OPERATOR, "OperatorMonoSSm-", ["HCo_OperatorMonoSSm/OpenType", ""]), (IDEAL, "IdealSansSSm-", ["HCo_IdealSansSSm_Pro/OpenType", "HCo_IdealSansSSm_Basic/OpenType"])] {
        if installed.iter().any(|n| n == family) {
            continue;
        }
        let mut seen = std::collections::HashSet::new();
        for dir in dirs {
            let Ok(rd) = std::fs::read_dir(fonts.join(dir)) else { continue };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                let Some(style) = name.strip_prefix(prefix).and_then(|r| r.strip_suffix(".otf")) else { continue };
                if !seen.insert(name.clone()) {
                    continue;
                }
                let Ok(bytes) = std::fs::read(e.path()) else { continue };
                let italic = style.contains("Italic");
                let weight = match style.trim_end_matches("Italic").trim_end_matches("-Pro") {
                    s if s.starts_with("XLight") => 200,
                    s if s.starts_with("Light") => 300,
                    s if s.starts_with("Medium") => 500,
                    s if s.starts_with("Semibold") => 600,
                    s if s.starts_with("Bold") => 700,
                    _ => 400,
                };
                out.push(Face { file: Box::leak(name.into_boxed_str()), bytes: Box::leak(bytes.into_boxed_slice()), family, weight, italic });
            }
        }
    }
    out
}

/// The bundled faces to gpui, and SF Mono: Terminal's copy, the whole
/// family (not a font the system lets be named, nor one to bundle),
/// else the system's `.SF NS Mono`; and H&Co's where they are not
/// installed.
pub fn install(cx: &mut App) {
    let installed = cx.text_system().all_font_names();
    let found = hco_faces(&installed);
    if !found.is_empty() {
        let bytes = found.iter().map(|f| std::borrow::Cow::Borrowed(f.bytes)).collect();
        if let Err(e) = cx.text_system().add_fonts(bytes) {
            eprintln!("apex-ui: H&Co's fonts: {e}");
        }
    }
    let _ = FOUND.set(found);
    let bundled = FACES.iter().filter(|f| !f.italic || !f.file.starts_with("MonaspaceRadon")).map(|f| std::borrow::Cow::Borrowed(f.bytes)).collect();
    if let Err(e) = cx.text_system().add_fonts(bundled) {
        eprintln!("apex-ui: the bundled fonts: {e}");
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
    let f = faces().find(|f| f.file == file)?;
    Some((f.bytes, if file.ends_with(".otf") { "font/otf" } else { "font/ttf" }))
}

/// For pages: the bundled faces as `@font-face`s (a page loads only
/// those it uses), and the set's families and features as variables --
/// `--apex-font`, `--apex-mono`, `--apex-font-features`,
/// `--apex-mono-features` -- which a page's stylesheet sets itself in.
pub fn page_css() -> String {
    let mut css = String::new();
    for f in faces() {
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
        Set::Go => ("\"Go\", sans-serif", "\"Go Mono\", monospace", "normal", "normal"),
        Set::Nova => ("-apple-system, BlinkMacSystemFont, sans-serif", "Menlo, monospace", "\"ss06\", \"tnum\"", "normal"),
        Set::Mona => ("\"Mona Sans\", sans-serif", "\"Monaspace Xenon\", monospace", "normal", "\"calt\", \"ss02\", \"ss03\", \"ss07\", \"ss08\""),
        Set::Hco => ("\"Ideal Sans SSm\", sans-serif", "\"Operator Mono SSm\", monospace", "normal", "normal"),
    };
    css.push_str(&format!(":root{{--apex-font:{sans};--apex-mono:{mono};--apex-font-features:{sans_features};--apex-mono-features:{mono_features}}}"));
    css
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(css.contains("apexfile://localhost/.apex-font/Go-Regular.ttf"));
        assert!(css.contains("--apex-mono:"));
    }
}
