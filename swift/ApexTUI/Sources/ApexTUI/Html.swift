//
//  Html.swift — HTML to lines of cells.
//
//  A web window in apex is a page the client renders (WEB.md §2); on a
//  terminal that means a text browser. This is a small one: block
//  structure, headings, lists, preformatted text, and links numbered so
//  they can be followed with B3, which is how a page is navigated
//  without a pointer that can hover.
//

import Foundation

struct Page {
    struct Row {
        var text: String
        /// Runs of the row that are a link, with the link's number.
        var links: [(range: Range<Int>, index: Int)] = []
        var heading = false
        var pre = false
    }
    var title: String = ""
    var rows: [Row] = []
    /// The targets, in the order they were numbered from 1.
    var links: [String] = []
}

enum Html {
    /// Render `html` `width` cells across.
    static func render(_ html: String, width: Int, baseURL: String) -> Page {
        var page = Page()
        let tokens = tokenize(html)
        var text = ""            // the paragraph being filled
        var spans: [(Range<Int>, Int)] = []
        var pre = false
        var heading = false
        var skipping = 0         // inside <script> or <style>
        var linkStart: Int? = nil
        var listDepth = 0

        func flush() {
            let trimmed = pre ? text : text.trimmingCharacters(in: .whitespaces)
            if !trimmed.isEmpty || pre {
                for row in wrap(trimmed, width: width, spans: spans, pre: pre) {
                    page.rows.append(Page.Row(text: row.0, links: row.1.map { (range: $0.0, index: $0.1) },
                                              heading: heading, pre: pre))
                }
            }
            text = ""
            spans = []
        }

        func blank() {
            if page.rows.last?.text.isEmpty != true && !page.rows.isEmpty {
                page.rows.append(Page.Row(text: ""))
            }
        }

        for token in tokens {
            switch token {
            case let .text(s):
                if skipping > 0 { continue }
                text += pre ? s : s.replacingOccurrences(of: "\n", with: " ")
            case let .open(name, attrs):
                switch name {
                case "script", "style", "head", "noscript":
                    skipping += 1
                case "br":
                    flush()
                case "p", "div", "section", "article", "header", "footer", "nav", "main", "table", "tr", "blockquote", "form":
                    flush(); blank()
                case "h1", "h2", "h3", "h4", "h5", "h6":
                    flush(); blank(); heading = true
                case "pre":
                    flush(); blank(); pre = true
                case "ul", "ol":
                    flush(); listDepth += 1
                case "li":
                    flush()
                    text = String(repeating: "  ", count: max(0, listDepth - 1)) + "\u{2022} "
                case "a":
                    if let href = attrs["href"], !href.isEmpty {
                        page.links.append(absolute(href, base: baseURL))
                        linkStart = text.count
                    }
                case "title":
                    flush()
                case "td", "th":
                    text += "  "
                default:
                    break
                }
            case let .close(name):
                switch name {
                case "script", "style", "head", "noscript":
                    skipping = max(0, skipping - 1)
                case "p", "div", "section", "article", "header", "footer", "nav", "main", "table", "tr", "blockquote", "form", "li":
                    flush()
                case "h1", "h2", "h3", "h4", "h5", "h6":
                    flush(); heading = false; blank()
                case "pre":
                    flush(); pre = false; blank()
                case "ul", "ol":
                    flush(); listDepth = max(0, listDepth - 1); blank()
                case "a":
                    if let start = linkStart {
                        let n = page.links.count
                        // the number is shown, so B3 can take it
                        text += "[\(n)]"
                        spans.append((start..<text.count, n))
                        linkStart = nil
                    }
                case "title":
                    page.title = text.trimmingCharacters(in: .whitespacesAndNewlines)
                    text = ""
                default:
                    break
                }
            }
        }
        flush()
        // the targets, listed at the foot as a text browser does
        if !page.links.isEmpty {
            page.rows.append(Page.Row(text: ""))
            page.rows.append(Page.Row(text: "Links", heading: true))
            for (i, link) in page.links.enumerated() {
                page.rows.append(Page.Row(text: "[\(i + 1)] \(link)"))
            }
        }
        return page
    }

    /// Wrap a paragraph at the view's width, carrying its link spans.
    private static func wrap(_ text: String, width: Int, spans: [(Range<Int>, Int)], pre: Bool)
        -> [(String, [(Range<Int>, Int)])] {
        guard width > 0 else { return [] }
        if pre {
            return text.split(separator: "\n", omittingEmptySubsequences: false).map { (String($0), []) }
        }
        var out: [(String, [(Range<Int>, Int)])] = []
        var line = ""
        var start = 0        // where `line` begins in `text`
        var at = 0
        for word in text.split(separator: " ", omittingEmptySubsequences: false) {
            let w = String(word)
            if !line.isEmpty && line.count + 1 + w.count > width {
                out.append((line, carry(spans, from: start, length: line.count)))
                start = at
                line = ""
            }
            if !line.isEmpty { line += " " }
            line += w
            at += w.count + 1
        }
        if !line.isEmpty || out.isEmpty {
            out.append((line, carry(spans, from: start, length: line.count)))
        }
        return out
    }

