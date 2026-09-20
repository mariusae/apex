//
//  Fuzzy.swift — the finder's ranking.
//
//  A port of `crates/apex-client/src/finder.rs`, so ⌘P puts the same
//  file first here as it does in the gpui client: every query character
//  must appear in order; one scores 1.0 when it starts the file name,
//  0.9 right after a `/`, 0.8 at a word start, 1.0 when it continues the
//  previous match and 0.55 otherwise, halved for a case mismatch, and a
//  match inside the file name is worth a little more. The score is the
//  mean per query character, less a nudge for long paths.
//
//  It runs in the UI because the query changes on every keystroke and
//  the candidate list does not: the view server sends the list once and
//  the typing never leaves this process.
//

import Foundation

enum Fuzzy {
    static func score(query: String, path: String) -> Double? {
        let q = Array(query.lowercased())
        let raw = Array(query)
        let p = Array(path)
        guard !q.isEmpty, q.count <= p.count else { return nil }
        let lower = p.map { Character($0.lowercased()) }
        let nameAt = path.lastIndex(of: "/").map { path.distance(from: path.startIndex, to: $0) + 1 } ?? 0

        // memo over (query index, path index, the previous character matched)
        var memo = [Int: Double?]()
        let stride = p.count + 1

        func best(_ qi: Int, _ pi: Int, _ prevMatched: Bool) -> Double? {
            if qi == q.count { return 0.0 }
            let key = (qi * stride + pi) * 2 + (prevMatched ? 1 : 0)
            if let cached = memo[key] { return cached }
            var out: Double? = nil
            let remaining = q.count - qi
            var j = pi
            while j <= p.count - remaining {
                defer { j += 1 }
                guard lower[j] == q[qi] else { continue }
                var s: Double
                if j == nameAt {
                    s = 1.0
                } else if j > 0 && p[j - 1] == "/" {
                    s = 0.9
                } else if prevMatched && j == pi {
                    s = 1.0
                } else if j > 0 && (isBoundary(p[j - 1]) || (p[j - 1].isLowercase && p[j].isUppercase)) {
                    s = 0.8
                } else {
                    s = 0.55
                }
                if p[j] != raw[qi] && p[j].isLowercase != raw[qi].isLowercase {
                    s *= 0.5
                }
                if j >= nameAt { s += 0.15 }
                if let rest = best(qi + 1, j + 1, true) {
                    let total = s + rest
                    if out == nil || total > out! { out = total }
                }
            }
            memo[key] = out
            return out
        }

        guard let total = best(0, 0, false) else { return nil }
        return total / Double(q.count) - Double(p.count) * 0.0005
    }

    private static func isBoundary(_ c: Character) -> Bool {
        c == "-" || c == "_" || c == "." || c == " " || c.isNumber
    }

    /// The candidates for a query, best first. With nothing typed, the
    /// windows that are open: the files closed lately are there to be
    /// found by name, not to be scrolled through.
    static func rank(_ candidates: [Candidate], query: String) -> [Candidate] {
        let q = query.trimmingCharacters(in: .whitespaces)
        if q.isEmpty {
            return candidates.filter { $0.open }
        }
        let scored: [(Double, Int, Candidate)] = candidates.enumerated().compactMap { i, c in
            guard let s = score(query: q, path: c.name) else { return nil }
            return (s, i, c)
        }
        return scored.sorted { a, b in
            if a.0 != b.0 { return a.0 > b.0 }
            if a.2.open != b.2.open { return a.2.open }
            return a.1 < b.1
        }.map { $0.2 }
    }
}
