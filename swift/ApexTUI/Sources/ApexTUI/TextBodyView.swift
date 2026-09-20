//
//  TextBodyView.swift — a window's text, drawn as acme draws it.
//
//  Yellow paper, the selection in acme's darker yellow, the scrollbar
//  down the left. The lines arrive wrapped, so this view only paints and
//  never has to know what a rune offset is; every gesture goes back to
//  the view server, which holds the buffer.
//

import TermKit

class TextBodyView: View {
    var model: TextModel? { didSet { setNeedsDisplay() } }
    /// Draw the caret: the window with the keyboard.
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
                       origin: model.origin, shown: height, total: max(model.total, 1))
        let textWidth = max(0, width - 1)
        guard textWidth > 0 else { return }
        let inner = painter.clipped(to: Rect(x: 1, y: 0, width: textWidth, height: height))
        for (i, line) in model.lines.enumerated() where i < height {
            Draw.line(inner, line, row: i, width: textWidth, ground: Theme.body) { kind in
                switch kind {
                case .sel: return Theme.bodySelected
                case .exec: return Theme.exec
                case .look: return Theme.look
                case .tagname: return Theme.tagName
                case .mark: return Theme.bodySelected
                }
            }
        }
    }

    override func positionCursor() {
        guard focused, let caret = model?.caret else {
            super.positionCursor()
            return
        }
        moveTo(col: 1 + caret.col, row: caret.row)
    }
}
