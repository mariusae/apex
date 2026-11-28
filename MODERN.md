# apex as a modern Mac app (the `modern-mac` branch)

An experiment in the UI, after Manifold (`~/src/manifold`). What makes
apex apex is untouched: acme's tiling and its rectangles (which are
shared state, in the log), tags that are text you type into and B2,
the three buttons and their chords, plumbing, the sessions and the
daemon. Everything here is drawn inside the rectangles the tiling
gives, or beside them, with one deliberate exception: the stash (below),
windows put away out of the tiling, which is the core's
(`Layout::stash`, protocol 37).

## What changed

- **Settings** (⌘,, Apex ▸ Settings…; `settings.rs`): a window of the
  app's, as Manifold's -- the appearance (Match System, Light, Dark),
  the theme, the fonts, the text's size and the switches that were the
  View menu's (a terminal's colours lifted where they don't read on its
  paper, the cursor's blink and its smooth movement, the layout's
  animations). The session's setup, its profile, is ⌘⇧, (Apex ▸
  Profile…). The View menu keeps the text's size (⌘+ ⌘− ⌘0), the
  sidebar, the stash and the overview.
- **Look.** Four palettes, each a light and a dark (Acme's the same in
  both), the appearance choosing which (`theme.rs`; Apex until another
  is chosen): Apex (what was Classic: acme's make -- cream paper, pale
  blue tags, a yellow selection -- toned down, in the hues of go.dev's
  playground), Acme (acme's own colours, for a reader who needs its
  contrast), and Manifold's Terra (linen and walnut, a sage accent,
  sienna and indigo for B2 and B3) and Spectrum (Claude's white pages
  and clay, its blue for B3). Settings shows each as a small apex in its
  colours: windows in a column, a text with a selection and B2's and
  B3's sweeps, a terminal printing in its sixteen, the keys' window
  ringed. Each is a handful of key colours from which the rest follows
  (`make`); every one but Acme keeps orange-for-red and blue-for-green
  where it matters (the terminal's ANSI, apex diff's lines) and B2's and
  B3's sweeps apart under a deuteranopia simulation, as a test holds
  them (Spectrum's own terminal red and green were 26 apart: it has the
  colour-blind sixteen). A palette chosen before and gone since
  (Alabaster, Xcode, GitHub, Nova, rsms) comes back as Apex. A grey
  header over each window, hairlines for acme's black borders.
- **Fonts** (Settings ▸ Fonts, `fonts.rs`; Apex until another is
  chosen), one choice for text windows and tags, mono windows and
  terminals, the sidebar, the palettes and menus, and pages (previews,
  apex diff, as `--apex-font` and `--apex-mono`), with a prose face for
  pages' running text -- a preview's, an agent's conversation
  (`--apex-prose`; what the user wrote to an agent is in the sans, the
  agent's words in the prose) -- Lucida Bright in Apex and Humanist
  (Lucida Grande where it is not installed), the text's own in System: Apex (what was
  Classic: Lucida Grande 13/17, Menlo 12/16), System (SF Pro with its
  high legibility set and tabular figures, 14/20; Terminal's own SF Mono
  at Medium, 12/16, the Regular being thin) and Humanist (what was
  Lucida: Lucida Grande with Lucida Grande Mono W1G, offered only where
  that is installed). ⌘+ and ⌘− (View ▸ Bigger, Smaller, and in
  Settings) take the text a pixel bigger or smaller a step, the mono
  faces in proportion, line heights to whole pixels; ⌘0 back to the
  set's own size. Kept, as the set is.
- **Full screen, apex's own** (`fullscreen.rs`; ⌃⌘F, View ▸ Enter
  Full Screen): the window over the whole of its screen where it is --
  gpui's simple full screen, borderless, in no Space of its own -- the
  menu bar and the Dock hidden until the pointer goes to them, and the
  title bar up in the menu bar's place. The menu bar comes only for the
  pointer at the very top edge, not anywhere in its height, where apex's
  bar is -- macOS's auto-hiding would show it over the session's chevron
  and take the click: the bar and the Dock are hidden outright, handed
  back to macOS at the top edge or the Dock's, and hidden again once the
  pointer is clear of both and no menu is open (`bars_tick`). On a screen with a camera's
  notch the bar is the band beside it (`safeAreaInsets.top`, 32 on a
  14-inch MacBook Pro), split round it, nothing behind the camera: the
  sidebar's card and the session lead the left wing, and the directory,
  the processes and the top row fill it in that order as far as they
  fit, the rest going to the right wing before the stash, each shown as
  richly as the two wings allow (`choose_split`). Kept as the window is: opened there
  again, its frame from before the one to come back to. The green button
  is still the Mac's own full screen, in a Space of its own.
- **Tags as title bars.** The window's name in the primary ink, a
  medium weight; the commands after it in the secondary. Column tags
  and the top row are all secondary. A header's caret is not drawn
  while it only rests at the start.
- **Handles as document dots** (`text_element::dot`): hollow clean,
  filled dirty, gold stale; live rings it in the accent and lights its
  middle; working turns a spinner's arc round it. A notified window's
  header takes a pale tint of the accent, as Mail tints a flagged row.
  pjw's face is only ever for another session wanting the user: on the
  title bar's chevron, by its row in the sidebar and the chevron's list,
  on its card in the overview -- never for this one. Column boxes are
  drag grips; the
  session's square, having nothing to drag, is bare.
  B1 on the top row past its text drags the Mac window, as a title bar
  does.
  All still acme's layout boxes: B1, B2 and B3 on them as ever. A
  window grown to the whole column with others hidden behind it has a
  square handle; alone in its column it stays round.
