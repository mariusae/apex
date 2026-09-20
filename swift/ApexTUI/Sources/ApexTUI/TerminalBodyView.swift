//
//  TerminalBodyView.swift — a terminal window's grid.
//
//  apex runs the emulator on the server (DESIGN.md §2): the session owns
//  the pty and the screen, and a client is handed cells. So this is not
//  TermKit's `TerminalView`, which drives SwiftTerm over a byte stream —
//  there is no byte stream here to give it, and running a second
//  emulator over the first would lose the scrollback the session keeps
//  and break detach and re-attach. What is left is exactly a painter:
//  colours as the program asked for them, the cursor, and acme's
//  scrollbar over the session's scrollback.
//

import TermKit

class TerminalBodyView: View {
    var model: TermModel? { didSet { setNeedsDisplay() } }
    var focused = false { didSet { setNeedsDisplay() } }

    override init() {
        super.init()
        colorScheme = ColorScheme(normal: Theme.body, focus: Theme.body, hotNormal: Theme.body, hotFocus: Theme.body)
        canFocus = true
    }

    override func drawContent(in region: Rect, painter: Painter) {
        let width = bounds.width
        let height = bounds.height
        guard width > 0, height > 0 else { return }
        Draw.fill(painter, bounds, Theme.body)
        guard let model else { return }
        Draw.scrollbar(painter, x: 0, y: 0, height: height,
                       origin: Int(model.origin), shown: height, total: Int(max(model.total, 1)))
        let textWidth = max(0, width - 1)
        guard textWidth > 0 else { return }
        let inner = painter.clipped(to: Rect(x: 1, y: 0, width: textWidth, height: height))
        for (r, row) in model.rows.enumerated() where r < height {
            draw(row: row, at: r, width: textWidth, painter: inner, selected: selection(row: r, width: textWidth))
        }
        if let cursor = model.cursor, cursor.row < height, cursor.col < textWidth {
            // a cursor on a terminal that has exited is drawn hollow
            inner.attribute = model.exited
                ? Theme.attr(Theme.palette.text, Theme.palette.bodyBackground, [.underline])
                : Theme.attr(Theme.palette.bodyBackground, Theme.palette.text)
            inner.goto(col: cursor.col, row: cursor.row)
            let ch = character(of: model.rows, row: cursor.row, col: cursor.col)
            inner.add(str: String(ch))
        }
    }

    /// The columns of `row` that the sweep covers, if any.
    private func selection(row: Int, width: Int) -> Range<Int>? {
        guard let sel = model?.sel, sel.count == 2 else { return nil }
        let a = sel[0], b = sel[1]
        let (first, last) = (a.row <= b.row) ? (a, b) : (b, a)
        guard row >= first.row, row <= last.row else { return nil }
        let lo = row == first.row ? first.col : 0
        let hi = row == last.row ? last.col : width
        guard lo < hi else { return nil }
        return max(0, lo)..<min(width, hi)
    }

    private func character(of rows: [TermRowModel], row: Int, col: Int) -> Character {
        guard row < rows.count else { return " " }
        let chars = Array(rows[row].text)
        return col < chars.count ? chars[col] : " "
    }

    private func draw(row: TermRowModel, at r: Int, width: Int, painter: Painter, selected: Range<Int>?) {
        // start from the terminal's own ink on acme's paper, then lay the
        // program's runs over it, then the sweep over those
        var fore = [Int](repeating: Theme.palette.text, count: width)
        var back = [Int](repeating: Theme.palette.bodyBackground, count: width)
        var flags = [CellFlags](repeating: [], count: width)
        for run in row.runs {
            let f = Theme.termColor(run.fg, background: false)
            let b = Theme.termColor(run.bg, background: true)
            let fl = Theme.termFlags(run.flags)
            for c in run.start..<min(run.start + run.len, width) where c >= 0 {
                fore[c] = f
                back[c] = b
                flags[c] = fl
            }
        }
        if let selected {
            for c in selected {
                back[c] = Theme.palette.bodySelection
                fore[c] = Theme.palette.text
            }
        }
        let chars = Array(row.text)
        var col = 0
        while col < width {
            let (f, b, fl) = (fore[col], back[col], flags[col])
            painter.attribute = Theme.attr(f, b, fl)
            painter.goto(col: col, row: r)
            var run = ""
            while col < width && fore[col] == f && back[col] == b && flags[col] == fl {
                run.append(col < chars.count ? chars[col] : " ")
                col += 1
            }
            painter.add(str: run)
        }
    }

    override func positionCursor() {
        guard focused, let cursor = model?.cursor else {
            super.positionCursor()
            return
        }
        moveTo(col: 1 + cursor.col, row: cursor.row)
    }
}
