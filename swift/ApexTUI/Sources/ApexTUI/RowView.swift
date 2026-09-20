//
//  RowView.swift — the row: acme's top line, and the columns under it.
//
//  This is the root of the UI. It owns the frames as they arrive, sizes
//  the column views to the rectangles acme's tiling chose, and turns the
//  pointer and the keyboard into events for the view server. It decides
//  nothing about what a click means: B1, B2 and B3 go over as they are,
//  and the session says what happened.
//

import Foundation
import TermKit

class RowView: View {
    private let bridge: Bridge
    private let topTag = TagView()
    private var columnViews: [UInt64: ColumnView] = [:]
    private var overlay: View?
    private let finder = FinderView()
    private let switcher = SwitcherView()
    private var statusText = ""
    /// Where the columns landed, for the borders between them.
    private var columnFrames: [Rect] = []
    private var lastSize = Size(width: 0, height: 0)
    private var mouseToken: Application.MouseHandlerToken?
    /// The last press, for the double-click acme needs to select a word.
    private var lastClick: (x: Int, y: Int, at: Date, count: Int) = (-1, -1, .distantPast, 0)

    init(bridge: Bridge) {
        self.bridge = bridge
        super.init()
        colorScheme = ColorScheme(normal: Theme.columnGround, focus: Theme.columnGround,
                                  hotNormal: Theme.columnGround, hotFocus: Theme.columnGround)
        topTag.showsBox = true
        addSubview(topTag)
        canFocus = true
        wantMousePositionReports = true

        finder.onOpen = { [weak self] name in
            self?.bridge.send(.open(name: name))
            self?.hideOverlay()
        }
        finder.onDismiss = { [weak self] in
            self?.bridge.send(.dismiss)
            self?.hideOverlay()
        }
        switcher.onPick = { [weak self] name in
            self?.bridge.send(.switchTo(name: name))
            self?.hideOverlay()
        }
        switcher.onDismiss = { [weak self] in
            self?.bridge.send(.dismiss)
            self?.hideOverlay()
        }
    }

    /// Watch every mouse event, wherever it lands: acme's buttons mean
    /// the same thing over any part of the row, and a sweep that leaves
    /// the window it began in still belongs to that window.
    func listenForMouse() {
        mouseToken = Application.addRootMouseHandler { [weak self] event in
            self?.handle(mouse: event)
        }
    }

    // ---- frames --------------------------------------------------------------

    func apply(_ frame: SessionFrame) {
        topTag.model = frame.top
        topTag.notified = frame.notification != nil
        topTag.dirty = false
        topTag.live = !frame.connected

        var live: [UInt64: ColumnView] = [:]
        for column in frame.columns {
            var view: ColumnView
            if let existing = columnViews[column.id] {
                view = existing
            } else {
                view = ColumnView()
                view.onFollow = { [weak self] id, url in self?.follow(url, in: id) }
                addSubview(view)
            }
            view.frame = Rect(x: column.x0, y: column.y0,
                              width: column.x1 - column.x0, height: column.y1 - column.y0)
            view.update(column)
            live[column.id] = view
        }
        for (id, view) in columnViews where live[id] == nil {
            removeSubview(view)
        }
        columnViews = live
        columnFrames = frame.columns.map { Rect(x: $0.x0, y: $0.y0, width: $0.x1 - $0.x0, height: $0.y1 - $0.y0) }

        switch frame.overlay {
        case let .finder(candidates, all):
            finder.set(candidates: candidates, all: all)
            show(overlay: finder)
            finder.focusQuery()
        case let .switcher(sessions, current):
            switcher.set(sessions: sessions, current: current)
            show(overlay: switcher)
        case .message, .none:
            hideOverlay()
        }

        if let snarf = frame.snarf {
            Clipboard.contents = snarf
        }
        statusText = status(of: frame)
        setNeedsDisplay()
        setNeedsLayout()
    }

    private func status(of frame: SessionFrame) -> String {
        var parts: [String] = [frame.title]
        if !frame.connected { parts.append("offline") }
        if frame.fenced { parts.append("fenced — another UI has the session") }
        if let n = frame.notification { parts.append("\u{25C9} \(n)") }
        return parts.joined(separator: "   ")
    }

    /// A link a page's own rendering resolved: hand it to the plumber,
    /// which is what B3 anywhere else would do with it.
    private func follow(_ url: String, in window: UInt64? = nil) {
        bridge.send(.plumb(window: window, text: url))
    }

    // ---- layout --------------------------------------------------------------

