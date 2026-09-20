//
//  Model.swift — the view model apex-tuid serves.
//
//  These mirror `crates/apex-tui/src/model.rs` one for one. Nothing here
//  draws: the view model says what is in a window, the views decide how
//  it looks.
//

import Foundation

enum SpanKind: String, Codable {
    case sel, exec, look, tagname, mark
}

struct Span: Codable, Equatable {
    let start: Int
    let len: Int
    let kind: SpanKind
}

struct Line: Codable, Equatable {
    let text: String
    let spans: [Span]

    enum CodingKeys: String, CodingKey { case text, spans }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        text = try c.decode(String.self, forKey: .text)
        spans = try c.decodeIfPresent([Span].self, forKey: .spans) ?? []
    }
}

struct TextModel: Codable, Equatable {
    let view: String
    let lines: [Line]
    let origin: Int
    let total: Int
    let caret: Caret?

    /// `[row, col]` on the wire.
    struct Caret: Codable, Equatable {
        let row: Int
        let col: Int
        init(from decoder: Decoder) throws {
            var c = try decoder.unkeyedContainer()
            row = try c.decode(Int.self)
            col = try c.decode(Int.self)
        }
        func encode(to encoder: Encoder) throws {
            var c = encoder.unkeyedContainer()
            try c.encode(row)
            try c.encode(col)
        }
    }
}

/// A run of terminal cells that is not the terminal's own ink on paper:
/// `[start, len, fg, bg, flags]`.
struct TermRunModel: Codable, Equatable {
    let start: Int
    let len: Int
    let fg: UInt32
    let bg: UInt32
    let flags: UInt8

    init(from decoder: Decoder) throws {
        var c = try decoder.unkeyedContainer()
        start = try c.decode(Int.self)
        len = try c.decode(Int.self)
        fg = try c.decode(UInt32.self)
        bg = try c.decode(UInt32.self)
        flags = try c.decode(UInt8.self)
    }
    func encode(to encoder: Encoder) throws {
        var c = encoder.unkeyedContainer()
        try c.encode(start); try c.encode(len); try c.encode(fg); try c.encode(bg); try c.encode(flags)
    }
}

struct TermRowModel: Codable, Equatable {
    let text: String
    let runs: [TermRunModel]

    enum CodingKeys: String, CodingKey { case text, runs }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        text = try c.decode(String.self, forKey: .text)
        runs = try c.decodeIfPresent([TermRunModel].self, forKey: .runs) ?? []
    }
}

struct Cursor: Codable, Equatable {
    let col: Int
    let row: Int
    init(from decoder: Decoder) throws {
        var c = try decoder.unkeyedContainer()
        col = try c.decode(Int.self)
        row = try c.decode(Int.self)
    }
    func encode(to encoder: Encoder) throws {
        var c = encoder.unkeyedContainer()
        try c.encode(col); try c.encode(row)
    }
}

struct TermModel: Codable, Equatable {
    let term: UInt64
    let cols: Int
    let rows: [TermRowModel]
    let cursor: Cursor?
    let origin: UInt64
    let total: UInt64
    let exited: Bool
    let sel: [Cursor]?
}

enum BodyModel: Equatable {
    case text(TextModel)
    case term(TermModel)
    case markdown(view: String, source: String, origin: Int)
    case web(view: String, url: String, html: String, origin: Int, loading: Bool)
}

