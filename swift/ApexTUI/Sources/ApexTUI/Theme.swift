//
//  Theme.swift — acme's colours, on a terminal.
//
//  The values are plan9port's, the same ones `crates/apex-client/src/theme.rs`
//  carries: yellow paper for a body, cyan for a tag, and the handle
//  colours that say what a window is doing. A terminal that can do RGB
//  gets them exactly; on a 256-colour terminal TermKit quantises them,
//  and on eight colours it falls back to the nearest name.
//

import TermKit

struct Palette {
    let bodyBackground: Int
    let bodySelection: Int
    let bodyBorder: Int
    let tagBackground: Int
    let tagSelection: Int
    let tagBorder: Int
    let text: Int
    let sweepText: Int
    let border: Int
    let column: Int
    let dirty: Int
    let live: Int
    let fenced: Int
    let notified: Int
    let execHighlight: Int
    let lookHighlight: Int
    let panelBackground: Int
    let panelText: Int
    let panelDim: Int
    let panelChosenBackground: Int
    let panelChosenText: Int
    let ansi: [Int]

    /// acme's own, from plan9port's acme.c.
    static let light = Palette(
        bodyBackground: 0xFFFFEA,
        bodySelection: 0xEEEE9E,
        bodyBorder: 0x99994C,
        tagBackground: 0xEAFFFF,
        tagSelection: 0x9EEEEE,
        tagBorder: 0x8888CC,
        text: 0x000000,
        sweepText: 0xFFFFFF,
        border: 0x000000,
        column: 0xFFFFFF,
        dirty: 0x000099,
        live: 0xB24073,
        fenced: 0xAA0000,
        notified: 0x0080FF,
        execHighlight: 0xAA0000,
        lookHighlight: 0x006600,
        panelBackground: 0xEAFFFF,
        panelText: 0x000000,
        panelDim: 0x555555,
        panelChosenBackground: 0x9EEEEE,
        panelChosenText: 0x000000,
        ansi: [0x000000, 0xCC241D, 0x3C8A2A, 0xB08A00, 0x1C4FD6, 0x9A2D9A, 0x0F8A8A, 0xBBBBBB,
               0x555555, 0xFF5555, 0x55C055, 0xD6C000, 0x5580FF, 0xDD55DD, 0x33C0C0, 0xFFFFFF]
    )

    static let dark = Palette(
        bodyBackground: 0x1A1A16,
        bodySelection: 0x3A3A22,
        bodyBorder: 0x66663A,
        tagBackground: 0x16242A,
        tagSelection: 0x2A4A52,
        tagBorder: 0x445588,
        text: 0xD8D8C8,
        sweepText: 0x000000,
        border: 0x000000,
        column: 0x101010,
        dirty: 0x6688FF,
        live: 0xE07AA8,
        fenced: 0xFF6666,
        notified: 0x66C0FF,
        execHighlight: 0xFF6666,
        lookHighlight: 0x66CC66,
        panelBackground: 0x16242A,
        panelText: 0xD8D8C8,
        panelDim: 0x8A8A80,
        panelChosenBackground: 0x2A4A52,
        panelChosenText: 0xFFFFFF,
        ansi: [0x000000, 0xCC241D, 0x3C8A2A, 0xB08A00, 0x1C4FD6, 0x9A2D9A, 0x0F8A8A, 0xBBBBBB,
               0x555555, 0xFF5555, 0x55C055, 0xD6C000, 0x5580FF, 0xDD55DD, 0x33C0C0, 0xFFFFFF]
    )
}

/// The one palette in force, and the attributes made from it.
enum Theme {
    /// `APEX_THEME=dark` picks the dark one; the default is acme's.
    static var palette: Palette = {
        if let v = ProcessInfo.processInfo.environment["APEX_THEME"], v.lowercased() == "dark" {
            return .dark
        }
        return .light
    }()

    static func color(_ hex: Int) -> Color {
        .rgb((hex >> 16) & 0xff, (hex >> 8) & 0xff, hex & 0xff)
    }

    private static var cache: [Int: Attribute] = [:]

    /// An attribute for a pair of `0xRRGGBB` colours, made once.
    static func attr(_ fore: Int, _ back: Int, _ flags: CellFlags = []) -> Attribute {
        let key = (fore &* 31 &+ back) &* 31 &+ Int(flags.rawValue)
        if let a = cache[key] { return a }
        let a = Application.makeAttribute(fore: color(fore), back: color(back), flags: flags)
        cache[key] = a
        return a
    }

    static func forgetCache() { cache.removeAll() }

    // The attributes the views ask for by name.

    static var body: Attribute { attr(palette.text, palette.bodyBackground) }
    static var bodySelected: Attribute { attr(palette.text, palette.bodySelection) }
    static var tag: Attribute { attr(palette.text, palette.tagBackground) }
    static var tagSelected: Attribute { attr(palette.text, palette.tagSelection) }
    static var tagName: Attribute { attr(palette.text, palette.tagBackground, [.bold]) }
    static var exec: Attribute { attr(palette.sweepText, palette.execHighlight) }
    static var look: Attribute { attr(palette.sweepText, palette.lookHighlight) }
    static var scrollbar: Attribute { attr(palette.bodyBorder, palette.bodyBackground) }
    static var scrollbarThumb: Attribute { attr(palette.bodyBorder, palette.bodyBorder) }
    static var columnGround: Attribute { attr(palette.text, palette.column) }
    static var panel: Attribute { attr(palette.panelText, palette.panelBackground) }
    static var panelDim: Attribute { attr(palette.panelDim, palette.panelBackground) }
    static var panelChosen: Attribute { attr(palette.panelChosenText, palette.panelChosenBackground) }

    /// The layout box of a window or a column, which says what state it
    /// is in: acme's blue square when dirty, and apex's own colours for
    /// a tool's window and for a session that wants the user.
    static func handle(dirty: Bool, live: Bool, notified: Bool, isTag: Bool) -> Attribute {
        let ground = isTag ? palette.tagBackground : palette.column
        if notified { return attr(palette.notified, ground, [.bold]) }
        if dirty { return attr(palette.dirty, ground, [.bold]) }
        if live { return attr(palette.live, ground) }
        return attr(palette.tagBorder, ground)
    }

    /// A terminal cell's colour, unpacked as `entry.rs` packs it: `0xff`
    /// a colour the program named, `0xfe` an ANSI index the theme draws,
    /// `0xfd` the theme's own ink or paper.
    static func termColor(_ packed: UInt32, background: Bool) -> Int {
        switch packed >> 24 {
        case 0xfe: return palette.ansi[Int(packed & 0xf)]
        case 0xfd: return (packed & 1) == 0 ? palette.text : palette.bodyBackground
        case 0: return background ? palette.bodyBackground : palette.text
        default: return Int(packed & 0x00ff_ffff)
        }
    }

    static func termFlags(_ flags: UInt8) -> CellFlags {
        var out: CellFlags = []
        if flags & 0x01 != 0 { out.insert(.bold) }        // FLAG_BOLD
        if flags & 0x02 != 0 { out.insert(.standout) }    // FLAG_ITALIC, which a terminal cell has no bit for
        if flags & 0x04 != 0 { out.insert(.underline) }   // FLAG_UNDERLINE
        return out
    }
}
