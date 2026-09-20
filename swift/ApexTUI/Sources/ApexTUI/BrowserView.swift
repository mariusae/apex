//
//  BrowserView.swift — a web window.
//
//  The gpui client puts a native web view in the window (WEB.md §2);
//  a terminal cannot, so the page is rendered as text: headings stand
//  out, links are numbered, and the numbers are what B3 follows. The
//  window is still an apex window — its tag holds Back, Fwd and Get, and
//  those go to the session like any other command.
//

import TermKit

class BrowserView: View {
    private(set) var url = ""
    private var html = ""
    private var page = Page()
    private(set) var origin = 0
    var loading = false

    /// Called when a link is followed: B3 on `[n]`, or Enter on a row.
    var onFollow: ((String) -> Void)?

    override init() {
        super.init()
        colorScheme = ColorScheme(normal: Theme.body, focus: Theme.body, hotNormal: Theme.body, hotFocus: Theme.body)
        canFocus = true
    }

    func set(url: String, html: String, origin: Int, loading: Bool) {
        let width = max(1, bounds.width - 1)
        if url != self.url || html != self.html {
            self.url = url
            self.html = html
            page = Html.render(html, width: width, baseURL: url)
        }
        self.origin = min(origin, max(0, page.rows.count - 1))
        self.loading = loading
        setNeedsDisplay()
    }

    /// The page's own title, for the window's tag.
    var title: String { page.title.isEmpty ? url : page.title }

    func scroll(by lines: Int) {
        origin = max(0, min(origin + lines, max(0, page.rows.count - 1)))
        setNeedsDisplay()
    }

    /// The link a click landed on, if any.
    func link(atRow row: Int, column: Int) -> String? {
        let index = origin + row
        guard index >= 0, index < page.rows.count else { return nil }
        let r = page.rows[index]
        for link in r.links where link.range.contains(column) {
            let n = link.index
            return n >= 1 && n <= page.links.count ? page.links[n - 1] : nil
        }
        // a row of the link list at the foot
        if r.text.hasPrefix("["), let close = r.text.firstIndex(of: "]"),
           let n = Int(r.text[r.text.index(after: r.text.startIndex)..<close]),
           n >= 1, n <= page.links.count {
            return page.links[n - 1]
        }
        return nil
    }

    override func layoutSubviews() throws {
        // a resize reflows the page
        page = Html.render(html, width: max(1, bounds.width - 1), baseURL: url)
        try super.layoutSubviews()
    }

    override func drawContent(in region: Rect, painter: Painter) {
        let width = bounds.width
        let height = bounds.height
        guard width > 0, height > 0 else { return }
        Draw.fill(painter, bounds, Theme.body)
        Draw.scrollbar(painter, x: 0, y: 0, height: height,
                       origin: origin, shown: height, total: max(page.rows.count, 1))
        let textWidth = max(0, width - 1)
        guard textWidth > 0 else { return }
        let inner = painter.clipped(to: Rect(x: 1, y: 0, width: textWidth, height: height))
        if loading && page.rows.isEmpty {
            inner.attribute = Theme.panelDim
            inner.goto(col: 0, row: 0)
            inner.add(str: Draw.clip("fetching \(url)\u{2026}", to: textWidth))
            return
        }
        for r in 0..<height {
            let index = origin + r
            guard index < page.rows.count else { break }
            let row = page.rows[index]
            let ground = row.heading
                ? Theme.attr(Theme.palette.dirty, Theme.palette.bodyBackground, [.bold])
                : (row.pre ? Theme.attr(Theme.palette.live, Theme.palette.bodyBackground) : Theme.body)
            inner.attribute = ground
            inner.goto(col: 0, row: r)
            inner.add(str: Draw.clip(row.text, to: textWidth))
            // the links, underlined where they are
            let linkAttr = Theme.attr(Theme.palette.lookHighlight, Theme.palette.bodyBackground, [.underline])
            let chars = Array(row.text)
            for link in row.links {
                let lo = max(0, link.range.lowerBound)
                let hi = min(min(link.range.upperBound, textWidth), chars.count)
                guard lo < hi else { continue }
                inner.attribute = linkAttr
                inner.goto(col: lo, row: r)
                inner.add(str: String(chars[lo..<hi]))
            }
        }
    }
}
