//
//  main.swift — apex, drawn with TermKit.
//
//  The session lives in a daemon and is reached through `apex-tuid`,
//  which keeps the replica and serves the view model; this process is
//  the UI and nothing else, which is what DESIGN.md asks a client to be.
//
//  apex-tui [--session NAME] [--attach SOCKET] [--via CMD] [--remote DEST] [files...]
//

import Foundation
import TermKit

/// Where `apex-tuid` is: beside this binary, or on the PATH.
func viewServer() -> String {
    let here = URL(fileURLWithPath: CommandLine.arguments[0])
        .deletingLastPathComponent().appendingPathComponent("apex-tuid").path
    if FileManager.default.isExecutableFile(atPath: here) { return here }
    if let env = ProcessInfo.processInfo.environment["APEX_TUID"] { return env }
    for dir in (ProcessInfo.processInfo.environment["PATH"] ?? "").split(separator: ":") {
        let p = (String(dir) as NSString).appendingPathComponent("apex-tuid")
        if FileManager.default.isExecutableFile(atPath: p) { return p }
    }
    return "apex-tuid"
}

let arguments = Array(CommandLine.arguments.dropFirst())
let bridge = Bridge(executable: viewServer(), arguments: arguments)

Application.prepare()

let top = Toplevel.create()
let row = RowView(bridge: bridge)
row.x = Pos.at(0)
row.y = Pos.at(1)              // the menu bar takes the first line
row.width = Dim.fill()
row.height = Dim.fill()

let menu = Menus.bar(bridge,
                     finder: { all in bridge.send(.finder(all: all)) },
                     switcher: { bridge.send(.switcher) })
top.addSubview(menu)
top.addSubview(row)

bridge.onFrame = { frame in
    row.apply(frame)
}
bridge.onEnd = {
    Application.requestStop()
}

do {
    try bridge.start()
} catch {
    FileHandle.standardError.write(Data("apex-tui: cannot start apex-tuid: \(error)\n".utf8))
    Application.shutdown(statusCode: 1)
}

row.listenForMouse()
bridge.send(.resize(cols: Application.terminalSize.width,
                    rows: max(1, Application.terminalSize.height - 2)))

Application.run()
