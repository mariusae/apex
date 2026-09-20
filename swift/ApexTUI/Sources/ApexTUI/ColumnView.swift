//
//  ColumnView.swift — a column: its own tag, and the windows in it.
//
//  A column squeezed to a strip (B3 on another column's box, or B4 on
//  its own) has no room for a tag across, so its name is drawn down the
//  side, which is what a terminal can do in place of acme's rotated one.
//

import TermKit

class ColumnView: View {
    private(set) var id: UInt64 = 0
    private let tagView = TagView()
    private var windowViews: [UInt64: WindowView] = [:]
    private var strip = false
    private var name = ""
    /// Where each window starts, so acme's borders can be drawn between
    /// them: the tiling leaves the cells, the column fills them.
    private var windowTops: [Int] = []

    var onFollow: ((UInt64, String) -> Void)?

    override init() {
        super.init()
        colorScheme = ColorScheme(normal: Theme.columnGround, focus: Theme.columnGround,
                                  hotNormal: Theme.columnGround, hotFocus: Theme.columnGround)
        addSubview(tagView)
    }

    func update(_ model: ColumnModel) {
        id = model.id
        strip = model.strip
        name = model.tag.lines.first?.text ?? ""
        tagView.model = model.tag
        // windows come and go; keep the views of the ones that stayed
        var live: [UInt64: WindowView] = [:]
        for window in model.windows {
            var view: WindowView
            if let existing = windowViews[window.id] {
                view = existing
            } else {
                view = WindowView()
                view.onFollow = { [weak self] id, url in self?.onFollow?(id, url) }
                addSubview(view)
            }
            view.update(window)
            view.frame = Rect(x: window.x0 - model.x0, y: window.y0 - model.y0,
                              width: window.x1 - window.x0, height: window.y1 - window.y0)
            live[window.id] = view
        }
        for (id, view) in windowViews where live[id] == nil {
            removeSubview(view)
        }
        windowViews = live
        windowTops = model.windows.map { $0.y0 - model.y0 }
        tagView.frame = Rect(x: 0, y: 0, width: bounds.width, height: strip ? 0 : 1)
        setNeedsDisplay()
    }

    /// The window at a point of the column, and where it starts.
    func windowAt(_ point: Point) -> (WindowView, Point)? {
        for view in windowViews.values where view.frame.contains(point) {
            return (view, view.frame.origin)
        }
        return nil
    }

    /// The window the pointer was last over, which the session marks.
    func activeWindow() -> WindowView? {
        windowViews.values.first { $0.isActive }
    }

    override func drawContent(in region: Rect, painter: Painter) {
        Draw.fill(painter, bounds, Theme.columnGround)
        if !strip {
            // a rule above every window: the column tag's border, and
            // the border between one window and the next
            for top in windowTops where top > 0 && top - 1 < bounds.height {
                Draw.rule(painter, y: top - 1, from: 0, to: bounds.width)
            }
        }
        guard strip else { return }
        // the strip: the column's name, a letter to a row
        painter.attribute = Theme.handle(dirty: false, live: false, notified: false, isTag: false)
        painter.goto(col: 0, row: 0)
        painter.add(str: "\u{25A1}")
        painter.attribute = Theme.tag
        for (i, ch) in name.prefix(max(0, bounds.height - 1)).enumerated() {
            painter.goto(col: 0, row: i + 1)
            painter.add(str: String(ch))
        }
    }
}
