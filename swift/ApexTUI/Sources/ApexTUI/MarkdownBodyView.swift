//
//  MarkdownBodyView.swift — a .md window, read as a page.
//
//  This is TermKit's own `MarkdownView` (swift-markdown behind it),
//  dressed in acme's colours and given acme's scrollbar. The window is
//  still a text window: select in it and the view server sends the text
//  body instead, so the same window is read and edited, as acme wants.
//

import TermKit

class MarkdownBodyView: View {
    private let markdown = MarkdownView()
    private var source: String = ""
    private(set) var origin = 0

    override init() {
        super.init()
        colorScheme = ColorScheme(normal: Theme.body, focus: Theme.body, hotNormal: Theme.body, hotFocus: Theme.body)
        markdown.colorScheme = colorScheme
        markdown.headingColor = Theme.attr(Theme.palette.dirty, Theme.palette.bodyBackground, [.bold])
        markdown.emphasisColor = Theme.attr(Theme.palette.text, Theme.palette.bodyBackground, [.bold])
        markdown.linkColor = Theme.attr(Theme.palette.lookHighlight, Theme.palette.bodyBackground, [.underline])
        markdown.codeColor = Theme.attr(Theme.palette.live, Theme.palette.bodyBackground)
        markdown.wrapAround = true
        markdown.canFocus = false
        addSubview(markdown)
    }

    func set(source: String, origin: Int) {
        if source != self.source {
            self.source = source
            markdown.setMarkdown(content: source)
        }
        if origin != self.origin {
            self.origin = origin
            markdown.scrollTo(row: origin)
        }
        setNeedsDisplay()
    }

    func scroll(by lines: Int) {
        if lines > 0 {
            for _ in 0..<lines { markdown.scrollDown() }
        } else {
            for _ in 0..<(-lines) { markdown.scrollUp() }
        }
    }

    override func layoutSubviews() throws {
        // acme's scrollbar takes the first column; the page has the rest
        markdown.frame = Rect(x: 1, y: 0, width: max(0, bounds.width - 1), height: bounds.height)
        try super.layoutSubviews()
    }

    override func drawContent(in region: Rect, painter: Painter) {
        Draw.fill(painter, bounds, Theme.body)
        Draw.scrollbar(painter, x: 0, y: 0, height: bounds.height,
                       origin: origin, shown: bounds.height, total: max(lineCount, 1))
    }

    private var lineCount: Int {
        max(source.split(separator: "\n", omittingEmptySubsequences: false).count, 1)
    }
}
