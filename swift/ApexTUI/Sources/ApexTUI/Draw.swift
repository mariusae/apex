//
//  Draw.swift — painting a model line onto a painter.
//
//  A line arrives already wrapped and tab-expanded, with spans over
//  display columns, so drawing one is a walk over runs. Every apex view
//  draws through here, which is what keeps a tag, a body and a finder
//  row looking like the same editor.
//

import TermKit

enum Draw {
    /// Paint `line` at `row`, `width` cells across, from `ground`, with
    /// each span's own attribute over it.
    static func line(_ painter: Painter, _ line: Line, row: Int, width: Int,
                     ground: Attribute, attribute: (SpanKind) -> Attribute) {
        guard width > 0 else { return }
        let chars = Array(line.text)
        // the attribute of every cell: the ground, then each span over it
        var attrs = [Attribute](repeating: ground, count: width)
        for span in line.spans {
            let a = attribute(span.kind)
            // an empty span is the caret's own cell: one column wide
            let len = max(span.len, span.kind == .sel ? 0 : 1)
            guard len > 0 else { continue }
            for c in span.start..<min(span.start + len, width) where c >= 0 {
                attrs[c] = a
            }
        }
        var col = 0
        var i = 0
        while col < width {
            let attr = attrs[col]
            painter.attribute = attr
            painter.goto(col: col, row: row)
            var run = ""
            // one run per stretch of equal attribute
            while col < width && attrs[col] == attr {
                if i < chars.count {
                    let ch = chars[i]
                    run.append(ch)
                    col += max(1, Draw.width(of: ch))
                    i += 1
                } else {
                    run.append(" ")
                    col += 1
                }
            }
            painter.add(str: run)
        }
    }

    /// Fill `rect` with an attribute.
    static func fill(_ painter: Painter, _ rect: Rect, _ attribute: Attribute) {
        painter.attribute = attribute
        painter.clear(rect)
    }

    /// Cells a character takes: the same rule the view server wraps by.
    static func width(of ch: Character) -> Int {
        guard let u = ch.unicodeScalars.first?.value else { return 1 }
        switch u {
        case 0x0300...0x036F, 0x200B...0x200F, 0xFEFF:
            return 0
        case 0x1100...0x115F, 0x2E80...0xA4CF, 0xAC00...0xD7A3, 0xF900...0xFAFF,
             0xFE30...0xFE6F, 0xFF00...0xFF60, 0xFFE0...0xFFE6,
             0x1F300...0x1F64F, 0x1F900...0x1F9FF, 0x20000...0x3FFFD:
            return 2
        default:
            return 1
        }
    }

    /// acme's scrollbar: a bar down the left of a body with the shown
    /// part filled in.
    static func scrollbar(_ painter: Painter, x: Int, y: Int, height: Int,
                          origin: Int, shown: Int, total: Int) {
        guard height > 0 else { return }
        painter.attribute = Theme.scrollbar
        for r in 0..<height {
            painter.goto(col: x, row: y + r)
            painter.add(str: "\u{2502}")
        }
        guard total > 0 else { return }
        let top = min(height - 1, origin * height / max(total, 1))
        let size = max(1, min(height - top, shown * height / max(total, 1)))
        painter.attribute = Theme.scrollbarThumb
        for r in top..<min(height, top + size) {
            painter.goto(col: x, row: y + r)
            painter.add(str: "\u{2588}")
        }
    }

    /// acme's border, drawn as a rule across the cells the tiling left
    /// for it. In acme the border is three pixels of black; in a
    /// terminal the nearest thing is a line.
    static func rule(_ painter: Painter, y: Int, from x0: Int, to x1: Int) {
        guard x1 > x0 else { return }
        painter.attribute = Theme.attr(Theme.palette.border, Theme.palette.column)
        painter.goto(col: x0, row: y)
        painter.add(str: String(repeating: "\u{2500}", count: x1 - x0))
    }

    /// The same, down a column boundary.
    static func verticalRule(_ painter: Painter, x: Int, from y0: Int, to y1: Int) {
        guard y1 > y0 else { return }
        painter.attribute = Theme.attr(Theme.palette.border, Theme.palette.column)
        for y in y0..<y1 {
            painter.goto(col: x, row: y)
            painter.add(str: "\u{2502}")
        }
    }

    /// Cut `text` to `width` cells, with an ellipsis where it was cut.
    static func clip(_ text: String, to cells: Int) -> String {
        guard cells > 0 else { return "" }
        var out = ""
        var w = 0
        for ch in text {
            let cw = max(1, Draw.width(of: ch))
            if w + cw > cells {
                if out.isEmpty { return "" }
                out.removeLast()
                return out + "\u{2026}"
            }
            out.append(ch)
            w += cw
        }
        return out
    }
}
