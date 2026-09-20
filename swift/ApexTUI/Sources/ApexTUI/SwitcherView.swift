//
//  SwitcherView.swift — the sessions this daemon has.
//
//  The same panel as the finder, over the daemon's sessions rather than
//  the session's files: a `ListView`, and the one you are in marked.
//

import TermKit

class SwitcherView: View, ListViewDataSource, ListViewDelegate {
    private let list = ListView()
    private let caption = Label("Sessions")
    private var sessions: [Candidate] = []
    private var current = ""

    var onPick: ((String) -> Void)?
    var onDismiss: (() -> Void)?

    override init() {
        super.init()
        let scheme = ColorScheme(normal: Theme.panel, focus: Theme.panelChosen,
                                 hotNormal: Theme.panel, hotFocus: Theme.panelChosen)
        colorScheme = scheme
        border = .single
        caption.colorScheme = scheme
        list.colorScheme = scheme
        list.allowMarking = false
        list.dataSource = self
        list.delegate = self
        addSubviews([caption, list])
        canFocus = true
    }

    func set(sessions: [Candidate], current: String) {
        self.sessions = sessions
        self.current = current
        list.reload()
        setNeedsDisplay()
    }

    override func processKey(event: KeyEvent) -> Bool {
        switch event.key {
        case .esc:
            onDismiss?()
            return true
        case .controlJ, .controlM:
            if list.selectedItem >= 0 && list.selectedItem < sessions.count {
                onPick?(sessions[list.selectedItem].name)
            }
            return true
        default:
            return super.processKey(event: event)
        }
    }

    override func layoutSubviews() throws {
        let inner = contentFrame
        caption.frame = Rect(x: 0, y: 0, width: inner.width, height: 1)
        list.frame = Rect(x: 0, y: 1, width: inner.width, height: max(0, inner.height - 1))
        try super.layoutSubviews()
    }

    func getCount(listView: ListView) -> Int { sessions.count }
    func isMarked(listView: ListView, item: Int) -> Bool { false }
    func setMark(listView: ListView, item: Int, state: Bool) {}
    func selectionChanged(listView: ListView) {}

    func activate(listView: ListView, item: Int) -> Bool {
        guard item < sessions.count else { return false }
        onPick?(sessions[item].name)
        return true
    }

    func render(listView: ListView, painter: Painter, selected: Bool, item: Int, col: Int, line: Int, width: Int) {
        guard item < sessions.count else { return }
        let s = sessions[item]
        painter.attribute = selected ? Theme.panelChosen : Theme.panel
        painter.goto(col: col, row: line)
        painter.add(str: String(repeating: " ", count: max(0, width)))
        painter.goto(col: col, row: line)
        let mark = s.name == current ? "\u{2022} " : "  "
        painter.add(str: Draw.clip(mark + s.name, to: width))
    }
}