- **The caret says where the keys go.** Every text's caret is a plain
  dark line, but the one the keys go to (acme's rule: the text under the
  pointer, else the last selected in; none while apex is not in front)
  is the accent blue, a little wider, blinking as iOS's does -- solid for
  half a second after a key, a click or the pointer coming to it, then
  on and off every 530 ms (Settings ▸ Behaviour, the blink off: steady). Its
  smooth movement (Settings ▸ Behaviour; off at first, to live with) has it glide where it
  moves, over 90 ms, as Neovide's and Ghostty's cursor shaders do -- the
  terminal's cursor too, where the keys go -- and go with the text at
  once when the text itself moves (a scroll, a window gliding). A
  header's shows at its start too while it is the one. It is as tall
  as the ink -- the tallest ascender to the deepest descender, a pixel
  over each way -- centred on the line, not the line's height, which a
  tag's air makes taller. The window is drawn again only when the caret
  changes.
  A terminal's cursor is the same caret: the accent's, blinking, where
  the keys go, and the plain dark one where they do not (a hollow box
  once its program has ended).
- **Pointers.** The system's, not Plan 9's: the arrow over text as over
  everything else (a click in apex's text does far more than place an
  insertion point, which is all the I-beam promises), the open hand over
  what drags (a window's handle, a column's box, the session's) and the
  closed hand everywhere while one is held; a page keeps its own.
- **Scrollers.** acme's lane, down a body's left under its handle (#13):
  the handle's dot and the thumb on one axis (`SPINE`), so a window
  opened a little by its box is a few points from its scrollbar, as in
  acme. The lane is room of its own (`LANE`, the text 16 in), in text,
  terminal and page windows alike. What shows in it is a slim rounded
  thumb (`paint_lane`), as long as the part of the text in view is of
  all of it, always there and faint, darker while the text moves, while
  the pointer is in the lane or dragging it, fading a second after
  (`Acme::scroller`). With the pointer in the lane, the track the thumb
  runs in shows too. B1 B2 B3 in the lane are acme's scrollbar.
- **Cards on a ground.** Each window is a card inset in its space, its
  outer corners rounded, on a ground a step below it that shows where
  acme drew black borders and under the column tags and the top row; no
  hairlines, tone parting a tag from its body. A window's tag inks its
  name's directory in the secondary, its last part in the primary, and
  its commands faintly until the pointer is on the tag (the column tags
  and the top row likewise). Selections and sweeps are softly rounded.
  B2 and B3 sweeps (and their pills) are drawn as a chat app draws a
  link: a pale wash of the button's colour -- the blue for B3, the
  action's orange for B2 -- with the text in that colour, deepened to
  read on it, and corners a little rounder than a selection's.
  The window the keys go to has a soft ring in the accent.
- **The title bar is the top row** (`main.rs`, `title_bar`), as a
  modern Mac app's: across the whole window, the window's buttons, the
  sidebar's button, the session (`titlebar.rs`), a divider, then acme's
  top tag, as editable as ever. The session is its name in bold -- a
  click makes it a field, return renames the session on its daemon --
  and a chevron that drops the sessions down (this one checked, a
  notified one wearing pjw, New Session after them; a click goes to
  one); pjw sits on the chevron, a badge, while another session wants
  the user.
  Its bare parts move the window and a double click there zooms
  it; the tiling's line for the top tag lies above acme's area (the
  row's rectangle starts a line up), the tag drawn in the bar instead.
- **Sidebar** (`sidebar.rs`; hidden until View ▸ Show Sidebar), as Manifold's: the
  sessions as vertical tabs in a card inset from the window's edges,
  running up round the window's buttons and its own toggle, one with
  them as Reflect's is (the title bar -- the session's name, the top
  row, the stash -- is the rest's, right of it) -- every known host's, under the host's name
  (this Mac first) when there is more than one: those open in the app
  as their tabs, the others fainter, a click opening one. Each host is
  asked for its sessions in the background as the sidebar shows and
  each minute it stays (what it had last shown until it answers, and
  kept, marked unreachable, if it does not). The shown session lists its windows,
  column by column, each with its dot; a click reveals one and lands on
  it, as taking a notification does. New Session opens the picker.
  Shown or not by the title bar's button (⌃⌘S, View ▸ Show/Hide
  Sidebar), it stands beside the content; hidden, the content has the
  whole window. Nothing brings it out by itself (the pointer at the
  window's edge once did, over the content, on Liquid Glass): the button
  is always there. There is no tab strip.
- **Palettes.** The picker (⌘T) and the finder are Manifold's command
  palette: a card 560 wide centred across the window, its top 30% of
  the way down, rounded 12 with a hairline and a soft shadow, over a
  light scrim; a 48-point search row with a magnifying glass, rows 34
  high with the title at 13.5 and what follows in the secondary ink. The
  chosen row, in every list, wears the colours of what choosing it does,
  as the B4 menu's does (`shell::chosen`): B2's faint wash, ink and a bar
  at its left where it runs a command (⌘⇧P), B3's where it goes to a
  place or opens a thing (⌘P, ⌘O, the picker, the folder and path
  pickers, ^F's list, the session menu, the outline by a scrollbar);
  in a list with no keys, the row under the pointer. Where a list
  already is (the session shown, ⌘P's tab) is a ✓ in the quiet ink;
  the heading being read, in the outline by a scrollbar, has the chosen
  row's bar at its left, and the pill naming it there is a popover
  row's height at 13, a target the pointer finds. What is anchored to a thing -- the B4 menu, the
  session menu, the pickers, ^F's list, the outline, the cards -- is a
  popover, one chrome for all (`shell::popover`): rounded 9, the
  palette's half-pixel hairline, one nearer shadow, rows 26 high at the
  palette's 13.5 (the path picker's and ^F's in the face of the text
  they list, ^F's a line of it high); a foot, where a list has one, is
  under a hairline with its keys at the right; a list emptied by a
  query says "Nothing matches". Every overlay fades in over 90 ms, as
  the B4 menu always has.
- **Maximizing** (`tiling::colfull`, `colmaximize` and their kin, the
  core's). B3 on a window's box is acme's again: the window grown to
  the whole column, keeping its place in it, the others hidden behind
  it (`Column::full`); B3 again or B1 gives them back exactly where
  they were. Its handle is square while others are hidden. B2
  maximizes as acme's B2 does: the others in the column down to their
  tags, which show, so it needs no mark; B1 on the maximized window's
  box gives each back the size it had (`Slot::premax`). Adding a
  window to the column, closing one, dragging or growing gives the
  hidden ones back first; going to a hidden window does too.
- **The stash** (`shelf.rs`, `tiling::stash`): one for the session, not
  a column's. ⌘M (View ▸ Stash Window) puts the window the keys go to
  away -- out of its column, its space going to a neighbour as a closed
  window's does; `Stash` typed and run in a tag does the same (it is
  not in the tags as drawn). The stashed windows show at the title
  bar's right end as their tags made small, bunched like a hand of
  cards, the latest on top. The pointer on them, or a scroll over
  them, fans them out, and the one under the pointer (or scrolled to)
  shows below the bar: the window itself, live, at the size it had
  (at least 480 wide and 320 or two fifths of the window tall: one put
  away as it was made, an errors window, may have had a line or two;
  such a window comes back with an even share of its column, too).
  The pointer can go onto it and work in it as in any window --
  select, snarf, B2, B3, type, scroll -- and it stays stashed (a Look in
  it is found and shown there, the mouse going to it there); the fan
  closes a quarter second after the pointer has left the cards and the
  preview, not while a button is held. A window worked in there (a
  click or a key in it, or a toast's Show All) is brought forward as
  the fan closes -- rightmost, the first card met next time -- not while
  it is open, where its card moving would take the preview away. A click on a card, or B1 on the
  handle in the preview, brings the window back where it was -- under the window it was under, at the share of its
  column it had, or at the foot of the active column if its own is
  gone. The sidebar lists them under Stashed, and a click there does
  the same. That is the only way back but one: whatever goes to a
  stashed window (a Look, the plumber, the finder, a notification)
  brings it back too. The finder and `apex win list` list stashed
  windows where they stood.
- **ctrl-tab** (`switcher.rs`) walks the sessions live: each press
  shows the next one in the window at once, sliding in from the right
  over the one it replaces (which slides out to the left, drawn from its
  replica for the moment it takes; ctrl-shift-tab the other way), and
  the walk goes on while control is held; letting go leaves the window
  where it came to, and escape goes back to where the walk began. The
  presses walk the sessions most recently settled on first; the ones
  passed on the way are not taken as settled on. A session not
  connected yet is being connected as it is passed.
- **⌘1 to ⌘9 and ⌘\` go to sessions** (`sessionkeys.rs`), as Manifold's
  spaces: ⌘1 to ⌘8 in the sidebar's order, ⌘9 the last. ⌘ held alone a
  quarter second shows each one's key, a pill (`⌘2`) by its row in the
  sidebar; with the sidebar hidden, the sessions' names slide out
  across the title bar from the session's own, each with its key (a
  click goes there). Another modifier, or letting go, puts them away; a
  key with ⌘ (⌘2, then ⌘3) leaves them. ⌘\` (⇧⌘\` back) is Manifold's
  ⌥⇥, as ⌘⇥ the Mac's apps: the sessions, the most recently settled on
  first, walked while ⌘ is held -- a tap goes back to the session
  before -- their cards (each session's window, small and live) coming
  up after a fifth of a second, the one chosen ringed; letting go of ⌘
  goes to it, escape stays, a card clicked goes to it. Nothing moves
  until ⌘ is let go, where ctrl-tab shows each session as it passes.
- **⌘⇧\\ (or ⌘') shows every session** (View ▸ Show All Sessions), as Mission
  Control shows the windows: a grid of cards over the window, as large
  as the window allows, each its session's window drawn small and live
  from its replica (a notified one wearing pjw), in the sidebar's order,
  on an opaque ground. The window shrinks into its card as the grid
  comes up. One ring, the accent's, is on the card chosen -- this
  session's at first -- and is at once on the one under the pointer or
  the arrows' next. A click or return goes to it, its card growing to fill
  the window; escape, a click off the cards or ⌘⇧\\ again goes back to
  this one the same way.
- **Web windows** (`webbar.rs`) have a header of their own in the tag's
  place, as the Claude app's browser does: the handle (every button and
  drag of it as any window's -- it moves, grows, maximizes), back and
  forward, and the address, and nothing else. A click in the address
  takes it for typing (all selected); return goes there (a bare host
  gets https://, a path is the host's file), escape or a click elsewhere
  leaves it. The tag is still the core's underneath -- its first word
  the address the page follows -- only not drawn. `Web` with nothing
  given or selected makes a blank page, its address field ready to type
  in (the core allows a web window with no address for it).
- **Letting a session go.** The pointer on a session's row in the
  sidebar shows an × (in place of the pjw a notified one wears), as the
  tabs had: the tab goes, the session stays on its host.
- **Column edges.** The line between two columns takes the ↔ pointer, and
  a drag of it moves only the line (`tiling::rowmovecol`, what the
  column box's resize does): the columns either side wider and narrower,
  never shuffled, and a click on it does nothing.
- **Placement preview.** While a handle is held -- a window's, a
  column's, or the line between columns -- where it
  would land were it let go now is shaded in the accent, as Manifold
  shows where a dragged sheet would go. It is the drop itself, done on a
  copy of the layout (`Node::drag_window_preview` and its kin), so what
  is shown is what happens.
- **Programs resumed through a server restart** (`apex_server::resume`,
  the Program Resume Protocol, OSC 7577, as Rex has it): each terminal's
  shell gets a token of its own (`OSC7577_TOKEN`), and a program in it
  that says the command to bring it back -- signed with the token, so
  output from a file or a remote host cannot -- is kept with the
  terminal in the session's dump (the user's alone), when the session's
  `Resume.programs` names it (claude and codex unless set; off keeps
  none). A genuine report of another, or of a process not the user's or
  not on that terminal, clears it; its process gone when the shell
  prompts again clears it, and so does the program saying so. When the
  daemon starts the session again, at the restored shell's first prompt
  (or two seconds on, a shell that marks none) `apex resume TICKET` is
  typed there: the ticket, the user's alone, holds the command and its
  directory, and `apex resume` goes there, says what it resumes, takes
  the record as its own, and becomes the program, its arguments never a
  shell's to read. apex's hooks report Claude Code and Codex at their
  session's start and as prompts are sent -- `claude [its options]
  --resume ID`, `codex resume [its options] ID`, once the agent has
  written the session down (at its first prompt; till then the agent
  started afresh, its options as they were) -- a prompt, a resume or a worktree it was started
  with left off, print mode and `codex exec` not reported -- and clear
  when the user ends the session, not when its terminal goes.
- **Restarting the server** (`restart.rs`), as Manifold offers: an
  attach that finds this machine's server of another version (which
  this Apex cannot talk to) offers, once a launch, to restart it, and
  Apex ▸ Restart Server does whenever asked: a submenu of this Mac and
  each host attached to this launch, each saying its daemon's protocol
  and build and whether that is this app's (or that it could not be
  reached), the host's restarted by putting this apex there and
  stopping its daemon, every tab on it attaching again. Both say first what goes
  with it: a server that keeps its sessions (DESIGN.md §12) has them
  back, windows and unsaved changes, the programs in its terminals
  ending; one from before that ends them (changes not saved counted,
  when it can); and the tabs stay, each attaching to its session again
  or to a fresh one of its name. A server of this
  protocol is asked to stop; one of another, which may not read our
  Stop as a Stop, is signalled, once the process at its socket's far
  end is seen to be an apex server on that socket
  (`remote::stop_any`). Nothing starts a second server over an old one
  any more: that took its socket and left it running, unreachable,
  with its sessions (`ensure_daemon`).
- **Force click is B3.** A trackpad pressed hard (macOS's "look up")
  turns the B1 press it began as into a B3 press where it is, before
  anything is swept: its release looks, or opens what the plumber finds.
- **Pills.** ⌘ held puts what a ⌘-click (B3) would take under the
  pointer -- the selection when the pointer is in it, else the word it
  would look for or open -- on a pill in B3's sweep colour, its text in
  the sweep's ink, the pointer a hand; ⌥ held, what a ⌥-click (B2) would
  run, on a pill in B2's. In tags as in bodies, and only with the
  modifier held.
- **Tag lines** are six pixels taller than a body's (`TAG_PAD`): the
  tiling's font height is theirs. The pad is the tag's, once, over its
  first row and under its last, not between them: a tag of `n` lines is
  a tag line and `n - 1` rows (`tiling::tag_height`, the client's
  `Info::tag_row` a body's line, `tag_row_height`), its rows a body's
  line apart, and a click in the pad is its nearest row's. What is drawn a
  tag's line high -- the handle's box, a selection, a sweep, a pill --
  still is, reaching into the pad over the first row and under the
  last, so a one-row tag is drawn as it always was. Text is centred by its ink, not by
  the face's ascent and descent (`ink_lift`: a face keeps room over its
  ascenders for accents, so text centred the usual way sits low), so
  there is as much air over the ascenders as under the descenders, in a
  pill as in a line; a folded window's card, a little shorter than the
  line, has its line and handle centred in it.
- **⌘⇧P** (`commands.rs`) is a palette of commands to run in the window
  under the pointer: the words in its tag, its tools (the B4 menu's),
  what was run lately anywhere in the session, and apex's own, fuzzy-
  matched; return runs one there as B2 would, or what was typed. Go to
  in All Tabs moves to ⌘⇧O.
- **Windows glide** (`glide.rs`): what the tiling moves -- a grow, a
  stash or a recall, a drag, a close -- is drawn part of the way from
  where it was to where it is, for a sixth of a second; one that appears
  opens down from its top. Resizing the OS window or switching session
  snaps. A gliding window's terminal keeps its size, and its text its
  scroll, until it lands.
- **Empty columns** say what to do there, faintly, in the middle of
  them (where there is room): ⌘P to go to a file, ⌘N for a new window,
  ⌥⌘N for a terminal (Newterm), ⇧⌘N for win, ⌘⇧P for every command. Drawing only: the
  column's ground takes the buttons as ever.
- **Errors as toasts** (`toasts.rs`): a command's errors still go to its
  errors window, but that window is made in the stash (as every
  diagnostic window is: a language server's, a script's
  `apex new -diagnostic`) and what is new in it shows in a toast at the
  app's lower right, with Show All (the
  window shown in the stash's preview and left stashed, the pointer on
  the toast's first line there, selected; B1 anywhere on the toast
  does the same) and ×; it goes after eight
  seconds unless the pointer is on it, and at once on a click anywhere
  off the toasts (in a page too). An errors window brought back is
  written to as before.
- **Session previews**: the pointer on a session in the sidebar (not the
  one shown) brings a sketch of its screen up beside the row, live
  (`sketch.rs`): not its text drawn small, which is too fine to read and
  says less for it, but its windows where the tiling has them, as
  Manifold's space switcher draws a space -- each a tile with its
  handle's dot, what kind of window it is (a file, a directory, a
  terminal, a page, an agent's conversation, errors) and its name, its
  body plain with that kind's icon large in it; the window last worked
  in ringed in the accent, and the stash's count under it.
- **Prompt marks** (OSC 133): with `eval "$(apex shell-integration zsh)"`
  (or `bash`) in the shell's startup file, the shell marks each
  command's prompt, output and end, and the terminal carries the marks
  (`TermOp::Marks`, protocol 33): ⌘↑ ⌘↓ go from prompt to prompt, a
  command that failed has a mark in the gutter by its prompt, in the
  terminal's red, and ⌘⇧C copies the last command's output.
- **^F completes inline** (`completion.rs`): the path fragment before
  the caret goes to the server (`Candidates`, protocol 34, answered to
  the asker alone), what its candidates share is typed in at once, and
  when more than one is left they are listed under the caret (over its
  line, when the window's foot leaves no room below). Typing
  goes on into the text and narrows the list; ↑ ↓ choose; return or tab
  takes the one chosen; a slash typed, escape or a click elsewhere puts
  the list away. One candidate is simply typed in, a directory with its
  slash, a file with a space after, as acme's ^F does. A completion done
  is done: the next list (a directory's names, say) comes only with the
  next ^F. No more lists in the errors window.
- **A page's outline in its scrollbar** (`outline.rs`; the page says
  it, `web.rs`'s `TOC_SCRIPT`): a page's scrollbar is a text's, in its
  lane down the page's left, B1 B2 B3 acme's, and on a page rendered
  from a buffer (a Markdown preview, apex diff) carries the page's
  headings as stops down it: dots on the thumb's axis, each where it
  stands in the page and bigger the higher it is, dark over the thumb as
  over the lane, the heading of the part being read in the accent (a
  scrollspy). The bar is for scrolling first: the pointer in it brings
  out only a pill by the accent's stop naming the part being read; the
  pointer onto the pill opens it
  into the headings as a list, over the page, the one being read in the
  accent and level with where the pill was, and a click on one goes
  there (a diff's files by their names, cut from the left when too long,
  since a path's end says most). Back in the bar, it is a pill again.
  Only with two headings or more; nothing of it is drawn in the page.
  The stops show while the pointer is in the bar, and the pill names
  the item whose stop is nearest the pointer (the list opening level
  with it), not the one being read.
  A page may say what its outline is instead: its elements marked
  `data-apex-outline="LEVEL"`, titled by `data-apex-title` or their
  text, are its entries and nothing else (an agent's conversation, a
  turn each, the headings of its answers not among them).
- **A tag is what the window is, then your words.** The head is drawn
  from the window's state (DESIGN.md, *Windows are what they are*), none
  of it in the tag's text: the path, its folders dim and its name
  strong (Untitled for a new window), a wider space after it; the label
  on a chip (a terminal's title, a tool's name for its pane; an errors
  window's or a preview's kind when it has none); apex's verbs as
  icons, faint until the pointer is on the tag; a hairline. Then the
  user's words, a step stronger: all the tag's text holds.
- **The session's place in the title bar**: the host, dim, and the
  session's directory as crumbs; a crumb clicked lists the folders in it
  to cd into (`./` for the folder itself). Paths inside the directory
  are drawn in the tags from there on, unmarked (`src/main.rs`), and
  every other path whole; the directory itself is a drawn `./`.
- **Layout Animations** (Settings ▸ Behaviour, off by default): windows and columns
  gliding to their places, a window opening down from its top, the
  pointer riding with the window it goes to. Off, the layout is where it
  goes at once; the overview, the stash's fan, a session sliding in, the
  scrolling and the status marks keep theirs.
- **Splitting a column by dragging**, as VS Code's and Zed's editors
  split: a window's box let go near a column's left or right edge (its
  outer eighth) makes a new column there, half the column wide, the
  window in it; the drag's shadow shows the window filling it. The
  window's own column too: at its left edge with the pointer pushed onto
  the edge itself (so a drag up or down drifting left still just moves
  it), at its right after a clear move right; never for a column's only
  window, nor a column too narrow to halve.
- **A column a drag empties stays**, as acme's does, whatever emptied
  it (a drag, Del, the stash): Delcol is what closes one (#4).
- **⌘O: open a file or folder in the session's directory.** The host
  lists everything under it and matches what is typed there, so a huge
  tree never crosses the wire: the walk (breadth first, leaving out
  dot-directories, node_modules and their like, stopping at a million)
  streams into an index; the matcher (Zed's fuzzy scoring, across the
  cores) sends only the best page, again as the walk goes on; a newer
  query abandons an older one, a longer one narrows the last one's
  matches; scrolling near the end asks for the next page; closing it
  stops both on the host. The list fills in from the first moment.
- **Processes as pills.** What runs for the session is shown before
  the top row's text, a pill each name: its name, how many when there
  are several (`Win 5`), and a × on a single one. The × ends it; B1 on
  it goes to its output, B3 to the window it was run from. The sidebar
  lists them too, under the session's windows.
- **The title bar fits its room** (`titlefit.rs`). The directory and the
  processes each have a few ways to show, and the bar takes the richest
  that leaves the top row its room -- all its text while the caret is
  in it. The directory gives way first: the host and the last two
  crumbs after `…/`, then the last crumb alone. Then the processes: the
  pills stacked as cards, the newest on top with the count of all; then
  the count alone. What is short fans out under the pointer, over the
  top row rather than pushing it along: the whole path, or every
  process on a pill of its own with its ×. It folds when the pointer
  leaves.
- **Where a window stands with its session** (`standing.rs`). It leads
  (what is typed takes), watches (another client leads), or has lost
  its link: stalled (the daemon has stopped answering) or offline (the
  link closed). Anything but leading is a chip after the session's name
  -- an eye in blue for watching, a broken link in gold for a lost one,
  and the word -- and a banner under the title bar saying what it means.
  Watching names the client that leads (UIs attach as `apex on HOST`)
  and offers Take over, never taken for the user; typing meanwhile
  flashes the banner, and the carets are hollow. A lost link is
  attached again by itself, after 1, 2, 5, 10, 20, then every 30
  seconds, and at once after the Mac wakes; a stalled one with edits
  the daemon has not yet acknowledged is waited for instead, the
  banner naming their windows, and Reconnect asks before dropping them.
  The chip's card says how the daemon answers and offers Take over,
  Reconnect, and Restart daemon for one of another build; the page of a
  tab with no link counts down to its next try. The tabs, the sidebar
  and the session menu say the same words.
- **apex's verbs as icons** (`VERB_ICONS`, `Node::window_verbs`): `Del`
  (×), `Snarf` (copy), `Undo` and `Redo` (curved arrows), `Put` (into
  a tray), `Get` (reload), `Send` (a paper plane), `Back` and `Fwd`
  (chevrons). B1 or B2 on one runs it. After a window closes, the
  pointer goes to the next window's `Del`, as acme's does.
- **The path is a breadcrumb.** One click on a folder or the name
  brings a picker down under it, as VS Code's do: that folder's entries
  (the name's siblings), listed by the host as ^F's names are, so a
  remote session's too, narrowed as a query is typed -- in the tag
  itself: while the picker is down the path shows the folder listed and
  then what is typed, with its caret, where the rest of the path was;
  the names below are in the tag's face, under it. First come the windows open on the
  folder or a file in it that are not a plain file's or folder's -- its
  errors, a file's preview, a terminal or a tool's pane there -- errors
  first, then previews: the quick way to them. Return (or a
  click) opens the one chosen, a file or a folder, in a window of its
  own; ⌥return opens it here, in place of what this window shows -- a
  file in a file's window, a folder in a folder's (asking again when it
  is unsaved). → or tab (or the › at a folder's row's end) goes into a
  folder; ← or backspace with nothing typed goes up. A double-click on the
  path makes it a field (one click on Untitled): return renames the
  window, a relative path in the folder it was in. A double-click on
  the label edits it. B3 on a folder or the name plumbs the path to
  there.
- **Columns answer as windows do.** B1 on a column's box grows it a
  little. B2 maximizes it: the others minimized where they stand, as a
  maximized window leaves the others their tags, each remembering its
  width; B1 on the maximized column's box brings them all back. A column
  is minimized too when a drag on the line beside it takes it past half
  the least a column may be, as a window dragged over goes down to its
  tag, or when a neighbour's B1 squeezes it. A minimized column is a
  slim card on its side where it stands, among the others in their
  order, its outline rounded as a folded window's tag is, the column's
  grip at its top (it is the column's box, and says it is a column),
  each window's handle down it where the window stands: a click on a handle brings the
  column back and lands on that window, and B1 anywhere else on it
  brings it back where it stands.
  B3 gives a column the whole row, as it grows a window to the whole
  column: the others hidden behind it (`Layout::full`), their places
  kept, its grip framed and its dots square while they are; B3 again or
  B1 on its box gives them back where they stood, it at the width it
  had (its share of the row, `Column::restore`). Anything that changes
  the row -- a column added, closed or dragged, B2, going to a window in
  a hidden one -- gives them back first. Columns are not put away. A column an older apex put away at
  the row's right is still drawn as the edges of sheets on their sides,
  and B1 brings it back where it stood.
- **Long paths shortened** (`text_element::shortened`, `Head::elided`):
  a window's path that takes more than half its tag and is what makes the
  tag wrap is drawn from a suffix instead -- the fewest of its leading
  folders put away for the tag to fit one line, its name always kept --
  behind `⋯/`, which says so. `⋯/` is a folder like the others: the last
  one it puts away, which B1 lists and B3 plumbs. A tag of more lines
  than one (a newline in it) is long anyway and keeps its path whole.
  One that would not fit on a line however short the path (a long name,
  a verb's icon more) has it as short as it goes, the name alone after
  `⋯/` -- never whole again for want of a few pixels.
- **Looking as you type** (`look.rs`): the query is the first `Look`'s
  argument in a window's tag (every tag begins with `Look `), the result
  the window's selection, as acme's Look has them. Written `Look/word`
  it is looked for as it is typed; `Look word` is acme's, and a word
  typed after an empty `Look ` is only a word (a command, say). A
  closing slash lets it have spaces, `Look/two words/`: the first slash
  followed by white space or the tag's end, so a path later in the tag
  is not taken for it. The slashes are in the text, so whatever a
  client does it shows; run (B2 takes a closed one whole), `Look/word`
  is `Look word`. While the caret is in a `Look/` argument, each
  change looks again from where the selection was
  when the typing began: a letter more narrows there or further on, a
  letter less goes back, empty puts the selection back; nothing found,
  it stays and the argument is struck through. A click ends it, and so
  does escape, which leaves a caret at the end of what was found with
  the pointer on it, to edit there. ⌘F takes
  the caret and the pointer to the argument, making it `Look/…/` (typing
  `Look// ` first where a tag has none), selected to type over; ⌘G and ⌘⇧G look again forwards
  and back, the pointer onto what they find as B3's is (⌘J is the next
  notification now). B3 on a word, found in its
  window, puts the word in the argument too, as `Look/word/`. While a look goes on in a
  window -- live, or its selection one of the places -- every place the
  word is is washed faintly in B3's blue, under the selection; once the
  selection is elsewhere, the marks are gone. A tag's later `Look`s are
  words like any other.
- **A lens on a window** (`state::Lens`, DESIGN.md "Lenses"): a
  window standing in for another, its base, in the base's place, made
  and removed and nothing else. `apex editor` run in a window -- a
  terminal, win's, a command from a tag: `$winid` -- ($EDITOR for git,
  say) opens the file as a lens on that window, rather than somewhere
  else: a window of its own, on the file's text if it is open elsewhere
  too (as Zerox makes one). The terminal is under it, out of the tiling
  and still running, and comes back in the place when the lens goes --
  wherever it has been moved or stashed meanwhile -- or when editor
  does: a lens goes with the attachment that made it, and with its
  base. Its Del is drawn as what is under it -- a terminal's prompt, a
  page -- with a return arrow (it takes the
  lens away only). Going to a window under a lens -- Look, the
  plumber, the finder, a notification taken -- shows the place as it
  is, the lens on top, and the place's handle says the most pressing
  status and any notification in it. A lens on a window with one goes
  on the top; closing one closes the lenses over it.
- **⌘↩ is View**: the word `View`, run in the window under the
  pointer as B2 there would, its meaning its tools' -- apex has none of
  its own, and where no tool claims it nothing happens. Preview claims
  it on the files it converts (the session's default rule starts it):
  the file's preview a lens on its window, scrolled to where the window
  was showing, kept live as the text changes; ⌘↩ on the page takes the
  lens away, the window put at the line the page had at its top (the
  client tells a page's owner that line as it scrolls,
  `WindowEvent::Scrolled`; `Proposal::Origin` puts a window there).
- **An input strip** (`Window::input`): a window's second text, a
  tool's, afloat at its body's foot in a box of its own -- rounded,
  bordered, no shadow, in from the sides -- so it reads as a
  place to write, a round send button at its right (an arrow up, the
  accent's while there is something to send: what ⌘↩ in it does); as
  tall as its text, one line to six. Over a text it floats, the text's
  last line able to go to the window's top as ever; a terminal, which
  cannot scroll past its last row, and a page, a native view apex draws
  nothing over, stop above it, the box in a band of its own at the
  foot. (Over a page it once floated too, through a rounded hole cut in
  the page and a shadow the page drew in CSS: two layers to keep in
  step, and when they were not the box vanished.) Apex text, replicated, a draft in every UI. Enter
  in it, as in a chat's box, and ⌘↩ are Send, the word its tool claims
  there to take the text (`take_input`); shift-Enter is a new line.
  The window's body is then read-only (`Node::read_only`): its tool's
  to write, the user's to select in and copy from, every edit of the
  user's refused in the core -- typing, Cut, Paste, Undo, an `Edit`
  that changes it. So the keys typed with the pointer anywhere on the
  body are the strip's, its caret shown there, but for those that move
  through the body or copy from it (↑ ↓, page up and down, home, end,
  ⌘↑ ⌘↓, ⌘C, ⌘A); the tag keeps its keys. A page with a strip (an
  agent's conversation) is never given the keyboard (`Webs::keyless`):
  the keys over it are the strip's, its scroll keys and ⌘C ⌘A passed
  to it; a click in it gives WebKit the keys for the selection, taken
  back once the button is up, the selection kept painted.
- **An agent's conversation is its terminal's lens**: ⌘↩ in a terminal
  running an agent (`apex tool agent`) shows what was asked and
  answered in its place, kept up as it goes, a prompt written in the
  strip under it and sent with ⌘↩, typed into the terminal; Stop in its
  tag interrupts; ⌘↩ on the conversation gives the terminal back, an
  unsent draft kept for the next time, and where it was scrolled: at
  its end the first time, following what comes, and back where it was
  left after (only the user's own scrolling takes a page off its end;
  WebKit holding what was in view in place as the page is written anew
  does not).
- **A terminal's commands are its outline**: each command its shell
  marked (OSC 133) a stop down its scrollbar at its prompt, listed by
  what ran -- as the shell says it (`C`'s `cmdline_url`, fish's and
  ghostty's way, and now apex's zsh and bash integrations'), else as
  its prompt's line reads, the prompt left off -- with when it began
  and how it went, as Manifold says it: "18m ago › took 2m 52s",
  "4:05pm › failed after 5.0s (exit 2)", "just now › running"
  (`PromptMark::cmd`, `started`, `ended`, the server's clock). B1 on
  one scrolls the terminal to it.
- **A tool's outline of a text** (`Buffer::outline`): items a tool
  gives a window's text, each at a place that moves with the edits as
  dot does, gone with the tool; drawn as a page's headings are, stops
  down the scrollbar and the list the pointer brings out there, B1 on
  one putting its line at the top. The lsp tool gives each file it
  serves its symbols (`textDocument/documentSymbol`, asked again as
  edits settle); an agent's conversation has its turns as headings.
- **Win's commands are its outline** (`apex tool win`): rc's `rcmain`
  (mariusae/rustrc), in apex, marks each prompt and command as zsh's
  and bash's integrations do (OSC 133, through its `fn prompt` and
  `fn preexec` hooks); win takes the marks out of the text and gives
  its window an outline of them -- each command at its prompt, titled
  as typed (a block of lines one), with when it ran, how long it took
  and its status (`OutlineItem::started`, `ended`, `exit`, protocol
  67), said as a terminal's: "18m ago › failed after 2.0s (exit 2)". So
  the scrollbar's stops, its pill and list, ⌘P's sections and the
  Outline lens have win's commands as they have a terminal's. A
  command's `cmdline=` is the rest of its mark, semicolons and all.
- **A command is a thing to act on** (`runs.rs`, `outline::run_of`):
  a shell's commands, a terminal's or win's. A row in a list of them is
  a place, as any outline's row is: B1 or B3 goes there; and B2 on a
  row of the outline by the scrollbar is Send, as B2 is everywhere --
  the command run again (typed to a terminal's shell; win's sent as its
  Send does), or on an agent's turn, what was asked sent to it again
  (`Send TEXT` in its lens, the page marking each prompt
  `data-apex-send`, the strip's draft left). It takes on B2's colours
  while down, goes on letting go on the same row (off it, or B3 with
  it, it does not), blinks as B2's run text does, and the outline goes,
  the window at its end. B1 held on the row and B2 copies the command
  and its output, as on its dot. A row with nothing to send (a
  heading, a symbol) takes no B2. The command
  itself is its dot -- at the end of its prompt's row while the pointer
  is on that row, and always at the end of its row in the outline's
  list by the scrollbar, in ⌘P's sections and at the end of its line in
  the Outline lens while the pointer is on it (where B1 on it goes there
  too, as choosing does, the lens or ⌘P going) -- quiet, blue while it runs, orange when it
  failed: B1 there selects the command and its output, B2 after it (B1
  still held) copies them, B2 runs it again (typed to its shell), B3
  opens its output in a window of its own (`dir/+make`, the command its
  label). A double-click on a prompt (on its characters, or at the
  start of its line) selects the command and its output as B1 on its
  dot does, as one by a bracket selects to its match; a selection
  copies as it reads, its prompt and all. ⌘C with the pointer on a dot
  or a command's row in the list, ⌘C in ⌘P with a command's section
  chosen, or `Snarfcmd` (B4, in a terminal or a
  win window: the command the pointer is in, else the last), copies it
  and its output: `$ ` and the command, then
  what it printed, whatever the prompt was. A terminal's output is read
  back from its history on the host; win's is the text between where
  its output began and ended (`OutlineItem::output`, kept with the
  edits; protocol 68).
- **Outline, a lens to filter and go to** (`apex tool outline`): in
  every window's tools menu; the window's outline (a file's symbols, a
  terminal's commands, a page's headings or an agent's turns) as text
  in its place, an entry a line. B1 or B3 on a line goes there; typing
  in its strip filters, ↩ going to the first match; Outline or ⌘↩ gives
  the window back. A tool like any other: `Tool::outline` reads any
  window's, `Tool::reveal` goes to an entry.
- **Files dropped from the Finder** (`drops.rs`; a screenshot's
  thumbnail is one): their paths where they land. On a terminal, as
  Terminal.app types them -- each escaped for a shell with backslashes,
  a space between and after, pasted -- so an agent there takes a path to
  an image for the image (Claude Code's `[Image #1]`); on a text at the
  place under the pointer, selected as a paste is, escaped the same where
  the text goes to a shell (a live window, an input strip), a line each
  anywhere else.
  On a session on another host, the files go there first, each to
  `~/.apex/drops/KEY/NAME` (its own name; the key of the file, so the
  same file dropped again goes no second time -- the key is of its name,
  size and modified time, not its contents, so a file changed with its
  size and time put back, `cp -p`, `rsync -t`, `touch -r`, is taken for
  the one there and not sent: kept so on purpose, hashing the contents
  first being a second a gigabyte before a drop goes), paced so keys are not
  left behind them, and their paths there are put in once all are
  there. Meanwhile the window is held, as if the drop took that long: its
  keys and clicks thrown away, a panel over it saying how far the files
  have got; Escape, its Stop or the sidebar's × stops them, nothing put
  in and nothing left there. Folders are not sent.
  An image dragged in with no file behind it (from a browser, a
  preview: `dragdata.rs`) is written to `~/.apex/drops/KEY/NAME` on this
  Mac first -- named as its URL names it, else as a screenshot is, its
  key the image's -- and dropped as that file: gpui's windows take only
  files, so they are registered for images too and its drag methods
  wrapped, a drag with files going to gpui as ever.
- **The Mac's clipboard on any host** (`apex_server::clip`, `apex-cli`'s
  `clip.rs`, the app's `clipboard.rs`): on a host that is not a Mac,
  apex stands in for xclip, xsel, wl-paste and wl-copy (links in
  `~/.apex/clip`, ahead on the session's PATH; `APEX_CLIPBOARD=0` for
  none), each answering as the tool does -- its options, its output byte
  for byte, its errors and exit status -- from the Mac's clipboard,
  through the daemon. So ^V in Claude Code there (`xclip -selection
  clipboard -t TARGETS -o`, then `-t image/png -o`) pastes the Mac's
  screenshot, a TIFF made a PNG. The daemon asks the app that last sent
  a terminal a key, and only within five seconds of it: a program reads
  the clipboard when the user pastes into it, not behind their back. The
  app answers on a paced stream of its own, so an image keeps no key
  waiting. What a program puts there (xclip -i, wl-copy) goes on the
  Mac's: text as OSC 52 does, the snarf buffer's too; an image as it is.
- **Transfers in the sidebar** (`transfers.rs`, as Manifold's): each
  file on its way at the foot of the sidebar's card -- its name, where
  to, a bar, how much of how much, how fast and how long to go, an × to
  stop it. One shows once it has taken a moment; a finished one stays a
  few seconds, a failed one (saying why) until its × puts it away.
- **⌘↑ ⌘↓ go from entry to entry** (`outline_jump`): in any window
  with an outline, to the entry before or after the line at the top
  (⌘↑ to the start of the one the view is in, when that is scrolled
  past); past the first, the top, and past the last, the end -- a
  terminal's screen, a text's end as End shows it. A terminal's or
  win's commands are scrolled to, the typing left where it is; a file's
  symbols are scrolled to and dot put there. A page from a buffer (a
  preview, apex diff, an agent's conversation) does it in the page
  (`TOC_SCRIPT`), by its headings, files or turns: past the last, its
  end, kept there as it grows. A window without one keeps its keys as
  they were (a page, WebKit's top and bottom).
- **⌘P goes to sections too** (`finder.rs`): every open window's
  outline, from the same source as the scrollbar's and Outline's. With
  nothing typed, the window the keys are in has its sections under it
  and the list starts on the one it is showing: ⌘P ↑ ↩ is the section
  before, ⌘P ↓ ↩ the one after. Typed, sections of every window match
  by title, or by the window's name and the title (`lib parse`), a
  window first at equal scores. Ranked once a keystroke and only the
  rows in view drawn; a page not shown parsed once a version.
- **The ssh agent survives its connection** (`apex_server::agent`, after
  Manifold's): a daemon on a host gives its terminals and commands
  SSH_AUTH_SOCK as a link, `~/.apex/agent.sock`, kept pointed at the
  newest agent that answers -- one forwarded to an apex connection (each
  `apex attach -stdio` notes its own, and takes it back as it ends), to
  an ssh login of yours, or named by `apex env agent`. A connection
  gone, the link goes to the next under the shells already running: no
  dead agent in an old shell. A daemon no connection started keeps its
  own agent (a Mac's); `apex env` says where the link points.
- **A status is said once** (as Manifold has it): in words only in its
  window's tag (the pill), or, the tag out of sight (stashed, in a
  strip, hidden behind a maximized window), in the sidebar's row for it;
  that it wants the user only by its handle's halo; per scope one mark
  more (the session's square, each session's row, the Dock). No toast
  and no line in +Errors for an agent asking or failing, nothing moved
  in the layout, no badge on the sidebar's button while the sidebar is
  open. Anything done in the window -- a click, a key, the wheel --
  takes its notification and a Done with it; a pill still blocked or
  failed stays, said plainly.
- **B4 on a box minimizes** -- shift-B1 on a laptop, which is B4
  everywhere (`tiling::colminimize`, `rowminimize`): on a
  window's box (or handle), the window down to its tag where it stands,
  its room to the window under it (over it, the last); on a column's
  box, the column a strip where it stands, its width to the nearest
  column with room, right of it first, and remembered for B1 on the
  strip to give back. B1 on a minimized window's box grows it again. A
  window alone in its column, or the last column with room, stays.
- **Menus.** The B4 tools menu (`menu.rs`, `Menu::place`) has
  menuhit's ways: up while the button is held, run on release over an
  item, the last choice remembered and opened under the pointer (which
  lands on it), a part and a lane past 25 items. It looks like the
  app's other menus (a popover), its rows centred in the interface's face;
  the item under the pointer in B2's colours -- a faint wash, its text,
  a bar at the row's left -- since choosing one is running it; no mark
  on the remembered one, which is where the menu opens; the lane down
  its right with a slim thumb. It fades in over 90 ms; let go on an
  item, the item blinks and the menu fades out (the command has run).
- **Columns on paper.** A column is the body's paper where its windows
  leave it (a body's part line at its foot, the gaps), with a hairline
  where each window meets the one above, as acme's column is white with
  black between.
- **Waiting.** A tab coming up shows the system's spinner over its words.

## Not done

Dragging sessions to reorder them in the sidebar, the sidebar sliding
out from the left edge when hidden (Manifold's), and anything that
would move the tiling's rectangles: cards with gaps and rounded
corners round every window would need the layout to leave room for
them, and the layout is the core's.
