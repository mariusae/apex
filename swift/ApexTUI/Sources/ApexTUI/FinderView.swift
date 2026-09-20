//
//  FinderView.swift — ⌘P, as a TermKit panel.
//
//  A `TextField` for the query and a `ListView` for what matches: both
//  are TermKit's own, so the editing, the selection and the paging come
//  from the toolkit rather than from apex. The ranking is `Fuzzy`, which
//  is the gpui client's, so the same file comes first.
//

import TermKit

class FinderView: View, ListViewDataSource, ListViewDelegate {
    private let field = TextField("")
    private let list = ListView()
    private let caption = Label("Open")
    private var candidates: [Candidate] = []
    private var shown: [Candidate] = []

    /// A file was chosen.
    var onOpen: ((String) -> Void)?
    /// Escape, or a click outside.
    var onDismiss: (() -> Void)?

    override init() {
        super.init()
        let scheme = ColorScheme(normal: Theme.panel, focus: Theme.panelChosen,
                                 hotNormal: Theme.panel, hotFocus: Theme.panelChosen)
        colorScheme = scheme
        border = .single
        caption.colorScheme = scheme
        field.colorScheme = scheme
        list.colorScheme = scheme
        list.allowMarking = false
        list.allowsMultipleSelection = false
        list.dataSource = self
        list.delegate = self
        field.textChanged = { [weak self] _, _ in self?.refilter() }
        field.onSubmit = { [weak self] _ in self?.choose() }
        list.activate = { [weak self] _ in
            self?.choose()
            return true
        }
        addSubviews([caption, field, list])
        canFocus = true
    }

    func set(candidates: [Candidate], all: Bool) {
        self.candidates = candidates
        caption.text = all ? "Open (every session)" : "Open"
        refilter()
    }

    func focusQuery() {
        setFocus(field)
    }

    private func refilter() {
        shown = Fuzzy.rank(candidates, query: field.text)
        list.reload()
        if !shown.isEmpty { list.selectedItem = 0 }
        setNeedsDisplay()
    }

    private func choose() {
        guard list.selectedItem >= 0, list.selectedItem < shown.count else { return }
        onOpen?(shown[list.selectedItem].name)
    }

    override func processKey(event: KeyEvent) -> Bool {
        switch event.key {
        case .esc:
            onDismiss?()
            return true
        case .cursorUp, .controlP:
            return list.moveSelectionUp()
        case .cursorDown, .controlN:
            return list.moveSelectionDown()
        case .pageUp:
            return list.movePageUp()
        case .pageDown:
            return list.movePageDown()
        case .controlJ, .controlM:
            choose()
            return true
        default:
            return super.processKey(event: event)
        }
    }

    override func layoutSubviews() throws {
        let inner = contentFrame
        caption.frame = Rect(x: 0, y: 0, width: inner.width, height: 1)
        field.frame = Rect(x: 0, y: 1, width: inner.width, height: 1)
        list.frame = Rect(x: 0, y: 2, width: inner.width, height: max(0, inner.height - 2))
        try super.layoutSubviews()
    }

    // ---- the list -----------------------------------------------------------

    func getCount(listView: ListView) -> Int { shown.count }

    func isMarked(listView: ListView, item: Int) -> Bool { false }

    func setMark(listView: ListView, item: Int, state: Bool) {}

    func render(listView: ListView, painter: Painter, selected: Bool, item: Int, col: Int, line: Int, width: Int) {
        guard item < shown.count else { return }
        let c = shown[item]
        painter.attribute = selected ? Theme.panelChosen : Theme.panel
        painter.goto(col: col, row: line)
        painter.add(str: String(repeating: " ", count: max(0, width)))
        // the name, then the directory it is in, dimmed
        let parts = split(c.name)
        let badge = badge(for: c)
        let nameWidth = min(parts.name.count, max(0, width - badge.count))
        painter.goto(col: col, row: line)
        painter.attribute = selected ? Theme.panelChosen : Theme.panel
        painter.add(str: Draw.clip(parts.name, to: nameWidth))
        let used = nameWidth + badge.count
        if !parts.directory.isEmpty && width > used + 2 {
            painter.attribute = selected ? Theme.panelChosen : Theme.panelDim
            painter.goto(col: col + nameWidth + 1, row: line)
            painter.add(str: Draw.clip(parts.directory, to: max(0, width - used - 1)))
        }
        if !badge.isEmpty {
            painter.attribute = selected ? Theme.panelChosen : Theme.panelDim
            painter.goto(col: col + max(0, width - badge.count), row: line)
            painter.add(str: badge)
        }
    }

    func selectionChanged(listView: ListView) {}

    func activate(listView: ListView, item: Int) -> Bool {
        choose()
        return true
    }

    private func split(_ path: String) -> (name: String, directory: String) {
        guard let i = path.lastIndex(of: "/") else { return (path, "") }
        return (String(path[path.index(after: i)...]), String(path[path.startIndex..<i]))
    }

    private func badge(for c: Candidate) -> String {
        switch c.kind {
        case .term: return " term"
        case .dir: return " dir"
        case .errors: return " +Errors"
        case .web: return " web"
        case .file: return c.open ? "" : " closed"
        }
    }
}