    override func layoutSubviews() throws {
        topTag.frame = Rect(x: 0, y: 0, width: bounds.width, height: 1)
        if let overlay {
            let w = min(max(40, bounds.width * 2 / 3), bounds.width - 2)
            let h = min(max(8, bounds.height - 4), bounds.height - 2)
            overlay.frame = Rect(x: (bounds.width - w) / 2, y: max(1, (bounds.height - h) / 3),
                                 width: w, height: h)
        }
        // a size we have not told the session about yet
        let size = Size(width: bounds.width, height: max(0, bounds.height - 1))
        if size != lastSize, size.width > 0, size.height > 0 {
            lastSize = size
            bridge.send(.resize(cols: bounds.width, rows: bounds.height - 1))
        }
        try super.layoutSubviews()
    }

    override func drawContent(in region: Rect, painter: Painter) {
        Draw.fill(painter, bounds, Theme.columnGround)
        // acme's borders, in the cells the tiling left for them: under
        // the row's tag, and down between the columns
        if let top = columnFrames.map({ $0.minY }).min(), top > 1 {
            Draw.rule(painter, y: top - 1, from: 0, to: bounds.width)
        }
        for f in columnFrames where f.minX > 0 {
            Draw.verticalRule(painter, x: f.minX - 1, from: f.minY, to: min(f.maxY, bounds.height - 1))
        }
        // the last line is the status: what the gpui client puts in the
        // window's title bar, which a terminal has not got
        guard bounds.height > 0 else { return }
        painter.attribute = Theme.panelDim
        painter.goto(col: 0, row: bounds.height - 1)
        painter.add(str: Draw.clip(statusText, to: bounds.width)
                        .padding(toLength: bounds.width, withPad: " ", startingAt: 0))
    }

    // ---- overlays ------------------------------------------------------------

    private func show(overlay view: View) {
        guard overlay !== view else { return }
        if let old = overlay { removeSubview(old) }
        overlay = view
        addSubview(view)
        setFocus(view)
        setNeedsLayout()
        setNeedsDisplay()
    }

    private func hideOverlay() {
        guard let view = overlay else { return }
        removeSubview(view)
        overlay = nil
        setNeedsDisplay()
    }

    var overlayIsUp: Bool { overlay != nil }

    // ---- the pointer ---------------------------------------------------------

    private func handle(mouse event: MouseEvent) {
        // while an overlay is up TermKit routes the event to it
        if overlayIsUp { return }
        // the handler sees screen coordinates; the row's are its own
        let here = screenToView(loc: event.absPos)
        let x = here.x
        let y = here.y
        guard x >= 0, y >= 0, y < bounds.height - 1 else { return }   // the status line
        // a page and a markdown window are the UI's own: only it knows
        // where the links and the rendered lines fell
        if handledLocally(x: x, y: y, flags: event.flags) { return }
        let mods = UIEvent.Mods(shift: event.flags.contains(.buttonShift),
                                ctrl: event.flags.contains(.buttonCtrl),
                                alt: event.flags.contains(.buttonAlt))
        // the wheel, where the driver reports it
        if event.flags.contains(.button4Pressed) {
            bridge.send(.mouse(x: x, y: y, button: .wheelup, motion: .down, mods: mods, clicks: 1))
            return
        }
        if event.flags.contains(.button4Released) {
            bridge.send(.mouse(x: x, y: y, button: .wheeldown, motion: .down, mods: mods, clicks: 1))
            return
        }
        let table: [(MouseFlags, UIEvent.MouseButton, UIEvent.Motion)] = [
            (.button1Pressed, .b1, .down), (.button1Released, .b1, .up),
            (.button2Pressed, .b2, .down), (.button2Released, .b2, .up),
            (.button3Pressed, .b3, .down), (.button3Released, .b3, .up),
        ]
        let dragging = event.flags.contains(.mousePosition)
        for (flag, button, motion) in table where event.flags.contains(flag) {
            if dragging {
                // a motion report carries the button that is held
                bridge.send(.mouse(x: x, y: y, button: button, motion: .move, mods: mods, clicks: 1))
                return
            }
            let clicks = motion == .down ? count(x: x, y: y) : 1
            bridge.send(.mouse(x: x, y: y, button: button, motion: motion, mods: mods, clicks: clicks))
            return
        }
        if dragging {
            bridge.send(.mouse(x: x, y: y, button: .b1, motion: .move, mods: mods, clicks: 0))
        }
    }

