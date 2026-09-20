//
//  TagView.swift — a window's or a column's tag, with its layout box.
//
//  acme's tag is a one-line (or more) text on cyan paper, with a small
//  square at its left that is both the handle you drag a window by and
//  the light that says whether it is dirty, a tool's, or wanting you.
//

import TermKit

class TagView: View {
    /// The tag's text, as the view server laid it out.
    var model: TextModel? { didSet { setNeedsDisplay() } }
    /// The square at the left, and what it should say.
    var showsBox = true
    var dirty = false
    var live = false
    var notified = false

    override init() {
        super.init()
        colorScheme = ColorScheme(normal: Theme.tag, focus: Theme.tag, hotNormal: Theme.tag, hotFocus: Theme.tag)
        canFocus = false
    }

    override func drawContent(in region: Rect, painter: Painter) {
        let width = bounds.width
        let height = bounds.height
        guard width > 0, height > 0 else { return }
        Draw.fill(painter, bounds, Theme.tag)
        let boxWidth = showsBox ? 1 : 0
        if showsBox {
            painter.attribute = Theme.handle(dirty: dirty, live: live, notified: notified, isTag: true)
            painter.goto(col: 0, row: 0)
            painter.add(str: boxGlyph)
            // the box is one line tall; the rest of the gutter is paper
            if height > 1 {
                painter.attribute = Theme.tag
                for r in 1..<height {
                    painter.goto(col: 0, row: r)
                    painter.add(str: " ")
                }
            }
        }
        guard let model else { return }
        let textWidth = max(0, width - boxWidth)
        guard textWidth > 0 else { return }
        let inner = painter.clipped(to: Rect(x: boxWidth, y: 0, width: textWidth, height: height))
        for (i, line) in model.lines.enumerated() where i < height {
            Draw.line(inner, line, row: i, width: textWidth, ground: Theme.tag) { kind in
                switch kind {
                case .sel: return Theme.tagSelected
                case .exec: return Theme.exec
                case .look: return Theme.look
                case .tagname: return Theme.tagName
                case .mark: return Theme.bodySelected
                }
            }
        }
    }

    /// acme draws the box as a filled square when the window is dirty
    /// and a hollow one when it is clean.
    private var boxGlyph: String {
        if notified { return "\u{25C9}" }   // a ringed dot: this one wants you
        if dirty { return "\u{25A0}" }      // filled: unsaved
        return "\u{25A1}"                   // hollow: clean
    }
}