extension BodyModel: Codable {
    enum CodingKeys: String, CodingKey { case kind, view, source, origin, url, html, loading }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "text":
            self = .text(try TextModel(from: decoder))
        case "term":
            self = .term(try TermModel(from: decoder))
        case "markdown":
            self = .markdown(view: try c.decode(String.self, forKey: .view),
                             source: try c.decode(String.self, forKey: .source),
                             origin: try c.decode(Int.self, forKey: .origin))
        default:
            self = .web(view: try c.decode(String.self, forKey: .view),
                        url: try c.decode(String.self, forKey: .url),
                        html: try c.decode(String.self, forKey: .html),
                        origin: try c.decode(Int.self, forKey: .origin),
                        loading: try c.decode(Bool.self, forKey: .loading))
        }
    }

    func encode(to encoder: Encoder) throws {
        // the UI never sends a body back; kept so the type is Codable
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .text: try c.encode("text", forKey: .kind)
        case .term: try c.encode("term", forKey: .kind)
        case .markdown: try c.encode("markdown", forKey: .kind)
        case .web: try c.encode("web", forKey: .kind)
        }
    }
}

enum WinKind: String, Codable {
    case file, dir, term, errors, web
}

struct WindowModel: Codable, Equatable {
    let id: UInt64
    let x0: Int, y0: Int, x1: Int, y1: Int
    let tag: TextModel
    let taglines: Int
    /// The body's own rectangle. It is not the window less its tag: the
    /// tiling leaves a border between the two, as acme does.
    let bx0: Int, by0: Int, bx1: Int, by1: Int
    let body: BodyModel
    let kind: WinKind
    let dirty: Bool
    let working: Bool
    let notified: Bool
    let active: Bool

    var rect: (x: Int, y: Int, w: Int, h: Int) { (x0, y0, x1 - x0, y1 - y0) }
    /// The body, in the window's own coordinates.
    var bodyRect: (x: Int, y: Int, w: Int, h: Int) { (bx0 - x0, by0 - y0, bx1 - bx0, by1 - by0) }
}

struct ColumnModel: Codable, Equatable {
    let id: UInt64
    let x0: Int, y0: Int, x1: Int, y1: Int
    let tag: TextModel
    let windows: [WindowModel]
    let strip: Bool
}

struct Candidate: Codable, Equatable {
    let name: String
    let kind: WinKind
    let open: Bool
    let whereIs: String

    enum CodingKeys: String, CodingKey { case name, kind, open, whereIs = "where_" }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = try c.decode(String.self, forKey: .name)
        kind = try c.decode(WinKind.self, forKey: .kind)
        open = try c.decode(Bool.self, forKey: .open)
        whereIs = try c.decodeIfPresent(String.self, forKey: .whereIs) ?? ""
    }
}

enum Overlay: Equatable {
    case finder(candidates: [Candidate], all: Bool)
    case switcher(sessions: [Candidate], current: String)
    case message(title: String, text: String)
}

extension Overlay: Codable {
    enum CodingKeys: String, CodingKey { case kind, candidates, all, sessions, current, title, text }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "finder":
            self = .finder(candidates: try c.decode([Candidate].self, forKey: .candidates),
                           all: try c.decode(Bool.self, forKey: .all))
        case "switcher":
            self = .switcher(sessions: try c.decode([Candidate].self, forKey: .sessions),
                             current: try c.decode(String.self, forKey: .current))
        default:
            self = .message(title: try c.decode(String.self, forKey: .title),
                            text: try c.decode(String.self, forKey: .text))
        }
    }
    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .finder: try c.encode("finder", forKey: .kind)
        case .switcher: try c.encode("switcher", forKey: .kind)
        case .message: try c.encode("message", forKey: .kind)
        }
    }
}

struct Warp: Codable, Equatable {
    let x: Int, y: Int
    init(from decoder: Decoder) throws {
        var c = try decoder.unkeyedContainer()
        x = try c.decode(Int.self); y = try c.decode(Int.self)
    }
    func encode(to encoder: Encoder) throws {
        var c = encoder.unkeyedContainer(); try c.encode(x); try c.encode(y)
    }
}

struct SessionFrame: Codable, Equatable {
    let seq: UInt64
    let cols: Int
    let rows: Int
    let title: String
    let top: TextModel
    let columns: [ColumnModel]
    let overlay: Overlay?
    let notification: String?
    let warp: Warp?
    let snarf: String?
    let connected: Bool
    let fenced: Bool
}
