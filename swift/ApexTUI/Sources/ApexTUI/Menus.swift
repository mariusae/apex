//
//  Menus.swift — the menu bar, over acme's commands.
//
//  acme has no menus: everything is text you execute with B2. A
//  terminal has no ⌘ key either, so the menu bar is where the commands
//  that have no obvious tag live — and each entry does exactly what
//  typing the same word in a tag and B2'ing it would do, so nothing here
//  is a second way of working.
//

import TermKit

enum Menus {
    static func bar(_ bridge: Bridge, finder: @escaping (Bool) -> Void, switcher: @escaping () -> Void) -> MenuBar {
        func exec(_ text: String) -> MenuItem {
            MenuItem(title: text, help: "as B2 on \(text) in a tag") {
                bridge.send(.exec(window: nil, text: text))
            }
        }
        return MenuBar(menus: [
            MenuBarItem(title: "_File", children: [
                MenuItem(title: "_Open\u{2026}", help: "the fuzzy finder", shortcut: .controlP) { finder(false) },
                MenuItem(title: "Open in any _session\u{2026}", help: "every tab's windows") { finder(true) },
                exec("New"),
                exec("Put"),
                exec("Get"),
                exec("Putall"),
                nil,
                MenuItem(title: "_Quit", help: "leave the session running", shortcut: .controlQ) {
                    bridge.send(.quit)
                    Application.requestStop()
                },
            ]),
            MenuBarItem(title: "_Edit", children: [
                exec("Undo"),
                exec("Redo"),
                nil,
                exec("Cut"),
                exec("Snarf"),
                exec("Paste"),
                nil,
                exec("Indent on"),
                exec("Indent off"),
            ]),
            MenuBarItem(title: "_Window", children: [
                exec("Newcol"),
                exec("Delcol"),
                exec("Del"),
                exec("Zerox"),
                exec("Sort"),
                nil,
                exec("Dump"),
                exec("Load"),
            ]),
            MenuBarItem(title: "_Session", children: [
                MenuItem(title: "_Sessions\u{2026}") { switcher() },
                exec("Exit"),
            ]),
            MenuBarItem(title: "_Help", children: [
                MenuItem(title: "_Buttons") {
                    MessageBox.info("The three buttons",
                                    message: """
                                        B1 selects, B2 executes what it sweeps, B3 looks it up.
                                        B1 then B2 cuts; B1 then B3 pastes.
                                        The square at a tag's left drags the window; B2 on it
                                        grows it, B3 hides the others.
                                    """)
                },
            ]),
        ])
    }
}