    /// A gesture the UI answers itself: B3 on a link in a page, and the
    /// wheel or a click on a rendered window's scrollbar. Everything
    /// else belongs to the session.
    private func handledLocally(x: Int, y: Int, flags: MouseFlags) -> Bool {
        guard let (window, origin) = windowAt(x: x, y: y) else { return false }
        let local = Point(x: x - origin.x, y: y - origin.y)
        if let browser = window.browser {
            let inBody = local.y - window.bodyTop
            guard inBody >= 0 else { return false }
            if flags.contains(.button3Pressed) || flags.contains(.button3Clicked) {
                if let url = browser.link(atRow: inBody, column: max(0, local.x - 1)) {
                    follow(url, in: window.id)
                    return true
                }
            }
            if flags.contains(.button4Pressed) { browser.scroll(by: -3); return true }
            if flags.contains(.button4Released) { browser.scroll(by: 3); return true }
            return false
        }
        if let markdown = window.markdown {
            if flags.contains(.button4Pressed) { markdown.scroll(by: -3); return true }
            if flags.contains(.button4Released) { markdown.scroll(by: 3); return true }
            return false
        }
        return false
    }

    /// The window at a point of the row, and where that window starts.
    private func windowAt(x: Int, y: Int) -> (WindowView, Point)? {
        for column in columnViews.values where column.frame.contains(x: x, y: y) {
            let inColumn = Point(x: x - column.frame.minX, y: y - column.frame.minY)
            if let (window, origin) = column.windowAt(inColumn) {
                return (window, Point(x: column.frame.minX + origin.x, y: column.frame.minY + origin.y))
            }
        }
        return nil
    }

    /// acme selects a word on the second click in the same place.
    private func count(x: Int, y: Int) -> Int {
        let now = Date()
        if x == lastClick.x, y == lastClick.y, now.timeIntervalSince(lastClick.at) < 0.4 {
            lastClick = (x, y, now, lastClick.count + 1)
        } else {
            lastClick = (x, y, now, 1)
        }
        return lastClick.count
    }

    // ---- the keyboard --------------------------------------------------------

    /// Page a rendered window, when the active one is rendered here.
    private func scrolledLocally(by lines: Int) -> Bool {
        guard let active = activeWindow() else { return false }
        if let browser = active.browser { browser.scroll(by: lines); return true }
        if let markdown = active.markdown { markdown.scroll(by: lines); return true }
        return false
    }

    private func activeWindow() -> WindowView? {
        for column in columnViews.values {
            if let w = column.activeWindow() { return w }
        }
        return nil
    }

    override func processKey(event: KeyEvent) -> Bool {
        if overlayIsUp { return super.processKey(event: event) }
        let mods = UIEvent.Mods(shift: false, ctrl: event.isControl, alt: event.isAlt)
        switch event.key {
        case .controlQ:
            bridge.send(.quit)
            Application.requestStop()
            return true
        case .controlP:
            bridge.send(.finder(all: false))
            return true
        case .controlJ, .controlM:
            bridge.send(.key(.enter, mods))
        case .controlI:
            bridge.send(.key(.tab, mods))
        case .controlH, .delete:
            bridge.send(.key(.backspace, mods))
        case .deleteChar:
            bridge.send(.key(.delete, mods))
        case .controlW:
            bridge.send(.key(.eraseWord, mods))
        case .controlU:
            bridge.send(.key(.eraseLine, mods))
        case .esc:
            bridge.send(.key(.escape, mods))
        case .cursorUp:
            bridge.send(.key(.up, mods))
        case .cursorDown:
            bridge.send(.key(.down, mods))
        case .cursorLeft:
            bridge.send(.key(.left, mods))
        case .cursorRight:
            bridge.send(.key(.right, mods))
        case .shiftCursorLeft:
            bridge.send(.key(.left, UIEvent.Mods(shift: true, ctrl: false, alt: false)))
        case .shiftCursorRight:
            bridge.send(.key(.right, UIEvent.Mods(shift: true, ctrl: false, alt: false)))
        case .home:
            bridge.send(.key(.home, mods))
        case .end:
            bridge.send(.key(.end, mods))
        case .pageUp:
            if scrolledLocally(by: -max(1, bounds.height - 3)) { return true }
            bridge.send(.key(.pageUp, mods))
        case .pageDown:
            if scrolledLocally(by: max(1, bounds.height - 3)) { return true }
            bridge.send(.key(.pageDown, mods))
        case let .letter(ch):
            if event.isControl || event.isAlt {
                // a control key belongs to the program in a terminal
                bridge.send(.text(String(ch)))
            } else {
                bridge.send(.text(String(ch)))
            }
        default:
            return false
        }
        return true
    }
}
