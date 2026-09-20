//
//  Bridge.swift — the link to apex-tuid.
//
//  apex-tuid holds the replica and the session; this process draws it.
//  One JSON value per line each way, over the child's stdin and stdout,
//  with the frames delivered onto the main queue (TermKit's loop is
//  libdispatch's, so `DispatchQueue.main.async` is all it takes).
//

import Foundation

/// What the UI sends back. The names match `crates/apex-tui/src/input.rs`.
enum UIEvent: Encodable {
    case resize(cols: Int, rows: Int)
    case mouse(x: Int, y: Int, button: MouseButton, motion: Motion, mods: Mods, clicks: Int)
    case text(String)
    case key(NamedKey, Mods)
    case exec(window: UInt64?, text: String)
    case open(name: String)
    case plumb(window: UInt64?, text: String)
    case finder(all: Bool)
    case switcher
    case dismiss
    case switchTo(name: String)
    case clipboard(String)
    case quit

    enum MouseButton: String, Encodable { case b1, b2, b3, wheelup, wheeldown }
    enum Motion: String, Encodable { case down, up, move }
    enum NamedKey: String, Encodable {
        case enter, tab, backspace, delete, escape, up, down, left, right
        case home, end
        case pageUp = "page-up", pageDown = "page-down"
        case eraseWord = "erase-word", eraseLine = "erase-line"
    }

    struct Mods: Encodable {
        var shift = false
        var ctrl = false
        var alt = false
        static let none = Mods()
    }

    private enum CodingKeys: String, CodingKey {
        case t, cols, rows, x, y, button, motion, mods, clicks, text, key, window, name, all
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case let .resize(cols, rows):
            try c.encode("resize", forKey: .t)
            try c.encode(cols, forKey: .cols)
            try c.encode(rows, forKey: .rows)
        case let .mouse(x, y, button, motion, mods, clicks):
            try c.encode("mouse", forKey: .t)
            try c.encode(x, forKey: .x); try c.encode(y, forKey: .y)
            try c.encode(button, forKey: .button)
            try c.encode(motion, forKey: .motion)
            try c.encode(mods, forKey: .mods)
            try c.encode(clicks, forKey: .clicks)
        case let .text(s):
            try c.encode("text", forKey: .t)
            try c.encode(s, forKey: .text)
        case let .key(k, mods):
            try c.encode("key", forKey: .t)
            try c.encode(k, forKey: .key)
            try c.encode(mods, forKey: .mods)
        case let .exec(window, text):
            try c.encode("exec", forKey: .t)
            try c.encodeIfPresent(window, forKey: .window)
            try c.encode(text, forKey: .text)
        case let .open(name):
            try c.encode("open", forKey: .t)
            try c.encode(name, forKey: .name)
        case let .plumb(window, text):
            try c.encode("plumb", forKey: .t)
            try c.encodeIfPresent(window, forKey: .window)
            try c.encode(text, forKey: .text)
        case let .finder(all):
            try c.encode("finder", forKey: .t)
            try c.encode(all, forKey: .all)
        case .switcher:
            try c.encode("switcher", forKey: .t)
        case .dismiss:
            try c.encode("dismiss", forKey: .t)
        case let .switchTo(name):
            try c.encode("switch", forKey: .t)
            try c.encode(name, forKey: .name)
        case let .clipboard(s):
            try c.encode("clipboard", forKey: .t)
            try c.encode(s, forKey: .text)
        case .quit:
            try c.encode("quit", forKey: .t)
        }
    }
}

/// The child process, and the frames it serves.
final class Bridge {
    private let proc = Process()
    private let toChild = Pipe()
    private let fromChild = Pipe()
    private let encoder = JSONEncoder()
    private let decoder = JSONDecoder()
    private let lock = NSLock()

    /// Called on the main queue for every frame.
    var onFrame: ((SessionFrame) -> Void)?
    /// Called on the main queue when the child goes away.
    var onEnd: (() -> Void)?

    init(executable: String, arguments: [String]) {
        proc.executableURL = URL(fileURLWithPath: executable)
        proc.arguments = arguments
        proc.standardInput = toChild
        proc.standardOutput = fromChild
        // the child's diagnostics go to the terminal we are drawing on,
        // so they are kept out of the way in a file instead
        if let log = FileHandle(forWritingAtPath: Bridge.logPath) {
            log.seekToEndOfFile()
            proc.standardError = log
        } else {
            FileManager.default.createFile(atPath: Bridge.logPath, contents: nil)
            proc.standardError = FileHandle(forWritingAtPath: Bridge.logPath) ?? FileHandle.nullDevice
        }
    }

    static var logPath: String {
        let dir = NSTemporaryDirectory()
        return (dir as NSString).appendingPathComponent("apex-tui.log")
    }

    func start() throws {
        try proc.run()
        read()
        proc.terminationHandler = { [weak self] _ in
            DispatchQueue.main.async { self?.onEnd?() }
        }
    }

    func stop() {
        send(.quit)
        proc.terminate()
    }

    func send(_ event: UIEvent) {
        guard let data = try? encoder.encode(event) else { return }
        lock.lock()
        defer { lock.unlock() }
        var line = data
        line.append(0x0a)
        // a full pipe means the child is wedged; dropping the event is
        // better than blocking the draw
        toChild.fileHandleForWriting.write(line)
    }

    /// Read frames off the child's stdout, a line at a time.
    private func read() {
        let handle = fromChild.fileHandleForReading
        DispatchQueue.global(qos: .userInteractive).async { [weak self] in
            var buffer = Data()
            while true {
                let chunk = handle.availableData
                if chunk.isEmpty { break }
                buffer.append(chunk)
                while let nl = buffer.firstIndex(of: 0x0a) {
                    let line = buffer[buffer.startIndex..<nl]
                    buffer = buffer[buffer.index(after: nl)...]
                    guard let self, !line.isEmpty else { continue }
                    guard let frame = try? self.decoder.decode(SessionFrame.self, from: line) else {
                        FileHandle.standardError.write(Data("apex-tui: undecodable frame\n".utf8))
                        continue
                    }
                    DispatchQueue.main.async { self.onFrame?(frame) }
                }
            }
            DispatchQueue.main.async { self?.onEnd?() }
        }
    }
}