    /// The spans of `[from, from+length)`, moved to the line's own columns.
    private static func carry(_ spans: [(Range<Int>, Int)], from: Int, length: Int) -> [(Range<Int>, Int)] {
        spans.compactMap { span, n in
            let lo = max(span.lowerBound, from) - from
            let hi = min(span.upperBound, from + length) - from
            guard lo < hi, lo >= 0, hi <= length else { return nil }
            return (lo..<hi, n)
        }
    }

    /// Resolve `href` against the page's URL, enough for the common cases.
    static func absolute(_ href: String, base: String) -> String {
        if href.contains("://") || href.hasPrefix("mailto:") || href.hasPrefix("#") { return href }
        guard let baseURL = URL(string: base) else { return href }
        return URL(string: href, relativeTo: baseURL)?.absoluteString ?? href
    }

    // ---- a very small tokenizer ---------------------------------------------

    enum Token {
        case text(String)
        case open(String, [String: String])
        case close(String)
    }

    static func tokenize(_ html: String) -> [Token] {
        var out: [Token] = []
        var text = ""
        var i = html.startIndex
        while i < html.endIndex {
            let ch = html[i]
            if ch == "<" {
                if !text.isEmpty {
                    out.append(.text(entities(text)))
                    text = ""
                }
                guard let close = html[i...].firstIndex(of: ">") else { break }
                let inner = String(html[html.index(after: i)..<close])
                i = html.index(after: close)
                if inner.hasPrefix("!") { continue }     // a comment or a doctype
                if inner.hasPrefix("/") {
                    out.append(.close(name(of: String(inner.dropFirst()))))
                } else {
                    out.append(.open(name(of: inner), attributes(of: inner)))
                    // a void element closes itself
                    if inner.hasSuffix("/") {
                        out.append(.close(name(of: inner)))
                    }
                }
            } else {
                text.append(ch)
                i = html.index(after: i)
            }
        }
        if !text.isEmpty { out.append(.text(entities(text))) }
        return out
    }

    private static func name(of tag: String) -> String {
        String(tag.prefix { !$0.isWhitespace && $0 != "/" && $0 != ">" }).lowercased()
    }

    private static func attributes(of tag: String) -> [String: String] {
        var out: [String: String] = [:]
        var rest = Substring(tag.drop { !$0.isWhitespace })
        while let eq = rest.firstIndex(of: "=") {
            let key = rest[rest.startIndex..<eq].trimmingCharacters(in: .whitespaces).lowercased()
            var value = rest[rest.index(after: eq)...].drop { $0.isWhitespace }
            if let quote = value.first, quote == "\"" || quote == "'" {
                value = value.dropFirst()
                guard let end = value.firstIndex(of: quote) else { break }
                out[key] = entities(String(value[value.startIndex..<end]))
                rest = value[value.index(after: end)...]
            } else {
                let end = value.firstIndex { $0.isWhitespace } ?? value.endIndex
                out[key] = entities(String(value[value.startIndex..<end]))
                rest = value[end...]
            }
        }
        return out
    }

    private static let named: [String: String] = [
        "amp": "&", "lt": "<", "gt": ">", "quot": "\"", "apos": "'", "nbsp": " ",
        "mdash": "\u{2014}", "ndash": "\u{2013}", "hellip": "\u{2026}", "copy": "\u{00A9}",
    ]

    static func entities(_ s: String) -> String {
        guard s.contains("&") else { return s }
        var out = ""
        var i = s.startIndex
        while i < s.endIndex {
            guard s[i] == "&", let end = s[i...].firstIndex(of: ";"),
                  s.distance(from: i, to: end) < 12 else {
                out.append(s[i])
                i = s.index(after: i)
                continue
            }
            let body = String(s[s.index(after: i)..<end])
            if body.hasPrefix("#") {
                let digits = body.dropFirst()
                let value = digits.hasPrefix("x") || digits.hasPrefix("X")
                    ? UInt32(digits.dropFirst(), radix: 16)
                    : UInt32(digits)
                if let value, let scalar = UnicodeScalar(value) {
                    out.append(Character(scalar))
                    i = s.index(after: end)
                    continue
                }
            } else if let named = named[body.lowercased()] {
                out += named
                i = s.index(after: end)
                continue
            }
            out.append(s[i])
            i = s.index(after: i)
        }
        return out
    }
}
