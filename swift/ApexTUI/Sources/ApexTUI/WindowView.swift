//
//  WindowView.swift — one acme window: a tag, and a body under it.
//
//  Which body a window has can change under it (a file becomes a
//  terminal, a page is opened), so the view keeps one of each kind and
//  shows the one the model asks for. The rectangle is the view server's:
//  acme's tiling decided it, in cells.
//

import TermKit

class WindowView: View {
    private(set) var id: UInt64 = 0
    private let tagView = TagView()
    private let textView = TextBodyView()
    private let terminalView = TerminalBodyView()
    private let markdownView = MarkdownBodyView()
    private let browserView = BrowserView()
    private var bodyView: View?
    private var taglines = 1
    /// The body's rectangle, in this window's coordinates: the tiling's,
    /// so the border it leaves between tag and body is left alone.
    private var bodyRect = Rect(x: 0, y: 1, width: 0, height: 0)
    /// The pointer is in this window (the session says so).
    private(set) var isActive = false

    /// Follow a link in a page: the UI asks the session to go there.
    var onFollow: ((UInt64, String) -> Void)?

    override init() {
        super.init()
        colorScheme = ColorScheme(normal: Theme.body, focus: Theme.body, hotNormal: Theme.body, hotFocus: Theme.body)
        addSubview(tagView)
        browserView.onFollow = { [weak self] url in
            guard let self else { return }
            self.onFollow?(self.id, url)
        }
    }

    func update(_ model: WindowModel) {
        id = model.id
        taglines = max(1, model.taglines)
        let r = model.bodyRect
        bodyRect = Rect(x: r.x, y: r.y, width: r.w, height: r.h)
        tagView.model = model.tag
        tagView.dirty = model.dirty
        tagView.live = model.working
        tagView.notified = model.notified
        isActive = model.active
        switch model.body {
        case let .text(text):
            show(textView)
            textView.model = text
            textView.focused = model.active
        case let .term(term):
            show(terminalView)
            terminalView.model = term
            terminalView.focused = model.active
        case let .markdown(_, source, origin):
            show(markdownView)
            markdownView.set(source: source, origin: origin)
        case let .web(_, url, html, origin, loading):
            show(browserView)
            browserView.set(url: url, html: html, origin: origin, loading: loading)
        }
        setNeedsDisplay()
    }

    private func show(_ view: View) {
        guard bodyView !== view else { return }
        if let old = bodyView { removeSubview(old) }
        bodyView = view
        addSubview(view)
        setNeedsLayout()
    }

    /// The first row of the body, below the tag and its border.
    var bodyTop: Int { bodyRect.minY }

    /// The page this window shows, when it shows one.
    var browser: BrowserView? { bodyView === browserView ? browserView : nil }
    var markdown: MarkdownBodyView? { bodyView === markdownView ? markdownView : nil }

    override func layoutSubviews() throws {
        let tagHeight = min(taglines, bounds.height)
        tagView.frame = Rect(x: 0, y: 0, width: bounds.width, height: tagHeight)
        bodyView?.frame = Rect(x: max(0, bodyRect.minX), y: max(0, bodyRect.minY),
                               width: min(bodyRect.width, bounds.width),
                               height: min(bodyRect.height, max(0, bounds.height - bodyRect.minY)))
        try super.layoutSubviews()
    }

    override func drawContent(in region: Rect, painter: Painter) {
        Draw.fill(painter, bounds, Theme.body)
        // what the tiling left between the tag and the body is acme's
        // border: a rule, as acme draws one
        let top = bodyRect.minY
        guard top > taglines, top - 1 < bounds.height else { return }
        Draw.rule(painter, y: top - 1, from: 0, to: bounds.width)
    }
}
