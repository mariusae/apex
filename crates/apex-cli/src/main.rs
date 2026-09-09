//! `apex`: the command. Scripts and tools never see the wire; they see
//! this. Every subcommand attaches to a session as a tool (a follower with
//! its own replica), reads from the replica, and proposes to the leader.
//! `apex help` lists the commands; `apex help <command>` explains one.
//!
//! Flags are Go's: `-flag=value`, or `-flag` for a boolean, before the
//! arguments; `--` ends them.

use std::collections::HashMap;
use std::io::{IsTerminal, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use apex_core::*;
use apex_server::daemon::{default_socket, Daemon};
use apex_server::proto::{ClientMsg, ServerMsg};
use apex_server::remote::Remote;
use apex_server::Proposal;

const TIMEOUT: Duration = Duration::from_secs(10);

type R = Result<(), String>;

// ---- flags, Go style ---------------------------------------------------------------

#[derive(Clone, Copy)]
struct Flag {
    name: &'static str,
    /// `-flag` alone; otherwise `-flag=value`.
    boolean: bool,
    help: &'static str,
}

const fn flag(name: &'static str, help: &'static str) -> Flag {
    Flag { name, boolean: false, help }
}
const fn switch(name: &'static str, help: &'static str) -> Flag {
    Flag { name, boolean: true, help }
}

/// Parsed flags, and the arguments after them.
struct Parsed {
    flags: HashMap<&'static str, String>,
    args: Vec<String>,
}

impl Parsed {
    fn get(&self, name: &str) -> Option<&str> {
        self.flags.get(name).map(String::as_str)
    }
    fn is(&self, name: &str) -> bool {
        self.flags.contains_key(name)
    }
}

/// Flags come before the arguments; the first argument that is not a
/// flag ends them, as does `--`. `-h` and `-help` are the usage.
fn parse(defs: &[Flag], args: &[String]) -> Result<Parsed, String> {
    let mut flags = HashMap::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--" {
            i += 1;
            break;
        }
        if !a.starts_with('-') || a == "-" {
            break;
        }
        let body = a.trim_start_matches('-');
        let (name, value) = match body.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (body, None),
        };
        if name == "h" || name == "help" {
            return Err("help".into());
        }
        let Some(def) = defs.iter().find(|d| d.name == name) else {
            return Err(format!("flag provided but not defined: -{name}"));
        };
        let value = match (def.boolean, value) {
            (true, None) => "true".to_string(),
            (true, Some(v)) if v == "true" || v == "false" => v,
            (true, Some(v)) => return Err(format!("invalid boolean value {v:?} for -{name}")),
            (false, Some(v)) => v,
            (false, None) => return Err(format!("flag needs an argument: -{name}=VALUE")),
        };
        if value != "false" {
            flags.insert(def.name, value);
        }
        i += 1;
    }
    Ok(Parsed { flags, args: args[i..].to_vec() })
}

// ---- the commands and their documentation --------------------------------------------

struct Cmd {
    name: &'static str,
    usage: &'static str,
    short: &'static str,
    flags: &'static [Flag],
    long: &'static str,
    run: fn(&Ctx, &Parsed) -> R,
}

struct Ctx {
    socket: PathBuf,
    session: String,
}

const GLOBAL: &[Flag] = &[
    flag("socket", "the daemon's socket (default $APEX_SOCKET, else $TMPDIR/apex-$USER/main.sock)"),
    flag("session", "the session to work on (default $apexsession, $APEX_SESSION, else default)"),
    switch("ensure-server", "start the daemon first if none answers on the socket"),
];

const RULE_FLAGS: &[Flag] = &[
    flag("verb", "the command the rule answers: plumb (B3, the default) or a word for the tools menu"),
    flag("text", "the plumbed text (a verb's arguments) must match this regexp, whole; groups bind $0..$9"),
    flag("file", "the window's name must match this regexp"),
    flag("kind", "the window must be: file, dir, term or errors"),
    flag("isfile", "this (expanded, relative to the window's directory) must be a file"),
    flag("isdir", "... a directory"),
    flag("edit", "open this (name or name:line) in the session"),
    flag("run", "run this command on the host, in the window's directory, the selection on stdin"),
    flag("client", "ask the UI to do this verb (open, preview); it may refuse"),
    flag("args", "the argument for -client"),
    flag("tool", "ask the tool attached under this name; it may refuse"),
    flag("to", "where a -run command's output goes: errors (default) or window"),
    flag("priority", "higher rules are tried first (default 0)"),
    switch("mine", "owned by this attachment, gone when it detaches, rather than by the session"),
];

const COMMANDS: &[Cmd] = &[
    Cmd { name: "server", usage: "apex [-socket=PATH] [-session=NAME] server", short: "run the daemon in the foreground", flags: &[], run: server, long: "\
Server runs the daemon on the socket, in the foreground, with one session
(the -session flag; default) to begin with. The daemon holds every
session's state; clients attach to it over the socket, or from other
machines through ssh (see apex help sessions). It ignores SIGHUP.

Attach and new-session start a daemon themselves when none answers, so
server is for running one by hand, under a supervisor, say." },
    Cmd { name: "ls", usage: "apex ls", short: "list the daemon's sessions", flags: &[], run: ls, long: "\
Ls prints the name of every session on the daemon, one per line." },
    Cmd { name: "stop", usage: "apex stop", short: "stop the daemon, its sessions with it", flags: &[], run: stop, long: "\
Stop asks the daemon to exit. Every session ends with it: unsaved text
is lost, terminals are closed. Use it to let a daemon of an old build go
before attaching with a new one (see apex help sessions)." },
    Cmd { name: "new-session", usage: "apex new-session NAME", short: "make a session", flags: &[], run: new_session, long: "\
New-session makes a session called NAME on the daemon, starting a daemon
if none answers. Making a session that exists is fine: it is there.

A new session runs its profile: ~/.apex/profile on the daemon's host,
then the creator's (see apex help scripts)." },
    Cmd { name: "end-session", usage: "apex end-session [-f] [NAME]", short: "end a session", flags: &[switch("f", "end it even with unsaved windows")], run: end_session, long: "\
End-session ends the session NAME (the current one when omitted): its
commands and terminals are killed, everything attached to it is told
and cut off (a window on it goes offline, saying so), and the session
is gone from the daemon, which keeps running for the others. It is
refused while any of its windows has unsaved changes, unless -f. The
picker's end on a session row does the same, and so does End in the
top row of a window (End -f forces), which then closes the window." },
    Cmd { name: "rename-session", usage: "apex rename-session [FROM] TO", short: "rename a session", flags: &[], run: rename_session, long: "\
Rename-session gives the session FROM (the current session when omitted)
the name TO. Everything attached stays attached; session names are
labels, and clients follow the rename." },
    Cmd { name: "attach", usage: "apex attach [-stdio] [[DEST/]SESSION | URL] [FILE...]", short: "open the app on a session, here or on a host", flags: &[switch("stdio", "bridge the daemon's socket to stdin and stdout (what ssh runs on a host)")], run: attach, long: "\
Attach opens the app (apex-ui) on a session: the current one, SESSION on
this machine's daemon, or DEST/SESSION on a destination reached through
its provider (user@host over ssh, or provider:name; see apex help
sessions), where apex is installed first. A session URL names both. FILEs
are opened in the session.

With -stdio, attach instead copies bytes between the daemon's socket and
its own stdin and stdout, starting the daemon if none answers. That is
what runs on a host: `ssh host apex -session=NAME attach -stdio` is the
whole remote story." },
    Cmd { name: "new", usage: "apex new [LABEL]", short: "a new window, with stdin in it", flags: &[], run: new, long: "\
New makes a new, empty window in the session and prints its id. When
stdin is not a terminal, its content goes into the window:

	./somecommand | apex new

A LABEL names the window (relative to the current directory when it is
not absolute); it can be edited in the tag later, and Put writes the
window to the name in its tag. See apex help windows." },
    Cmd { name: "open", usage: "apex open FILE...", short: "open files", flags: &[], run: open, long: "\
Open opens each FILE (relative to the current directory) in the first
column, as B2 on `New FILE` would, and prints the id and name of each
window. A file already open gets no second window." },
    Cmd { name: "win", usage: "apex win list | apex win del WIN", short: "list windows, delete one", flags: &[], run: win, long: "\
Win list prints every window: its id, a mark (* for unsaved text, > for
a live window: a process behind it, a terminal's or a win's, which is
neither clean nor dirty), and its name, column by column. Win del WIN deletes a window as Del would: a
dirty window is warned once, and deleted the second time. WIN is a window
id or a unique substring of a name (see apex help windows)." },
    Cmd { name: "text", usage: "apex text read [-addr=ADDR] WIN", short: "read a window's text", flags: &[flag("addr", "print only this address (sam syntax: 3,5 or /re/)")], run: text, long: "\
Text read prints the body of window WIN. With -addr, only the text the
address selects, in the Edit language's address syntax: a line range
(2,5), a regexp (/func main/), or anything Edit takes." },
    Cmd { name: "edit", usage: "apex edit WIN PROGRAM", short: "run an Edit program on a window", flags: &[], run: edit, long: "\
Edit runs PROGRAM, in acme's Edit language (sam's commands), on the body
of window WIN, as `Edit PROGRAM` in its tag would:

	apex edit main.go ',x/foo/ c/bar/'" },
    Cmd { name: "sel", usage: "apex sel WIN [Q0 Q1]", short: "read or set a window's selection", flags: &[], run: sel, long: "\
Sel prints the selection of window WIN as two character offsets, or with
Q0 and Q1 sets it." },
    Cmd { name: "exec", usage: "apex exec [WIN] COMMAND", short: "run a command as B2 would", flags: &[], run: exec, long: "\
Exec runs COMMAND as B2 on it would: in the context of window WIN, or of
the top row when no window is given. Built-ins (Put, Del, Look, ...),
rule verbs and shell commands alike:

	apex exec main.go Put
	apex exec 'Newterm'" },
    Cmd { name: "events", usage: "apex events [-shard=S]", short: "stream the session's entries", flags: &[flag("shard", "only shards whose name starts with S (buffer, window, layout, term, meta)")], run: events, long: "\
Events prints every entry appended to the session, as it happens, one
JSON object per line: the shard, its sequence number, the attachment that
appended it, and the operation. It runs until the connection ends. This
is acme's event file, generalised: a tool that wants to follow edits,
selections, or windows reads this." },
    Cmd { name: "ps", usage: "apex ps", short: "the commands the session is running", flags: &[], run: ps, long: "\
Ps lists the commands the session's server is running: what B2 started
from a tag (shell commands, Win, the profile and attach scripts, tools
started from them), as the top row names them. One per line: the pid,
the name (the first word, what Kill and apex kill go by), where it was
started from (a window id, or top), when, its directory, and the whole
command line. Terminals' shells are listed too while they run, named
after the shell (or the command Newterm was given). A program may say
what it is called (apex tool lsp is lsp, not apex); one the server did
not start is listed for as long as it stays connected." },
    Cmd { name: "kill", usage: "apex kill NAME|PID...", short: "end running commands", flags: &[], run: kill, long: "\
Kill ends every running command named NAME, or the one with pid PID, as
Kill in the top row does: the command's process group is sent SIGTERM,
so what a shell started goes with it. The commands left running are
listed afterwards, as ps lists them." },
    Cmd { name: "term", usage: "apex term new [CMD...] | apex term send TERM TEXT | apex term read TERM", short: "terminals", flags: &[], run: term, long: "\
Term new makes a terminal window running the user's shell (the
Newterm.shell setting, else the daemon's $SHELL), or CMD through it (as
Newterm does), and prints the terminal's id. Term send
types TEXT into terminal TERM; a final newline is the Enter key. Term
read prints the terminal's screen." },
    Cmd { name: "web", usage: "apex web open URL | apex web [-name NAME] <HTML", short: "web windows", flags: &[flag("name", "the window's name (default DIR/+web)")], run: web, long: "\
Web open makes a web window on URL in the session, as Newweb URL in a
tag does, and prints the window's id. The page is rendered by the
client showing the session; only the URL is session state (its name in
the tag), so a reattach loads the page anew, and where the page goes
the name follows, with Back and Fwd along the navigation stack.

Web alone reads HTML on stdin into a window shown as a page: the HTML
is the window's text (edit, Put and Get it as text; the page follows
every change in place), named NAME, or +web in the current directory.
Relative links in it resolve against the window's directory on the
host (apexfile://). A link followed in such a page opens a web window
on it." },
    Cmd { name: "plumb", usage: "apex plumb [-dry-run] [-edit] TEXT | apex plumb rule add FLAGS | rm ID | ls", short: "plumb text; the rule table", flags: &[switch("dry-run", "only say what each rule would do"), switch("edit", "plan 9's B: only rules that open in the session, else TEXT as a path")], run: plumb, long: "\
Plumb sends TEXT through the session's plumbing rules from the current
directory, as B3 on it would: the first rule that matches and is taken
acts, and with none left the text is looked for in the window (Look).
With -dry-run, plumb prints what each rule would do instead. With -edit
only rules that open in the session are tried, and failing those TEXT is
opened as a path; that is what B does.

Plumb rule ls prints the table in the order it is tried: the rule's id,
its owner, its priority, and the flags that make it. Plumb rule rm ID
removes one. Plumb rule add installs one, owned by the session (or by
this attachment with -mine, gone when it detaches):

	apex plumb rule add -text='https?://\\S+' -client=open -args='$0'
	apex plumb rule add -verb=Preview -file='\\.md$' -run='open -a Marked $file' -priority=10
	apex plumb rule add -file='\\.go$' -text='\\w+' -tool=lsp -priority=10

See apex help rules for the predicates, the actions, and the templates." },
    Cmd { name: "B", usage: "apex B FILE[:LINE]...", short: "open files in the session (plan 9's B)", flags: &[], run: b, long: "\
B opens each FILE in the session from the current directory, at LINE when
given, through the plumbing rules that open in the session (see apex
help rules), else as a path. It is plan 9's B: a shell in an apex terminal
has it as a function." },
    Cmd { name: "editor", usage: "apex editor FILE", short: "$EDITOR: open FILE, exit when its window is deleted", flags: &[], run: editor, long: "\
Editor is plan9port's editinacme for apex, for use as $EDITOR: it opens
FILE in the session (through the rules that open in the session, as B
does, so an open window is shown and the pointer warped to it), waits
until the file's window is deleted, and exits. Terminals and commands
have EDITOR set to apex-editor, a link to the apex binary beside it that
does the same (one word, since zsh and rc do not split $EDITOR into
words), unless the profile says otherwise." },
    Cmd { name: "env", usage: "apex env [-import] [KEY=VALUE...]", short: "the session's environment", flags: &[switch("import", "this environment's changes become the session's")], run: env_cmd, long: "\
Env sets variables in the session's environment: what every terminal and
command started from then on gets, beyond the daemon's own. With no
arguments it prints the environment.

With -import, what env's own environment has changed, against what the
session gives a command, is applied to the session's: variables set or
changed are set there, and variables dropped are unset. The profile's
exit hook runs this, so the profile's environment at its end is the
session's, rc functions included (see apex help scripts)." },
    Cmd { name: "set", usage: "apex set [KEY VALUE]", short: "a setting", flags: &[], run: set, long: "\
Set records a setting in the session: the session's own, or the attaching
client's when run from its attach script (see apex help scripts), gone
when that client detaches. A client reads its own settings first, then
the session's. With no arguments, set prints every setting with its
owner. Settings in use:

	Preview.EXT CMD   the converter for files with that extension: a
	                  command reading the file on stdin, writing HTML
	                  (apex help preview); .md and .markdown have
	                  'apex md', .html, .htm and .svg 'cat' unless set;
	                  an empty value turns one off
	Preview APP       the app a rule's own -client=preview falls back to
	lsp.LANG CMD      the language server for LANG (apex help tool)
	lsp.root MARKER   an overriding workspace-root marker for every language;
	lsp.LANG.root MARKER overrides it for one language
	Newterm.shell SH  the shell Newterm runs (a path, or a name on the
	                  daemon's PATH); the daemon's $SHELL otherwise" },
    Cmd { name: "cat", usage: "apex cat PATH", short: "the bytes of a file on the host", flags: &[], run: cat, long: "\
Cat prints the file PATH as it is on the session's host, whatever machine
the command runs on (apex io GET file://PATH)." },
    Cmd { name: "io", usage: "apex io [-watch] METHOD URL [<BODY]", short: "a request on the session's I/O plane", flags: &[switch("watch", "keep the stream open: every change to the file comes again")], run: io_cmd, long: "\
Io sends one request on the session's I/O plane, the streams beside the
log that clients use for files and, later, the network (WEB.md), and
prints the answer's body. The status goes to stderr when it is not 200
and the exit status is 1 then. What the host answers today:

	GET file://PATH    the file's bytes
	PUT file://PATH    stdin written to the file
	GET -watch         the file now and again on every change, each
	                   version's contents to stdout, until interrupted
	GET http://URL     fetched by the host, on its network (any method;
	                   stdin is the body when it is not a terminal)
	CONNECT HOST:PORT  a byte tunnel to there from the host: stdin goes
	                   in, what comes back goes to stdout (nc)

The plane is HTTP-shaped: a request opens a numbered stream, the answer
is a status and a body, and a watch or a tunnel is a stream that does
not end on its own." },
    Cmd { name: "preview", usage: "apex preview FILE", short: "show FILE as a page, live", flags: &[], run: preview_cmd, long: "\
Preview shows FILE as a page (Preview in its tag does the same): the
file's buffer, opened if it is not, goes through the converter its
extension names in the settings (Preview.EXT, see apex help set) and
the HTML is a window named FILE+Preview beside it, kept so as the
buffer changes, unsaved edits included. The converter runs on the
host as apex tool preview FILE, a command named preview (apex ps,
Kill preview); it ends with either window. Relative links in the page
resolve in the file's directory (apexfile://)." },
    Cmd { name: "md", usage: "apex md <MARKDOWN", short: "Markdown on stdin to HTML on stdout", flags: &[], run: md, long: "\
Md converts Markdown on stdin to an HTML page on stdout: CommonMark
with tables, footnotes, strikethrough and task lists, styled as GitHub
renders Markdown (its own stylesheet, fonts included). Every block is preceded by an empty span carrying the
source line it starts on (data-line, counted from 1), which is how a
preview follows dot; a converter of your own may do the same. It is the
converter Preview uses for .md and .markdown files unless a setting
names another." },
    Cmd { name: "tool", usage: "apex tool win [CMD...] | apex tool lsp | apex tool preview FILE", short: "the tools that come with apex", flags: &[], run: tool_cmd, long: "\
Tool runs one of the tools that come with apex. None is privileged: each
attaches to the session like anything else on this command line and works
through the same protocol.

apex tool win [CMD...] is acme's win: a shell ($acmeshell, rc by default;
or CMD) in a text window named dir/-host in the last column, the editable
transcript. The shell's output is inserted at the output point; what you
type after it goes to the shell at each newline (or ^D), and the point
moves past it; ^C or DEL typed interrupts and drops the typing; Send
appends the snarf buffer and a newline; the tools menu offers Interrupt
and EOF; labels in the output (awd) name the window. A program that
turns echo off (a password) is served raw: keys go at once and leave the
window. Win in a tag runs this in the window's directory; Kill Win ends
it. Newterm is the other kind of shell window: a real terminal, running
the user's shell.

apex tool lsp attaches as the tool named lsp and runs language
servers for the files open in it, one per workspace root: gopls,
rust-analyzer, pyright, typescript-language-server and clangd unless a
setting lsp.LANG names another command. The optional lsp.root setting names
a workspace marker that takes precedence over the built-in language markers;
lsp.LANG.root overrides it for one language. Documents are opened as buffers
appear and every edit is fed incrementally. Diagnostics go to root/+lsp,
one plumbable file:line:col: message per line.

Its rules, gone when it exits: cmd-B3 on an identifier in a source file
goes to the definition (B3 itself stays acme's look; on a laptop, where
cmd-click is B3, ctrl-cmd-click is cmd-B3), and the tools menu of a
source window offers Def Refs Type Hov Sig Fmt Rn. Definitions open and select; references, hover and signatures go
to +Errors; Fmt replaces the text with the server's formatting; Rn NAME
renames. Back and Fwd, offered everywhere, walk the session's navigation
stack: every jump (Def, B3 on file:line, apex B, the app's cmd-p) records
where it left from, and Back returns there, Fwd undoes a Back; cmd-[ and
shift-cmd-B3 are Back, cmd-] is Fwd.

Start it from the host's profile: apex tool lsp & (see apex help scripts).
APEX_LSP_DEBUG=1 traces the JSON-RPC on stderr." },
    Cmd { name: "label", usage: "apex label TEXT", short: "title this terminal's window", flags: &[], run: label_cmd, long: "\
Label gives the window of the terminal it runs in the title TEXT,
through the escape sequence acme's win reads (plan9port's label). A
terminal's window is named DIR/-TITLE, DIR being the directory the shell
last reported (OSC 7; apex awd reports it) and TITLE its title (a label,
or an xterm title); with no directory ever reported the name is -TITLE,
and before either it is where the shell started and the host." },
    Cmd { name: "awd", usage: "apex awd [LABEL]", short: "name this terminal's window after its directory", flags: &[], run: awd, long: "\
Awd reports the terminal's working directory (OSC 7) and titles the
window LABEL (the host's name unless given), so the window is named
PWD/-LABEL, as plan9port's awd names it. rc does this on every cd in an
apex terminal; for zsh, bash and fish see examples/profile." },
    Cmd { name: "version", usage: "apex version", short: "print the build id", flags: &[], run: version, long: "\
Version prints this build's id, a hash of the sources it was built from.
A daemon says its own on every connection; a client of another build
refuses to go on and says so." },
];

/// Help topics beyond the commands.
const TOPICS: &[(&str, &str, &str)] = &[
    ("sessions", "sessions, daemons and their URLs", "\
A daemon holds sessions; a session is a set of windows, buffers and
terminals on one machine, with its state kept by the daemon so that
clients can attach and detach without losing anything. Every session is a
URL: local:///NAME on this machine's daemon, ssh://user@host/NAME on a
host reached over ssh, PROVIDER://ARG/NAME through a provider. The
default session is called default.

A provider is a command apex-remote-PROVIDER on the PATH that runs a
command on a destination: apex-remote-PROVIDER DEST COMMAND. ssh is built
in. Attaching through a provider installs this build's apex and rc on the
destination (in ~/.apex/bin), starts a daemon there if none answers, and
bridges frames through `apex attach -stdio` run there.

A daemon says its build id first on every connection. A client of another
build stops there and says what to do: when the daemon's sessions can be
let go, apex stop on its machine, then attach again."),
    ("scripts", "the profile and attach scripts", "\
Two scripts, like a shell's profile and rc. When a session is made, one rc
on its host sources ~/.apex/profile there, then the creator's
~/.apex/profile (shipped along; skipped when it is the same file). It runs
like any command, named profile in the top row with its output in
+Errors, with apexsession, APEX_SOCKET and apexclient set, so apex in it
configures the session: apex open, apex exec Newcol, apex set, apex
plumb rule add, apex tool lsp &. The profile's environment at its end
is the session's: variables it sets or changes (x=y, path=($home/bin
$path)) and functions it defines (fn g { ... }, exported as rc does)
are what every terminal and command gets from then on, and what it
unsets is gone. An exit hook (rc's sigexit) sends it back through apex
env -import, so a profile that defines its own sigexit forgoes this.

Every time a client attaches, its ~/.apex/attach runs on the host the
same way, with apexattachment naming the attaching client, so apex set
there records that client's own settings (apex help set). See
examples/profile for one that has shells name their windows on cd."),
    ("rules", "plumbing rules", "\
B3 (and apex plumb) walks the session's rule table in priority order,
highest first, then by age; the first rule that matches and is taken ends
the walk, and with none left the text is looked for in the window (Look).
A rule's verb is the command it answers: plumb is B3; any other verb is
offered in the tools menu (B4, or shift-click) of every window the rule
applies to, and B2 on the word does the same. The verb exec is special:
it takes every B2 command in the windows it applies to that no builtin
and no other verb took, the whole line as its text (win's rule, so that
B2 on an old command line runs it again); it is no word in the menu.

Predicates (all given must hold):
	-text=RE      the plumbed text (a verb's arguments) matches RE, whole;
	              its groups bind $0..$9
	-file=RE      the window's name matches RE
	-kind=K       file, dir, term, errors or web
	-isfile=EXPR  EXPR, expanded, is a file (relative to the window's directory)
	-isdir=EXPR   ... a directory
Actions (exactly one):
	-edit=EXPR    open EXPR (name, or name:line) in the session
	-run=CMD      run CMD on the host in the window's directory, the
	              selection on stdin, output to dir/+Errors
	-client=VERB -args=ARGS
	              ask the UI that asked to do VERB with ARGS (open a URL,
	              say); a UI that cannot refuses, and the walk goes on
	-tool=NAME    ask the tool attached as NAME; it answers within a second
	              or is taken to refuse (NACK), and the walk goes on
Templates expand $0..$9, $file, $dir, $win, $line and $sel.

Rules from the command line are the session's. A UI installs its own on
attach (URLs go to the platform's open; Preview where a Preview.EXT
setting names an app), a tool those naming it; both go when their owner
does. The session starts with three rules at priority -100 that open name
and name:line when they exist, as B3 always did. apex plumb -dry-run TEXT
prints what each rule would do."),
    ("windows", "naming windows", "\
Commands take a window as WIN: its id (apex win list), or a unique
substring of its name. A window's name is the first word of its tag; a
file's window is named by its path, a directory's ends in /, a terminal's
is dir/-host (or dir/-cmd), command output goes to dir/+Errors.

The name in a tag can be edited: type a new one and Put writes the window
there, the buffer taking the name; ^F completes file names in the tag.
That is how an empty window from New (or apex new, with stdin in it)
becomes a file."),
];

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // `apex-editor FILE`, the link $EDITOR names: `apex editor FILE`
    let argv0 = std::env::args().next().unwrap_or_default();
    if argv0.rsplit('/').next() == Some("apex-editor") {
        args.insert(0, "editor".into());
    }
    let global = match parse(GLOBAL, &args) {
        Ok(p) => p,
        Err(e) if e == "help" => overview(),
        Err(e) => die(&e),
    };
    let socket = global.get("socket").map(PathBuf::from).or_else(|| std::env::var("APEX_SOCKET").ok().map(PathBuf::from)).unwrap_or_else(default_socket);
    // in a terminal apex runs, `apexsession` names the session it is in
    let session = global
        .get("session")
        .map(String::from)
        .or_else(|| std::env::var("apexsession").ok())
        .or_else(|| std::env::var("APEX_SESSION").ok())
        .unwrap_or_else(|| "default".into());
    let ctx = Ctx { socket, session };
    if global.is("ensure-server") {
        if let Err(e) = ensure_server(&ctx.socket, &ctx.session) {
            die(&e);
        }
    }
    let Some(name) = global.args.first().cloned() else { overview() };
    if name == "help" {
        help(global.args.get(1).map(String::as_str));
    }
    let Some(cmd) = COMMANDS.iter().find(|c| c.name == name) else {
        eprintln!("apex {name}: unknown command\nRun 'apex help' for usage.");
        std::process::exit(2);
    };
    let parsed = match parse(cmd.flags, &global.args[1..]) {
        Ok(p) => p,
        Err(e) if e == "help" => {
            usage(cmd);
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("apex {name}: {e}");
            usage(cmd);
            std::process::exit(2);
        }
    };
    if let Err(e) = (cmd.run)(&ctx, &parsed) {
        if e == "usage" {
            usage(cmd);
            std::process::exit(2);
        }
        eprintln!("apex {name}: {e}");
        std::process::exit(1);
    }
}

fn die(msg: &str) -> ! {
    eprintln!("apex: {msg}\nRun 'apex help' for usage.");
    std::process::exit(2);
}

/// `apex CMD -h`: the usage, briefly, and where the rest is.
fn usage(cmd: &Cmd) {
    eprintln!("usage: {}", cmd.usage);
    for f in cmd.flags {
        eprintln!("\t-{}{}\n\t\t{}", f.name, if f.boolean { "" } else { "=VALUE" }, f.help);
    }
    eprintln!("Run 'apex help {}' for details.", cmd.name);
}

/// `apex help`, `apex`: the commands and the topics.
fn overview() -> ! {
    println!("Apex is acme, remade: a text editor and shell, remote and scriptable.\n");
    println!("Usage:\n\n\tapex [-socket=PATH] [-session=NAME] [-ensure-server] <command> [arguments]\n");
    println!("The commands are:\n");
    for c in COMMANDS {
        println!("\t{:<16}{}", c.name, c.short);
    }
    println!("\nUse \"apex help <command>\" for more information about a command.\n");
    println!("Additional help topics:\n");
    for (t, s, _) in TOPICS {
        println!("\t{:<16}{}", t, s);
    }
    println!("\nUse \"apex help <topic>\" for more information about that topic.\n");
    println!("The flags apply to every command:\n");
    for f in GLOBAL {
        println!("\t-{}{}\n\t\t{}", f.name, if f.boolean { "" } else { "=VALUE" }, f.help);
    }
    std::process::exit(0);
}

/// `apex help CMD|TOPIC`.
fn help(what: Option<&str>) -> ! {
    let Some(what) = what else { overview() };
    if let Some(c) = COMMANDS.iter().find(|c| c.name == what) {
        println!("usage: {}\n", c.usage);
        if !c.flags.is_empty() {
            for f in c.flags {
                println!("\t-{}{}\n\t\t{}", f.name, if f.boolean { "" } else { "=VALUE" }, f.help);
            }
            println!();
        }
        println!("{}", c.long);
        if c.name == "plumb" {
            println!("\nThe flags of plumb rule add:\n");
            for f in RULE_FLAGS {
                println!("\t-{}{}\n\t\t{}", f.name, if f.boolean { "" } else { "=VALUE" }, f.help);
            }
        }
        std::process::exit(0);
    }
    if let Some((_, _, text)) = TOPICS.iter().find(|(t, _, _)| *t == what) {
        println!("{text}");
        std::process::exit(0);
    }
    eprintln!("apex help {what}: unknown help topic. Run 'apex help'.");
    std::process::exit(2);
}

fn tool(ctx: &Ctx) -> Result<Remote, String> {
    Remote::connect_as(&ctx.socket, &ctx.session, "apex-cli", AttachmentKind::Tool).map_err(|e| format!("{}: {e}", ctx.socket.display()))
}

/// Pump until `done` or the timeout.
fn wait(r: &mut Remote, mut done: impl FnMut(&Remote) -> bool) -> Result<(), String> {
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        if done(r) {
            return Ok(());
        }
        match r.step(Duration::from_millis(20)) {
            Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => return Err("connection closed".into()),
        }
    }
    if done(r) {
        Ok(())
    } else {
        Err("timed out".into())
    }
}

// ---- server, sessions -----------------------------------------------------------

fn server(ctx: &Ctx, _: &Parsed) -> R {
    if let Some(d) = ctx.socket.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    eprintln!("apex server: session {} on {}", ctx.session, ctx.socket.display());
    Daemon::run(&ctx.socket, &ctx.session).map_err(|e| e.to_string())
}

/// Start a daemon in the background if the socket does not answer.
fn ensure_server(socket: &Path, session: &str) -> R {
    if UnixStream::connect(socket).is_ok() {
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    apex_server::daemon::spawn_server(&exe, socket, session).map_err(|e| format!("start server: {e}"))
}

fn ls(ctx: &Ctx, _: &Parsed) -> R {
    for s in apex_server::remote::list_sessions(&ctx.socket).map_err(|e| format!("{}: {e}", ctx.socket.display()))? {
        println!("{s}");
    }
    Ok(())
}

fn stop(ctx: &Ctx, _: &Parsed) -> R {
    apex_server::remote::stop(&ctx.socket).map_err(|e| format!("{}: {e}", ctx.socket.display()))
}

fn print_procs(procs: &[apex_server::Running]) {
    for r in procs {
        let from = match r.ctx {
            ExecCtx::Window(w) => w.0.to_string(),
            ExecCtx::Column(c) => format!("col {c}"),
            ExecCtx::Top => "top".into(),
        };
        let started = std::time::UNIX_EPOCH + Duration::from_secs(r.started);
        let ago = std::time::SystemTime::now().duration_since(started).map(|d| d.as_secs()).unwrap_or(0);
        let when = if ago < 60 { format!("{ago}s ago") } else if ago < 3600 { format!("{}m ago", ago / 60) } else { format!("{}h{:02}m ago", ago / 3600, (ago % 3600) / 60) };
        println!("{}\t{}\t{from}\t{when}\t{}\t{}", r.pid, r.name, r.dir, r.cmd);
    }
}

fn ps(ctx: &Ctx, _: &Parsed) -> R {
    let mut c = tool(ctx)?;
    print_procs(&c.ps(TIMEOUT)?);
    Ok(())
}

fn kill(ctx: &Ctx, p: &Parsed) -> R {
    if p.args.is_empty() {
        return Err("usage".into());
    }
    let mut c = tool(ctx)?;
    let before = c.ps(TIMEOUT)?;
    let known = |t: &String| before.iter().any(|r| r.name == *t || r.pid.to_string() == *t);
    if let Some(t) = p.args.iter().find(|t| !known(t)) {
        return Err(format!("{t}: no such command; apex ps lists them"));
    }
    let left = c.kill(p.args.clone(), TIMEOUT)?;
    print_procs(&left);
    Ok(())
}

fn version(_: &Ctx, _: &Parsed) -> R {
    println!("apex build {} protocol {}", apex_server::BUILD_ID, apex_server::proto::PROTOCOL);
    Ok(())
}

fn tool_cmd(ctx: &Ctx, p: &Parsed) -> R {
    match p.args.first().map(String::as_str) {
        Some("lsp") => apex_tool_lsp::run(&ctx.socket, &ctx.session),
        Some("win") => {
            let dir = std::env::current_dir().map_err(|e| e.to_string())?;
            apex_tool_win::run(&ctx.socket, &ctx.session, &dir, &p.args[1..])
        }
        Some("preview") => match p.args.get(1) {
            Some(file) => match apex_tool_preview::run(&ctx.socket, &ctx.session, file) {
                Err(e) if e == "shown" => Ok(()), // another preview of it is up: shown
                r => r,
            },
            None => Err("usage".into()),
        },
        _ => Err("usage".into()),
    }
}

/// `apex preview FILE`: the tool, run on the host as a command of the
/// session (so the terminal is free and the top row names it).
fn preview_cmd(ctx: &Ctx, p: &Parsed) -> R {
    let [file] = p.args.as_slice() else { return Err("usage".into()) };
    let file = std::path::absolute(file).map_err(|e| format!("{file}: {e}"))?.display().to_string();
    let mut c = tool(ctx)?;
    let text = format!("apex tool preview {}", apex_server::shell_quote(&file));
    c.propose(Proposal::Exec { ctx: ExecCtx::Top, text }, TIMEOUT)?;
    Ok(())
}

/// `apex md`: Markdown on stdin to an HTML page on stdout.
fn md(_: &Ctx, _: &Parsed) -> R {
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text).map_err(|e| e.to_string())?;
    let html = markdown_page(&text);
    std::io::stdout().write_all(html.as_bytes()).map_err(|e| e.to_string())
}

/// Markdown as a whole page, with the stylesheet Preview pages get.
pub fn markdown_page(text: &str) -> String {
    use pulldown_cmark::{html, Options, Parser};
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_HEADING_ATTRIBUTES);
    // a marker before every block with the source line it starts on, so
    // a preview can follow dot (WEB.md §3.3)
    let line_starts: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let line_at = |offset: usize| line_starts.partition_point(|&s| s <= offset);
    let mut events: Vec<pulldown_cmark::Event> = Vec::new();
    for (ev, range) in Parser::new_ext(text, opts).into_offset_iter() {
        use pulldown_cmark::{CowStr, Event, Tag};
        if let Event::Start(Tag::Paragraph | Tag::Heading { .. } | Tag::BlockQuote(_) | Tag::CodeBlock(_) | Tag::Item | Tag::Table(_) | Tag::HtmlBlock) = &ev {
            events.push(Event::Html(CowStr::from(format!("<span class=\"apex-line\" data-line=\"{}\"></span>", line_at(range.start)))));
        }
        events.push(ev);
    }
    let mut body = String::new();
    html::push_html(&mut body, events.into_iter());
    format!("<!doctype html>\n<html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><style>{MD_STYLE}\n{MD_PAGE}</style></head><body><article class=\"markdown-body\">\n{body}</article></body></html>\n")
}

/// GitHub's own Markdown stylesheet (github-markdown-css, MIT), and the
/// page around it as GitHub lays it out.
const MD_STYLE: &str = include_str!("github-markdown-light.css");
const MD_PAGE: &str = ".markdown-body { box-sizing: border-box; min-width: 200px; max-width: 980px; margin: 0 auto; padding: 45px; } @media (max-width: 767px) { .markdown-body { padding: 15px; } } body { margin: 0; background: #fff; }";

fn rename_session(ctx: &Ctx, p: &Parsed) -> R {
    let (from, to) = match p.args.as_slice() {
        [a, b] => (a.clone(), b.clone()),
        [b] => (ctx.session.clone(), b.clone()),
        _ => return Err("usage".into()),
    };
    apex_server::remote::rename_session(&ctx.socket, &from, &to).map_err(|e| e.to_string())
}

fn end_session(ctx: &Ctx, p: &Parsed) -> R {
    let name = match p.args.as_slice() {
        [] => ctx.session.clone(),
        [n] => n.clone(),
        _ => return Err("usage".into()),
    };
    apex_server::remote::end_session(&ctx.socket, &name, p.get("f").is_some()).map_err(|e| e.to_string())
}

fn new_session(ctx: &Ctx, p: &Parsed) -> R {
    let [name] = p.args.as_slice() else { return Err("usage".into()) };
    ensure_server(&ctx.socket, &ctx.session)?;
    apex_server::remote::new_session(&ctx.socket, name, apex_server::remote::local_profile()).map_err(|e| e.to_string())
}

// ---- attach -----------------------------------------------------------------------

fn attach(ctx: &Ctx, p: &Parsed) -> R {
    let mut args = p.args.clone();
    let target = if args.first().is_some_and(|a| !Path::new(a).exists()) { args.remove(0) } else { ctx.session.clone() };
    // a URL names the destination and the session in one
    let target = match apex_server::providers::SessionUrl::parse(&target) {
        Some(u) if u.is_local() => u.session,
        Some(u) => format!("{}/{}", u.dest().unwrap_or_default(), u.session),
        None => target,
    };
    if p.is("stdio") {
        // the bridge on a host: the daemon there may need starting
        ensure_server(&ctx.socket, &ctx.session)?;
        return bridge(&ctx.socket);
    }
    let ui = std::env::current_exe().map_err(|e| e.to_string())?.with_file_name("apex-ui");
    let status = match apex_server::providers::split_spec(&target) {
        Some((host, sess)) => {
            // remote: the UI talks to `ssh host apex attach -stdio`,
            // after our apex is put on the host
            Command::new(&ui).arg("--remote").arg(host).arg("--session").arg(sess).args(&args).status()
        }
        None => {
            ensure_server(&ctx.socket, &target)?;
            Command::new(&ui).arg("--attach").arg(&ctx.socket).arg("--session").arg(&target).args(&args).status()
        }
    }
    .map_err(|e| format!("{}: {e}", ui.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("apex-ui exited with {status}"))
    }
}

/// Copy bytes between the socket and stdin/stdout, both ways, until one
/// side closes. Frames pass through untouched: this is the whole remote
/// story on the server side.
fn bridge(socket: &Path) -> R {
    let s = UnixStream::connect(socket).map_err(|e| format!("{}: {e}", socket.display()))?;
    let mut from_sock = s.try_clone().map_err(|e| e.to_string())?;
    let mut to_sock = s;
    let up = std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let _ = std::io::copy(&mut stdin, &mut to_sock);
        let _ = to_sock.shutdown(std::net::Shutdown::Write);
    });
    {
        let mut stdout = std::io::stdout().lock();
        let mut buf = [0u8; 64 * 1024];
        loop {
            match from_sock.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if stdout.write_all(&buf[..n]).and_then(|_| stdout.flush()).is_err() {
                        break;
                    }
                }
            }
        }
    }
    // stdin may still be open (the ssh session); we are done regardless
    drop(up);
    Ok(())
}

// ---- windows and text ---------------------------------------------------------------

fn find_window(c: &Remote, spec: &str) -> Result<WindowId, String> {
    if let Ok(n) = spec.parse::<u64>() {
        let w = WindowId(n);
        return c.node.state.window(w).map(|_| w).map_err(|e| e.to_string());
    }
    // a name exactly first (notes.md beside notes.md+Preview), then a
    // unique substring
    let exact: Vec<WindowId> = c.node.state.windows.keys().copied().filter(|w| c.node.window_name(*w) == spec).collect();
    if exact.len() == 1 {
        return Ok(exact[0]);
    }
    let hits: Vec<WindowId> = c.node.state.windows.keys().copied().filter(|w| c.node.window_name(*w).contains(spec)).collect();
    match hits.len() {
        1 => Ok(hits[0]),
        0 => Err(format!("no window matches {spec:?}")),
        n => Err(format!("{n} windows match {spec:?}")),
    }
}

fn body_text(c: &Remote, w: WindowId) -> Result<String, String> {
    let b = c.node.view_buffer(ViewId::Body(w)).map_err(|e| e.to_string())?;
    Ok(c.node.state.buffer(b).map_err(|e| e.to_string())?.text.to_string())
}

/// `apex new [LABEL]`: a new window, always; stdin's content in it when
/// stdin is not a terminal; LABEL as its name.
fn new(ctx: &Ctx, p: &Parsed) -> R {
    let label = match p.args.as_slice() {
        [] => None,
        [l] => Some(l.clone()),
        _ => return Err("usage".into()),
    };
    let mut c = tool(ctx)?;
    let before: Vec<WindowId> = c.node.state.windows.keys().copied().collect();
    c.propose(Proposal::Exec { ctx: ExecCtx::Top, text: "New".into() }, TIMEOUT)?;
    wait(&mut c, |r| r.node.state.windows.keys().any(|w| !before.contains(w)))?;
    let w = c.node.state.windows.keys().copied().find(|w| !before.contains(w)).ok_or("no window appeared")?;
    let b = c.node.state.window(w).map_err(|e| e.to_string())?.body_buffer().ok_or("not a text window")?;
    if !std::io::stdin().is_terminal() {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text).map_err(|e| e.to_string())?;
        if !text.is_empty() {
            let version = c.node.state.buffer(b).map_err(|e| e.to_string())?.version;
            c.propose(Proposal::ReplaceRange { dir: None, buffer: b, version, q0: 0, q1: 0, text }, TIMEOUT)?;
        }
    }
    if let Some(l) = label {
        let name = if l.starts_with('/') { l } else { std::env::current_dir().map_err(|e| e.to_string())?.join(l).display().to_string() };
        c.propose(Proposal::Rename { buffer: b, window: w, name }, TIMEOUT)?;
    }
    println!("{}", w.0);
    Ok(())
}

/// `apex open FILE...`: open files in the first column.
fn open(ctx: &Ctx, p: &Parsed) -> R {
    if p.args.is_empty() {
        return Err("usage".into());
    }
    let mut c = tool(ctx)?;
    let col = c.node.state.layout.cols.first().ok_or("no column")?.id;
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let before = c.node.state.windows.len();
    for f in &p.args {
        let path = cwd.join(f);
        c.send(&ClientMsg::OpenFile { col, ctx: ExecCtx::Top, name: path.to_string_lossy().to_string() });
    }
    // opening an already-open file adds no window; wait for what is new
    let want = p.args.len();
    let _ = wait(&mut c, |r| r.node.state.windows.len() >= before + want);
    for f in &p.args {
        let path = cwd.join(f).to_string_lossy().to_string();
        if let Ok(w) = find_window(&c, &path) {
            println!("{}\t{}", w.0, c.node.window_name(w));
        }
    }
    Ok(())
}

fn win(ctx: &Ctx, p: &Parsed) -> R {
    let mut c = tool(ctx)?;
    match p.args.first().map(|s| s.as_str()) {
        Some("list") | None => {
            for col in &c.node.state.layout.cols {
                for slot in &col.wins {
                    let w = slot.window;
                    let dirty = c.node.state.window(w).ok().and_then(|x| x.body_buffer()).and_then(|b| c.node.state.buffer(b).ok()).is_some_and(|b| b.dirty());
                    let mark = if c.node.window_live(w) { ">" } else if dirty { "*" } else { " " };
                    println!("{}\t{mark}{}", w.0, c.node.window_name(w));
                }
            }
            Ok(())
        }
        Some("del") => {
            let w = find_window(&c, p.args.get(1).ok_or("usage")?)?;
            c.propose(Proposal::Exec { ctx: ExecCtx::Window(w), text: "Del".into() }, TIMEOUT)?;
            Ok(())
        }
        _ => Err("usage".into()),
    }
}

fn text(ctx: &Ctx, p: &Parsed) -> R {
    // the flags follow `read`, as the usage says
    let (Some(read), rest) = (p.args.first(), &p.args[p.args.len().min(1)..]) else { return Err("usage".into()) };
    if read != "read" {
        return Err("usage".into());
    }
    let f = parse(&[flag("addr", "")], rest).map_err(|e| if e == "help" { "usage".to_string() } else { e })?;
    let c = tool(ctx)?;
    match f.args.as_slice() {
        [spec] => {
            let w = find_window(&c, spec)?;
            let text = body_text(&c, w)?;
            let out = match f.get("addr") {
                None => text,
                Some(a) => {
                    let t = Text::new(&text);
                    let (q0, q1) = c.node.selection(ViewId::Body(w)).unwrap_or((0, 0));
                    let name = c.node.window_name(w);
                    let outcome = apex_edit::Edit::new().run(&t, (q0, q1), Some(&name), &format!("{a}p")).map_err(|e| e.to_string())?;
                    outcome.output_string()
                }
            };
            print!("{out}");
            let _ = std::io::stdout().flush();
            Ok(())
        }
        _ => Err("usage".into()),
    }
}

fn edit(ctx: &Ctx, p: &Parsed) -> R {
    let [spec, program] = p.args.as_slice() else { return Err("usage".into()) };
    let mut c = tool(ctx)?;
    let w = find_window(&c, spec)?;
    c.propose(Proposal::Edit { window: w, program: program.clone() }, TIMEOUT)?;
    Ok(())
}

fn sel(ctx: &Ctx, p: &Parsed) -> R {
    let mut c = tool(ctx)?;
    let spec = p.args.first().ok_or("usage")?;
    let w = find_window(&c, spec)?;
    let v = ViewId::Body(w);
    match (p.args.get(1), p.args.get(2)) {
        (Some(q0), Some(q1)) => {
            let q0 = q0.parse().map_err(|_| "Q0 must be a number")?;
            let q1 = q1.parse().map_err(|_| "Q1 must be a number")?;
            c.propose(Proposal::Select { view: v, q0, q1 }, TIMEOUT)?;
            Ok(())
        }
        (None, None) => {
            let (q0, q1) = c.node.selection(v).map_err(|e| e.to_string())?;
            println!("{q0} {q1}");
            Ok(())
        }
        _ => Err("usage".into()),
    }
}

fn exec(ctx: &Ctx, p: &Parsed) -> R {
    let mut c = tool(ctx)?;
    let (ectx, text) = match p.args.as_slice() {
        [cmd] => (ExecCtx::Top, cmd.clone()),
        [w, cmd] => (ExecCtx::Window(find_window(&c, w)?), cmd.clone()),
        _ => return Err("usage".into()),
    };
    c.propose(Proposal::Exec { ctx: ectx, text }, TIMEOUT)?;
    Ok(())
}

fn events(ctx: &Ctx, p: &Parsed) -> R {
    let mut c = tool(ctx)?;
    let only = p.get("shard").map(String::from);
    let mut out = std::io::stdout().lock();
    loop {
        let m = match c.link.rx.recv() {
            Ok(m) => m,
            Err(_) => return Ok(()),
        };
        if let ServerMsg::Entries { shard, entries } = &m {
            if only.as_ref().is_none_or(|o| shard.to_string().starts_with(o)) {
                for e in entries {
                    let line = serde_json::json!({ "shard": shard.to_string(), "seq": e.seq, "attachment": e.attachment.0, "epoch": e.epoch, "op": e.op });
                    if writeln!(out, "{line}").is_err() {
                        return Ok(());
                    }
                }
                let _ = out.flush();
            }
        }
        c.handle(m);
    }
}

fn term(ctx: &Ctx, p: &Parsed) -> R {
    let mut c = tool(ctx)?;
    let find_term = |c: &Remote, spec: &str| -> Result<TermId, String> {
        let n: u64 = spec.parse().map_err(|_| "TERM must be a number")?;
        if c.node.state.terms.contains_key(&TermId(n)) {
            Ok(TermId(n))
        } else {
            Err(format!("no terminal {n}"))
        }
    };
    match p.args.first().map(|s| s.as_str()) {
        Some("new") => {
            // `term new CMD...`: the terminal runs CMD instead of a shell
            let before: Vec<TermId> = c.node.state.terms.keys().copied().collect();
            let text = std::iter::once("Newterm").chain(p.args[1..].iter().map(String::as_str)).collect::<Vec<_>>().join(" ");
            c.propose(Proposal::Exec { ctx: ExecCtx::Top, text }, TIMEOUT)?;
            wait(&mut c, |r| r.node.state.terms.keys().any(|t| !before.contains(t)))?;
            let t = c.node.state.terms.keys().find(|t| !before.contains(t)).unwrap();
            println!("{}", t.0);
            Ok(())
        }
        Some("send") => {
            let t = find_term(&c, p.args.get(1).ok_or("usage")?)?;
            // the text is pasted; a final newline is the Enter key, since
            // shells take a pasted newline literally (bracketed paste)
            let mut text = p.args[2..].join(" ");
            let enter = text.ends_with('\r') || text.ends_with('\n');
            if enter {
                text.pop();
            }
            if !text.is_empty() {
                c.send(&ClientMsg::TermPaste { term: t, text });
            }
            if enter {
                c.send(&ClientMsg::TermKey { term: t, key: apex_server::TermKey { key: "enter".into(), text: Some("\r".into()), shift: false, control: false, alt: false } });
            }
            // give the daemon a moment to take it before we hang up
            let _ = c.step(Duration::from_millis(50));
            Ok(())
        }
        Some("read") => {
            let t = find_term(&c, p.args.get(1).ok_or("usage")?)?;
            let term = &c.node.state.terms[&t];
            for row in &term.grid {
                let line: String = row.iter().map(|c| c.ch).collect();
                println!("{}", line.trim_end());
            }
            Ok(())
        }
        _ => Err("usage".into()),
    }
}

fn plumb(ctx: &Ctx, p: &Parsed) -> R {
    if p.args.first().is_some_and(|a| a == "rule") {
        return rule(ctx, &p.args[1..]);
    }
    let text = p.args.join(" ");
    if text.is_empty() {
        return Err("usage".into());
    }
    let (dry, edit_only) = (p.is("dry-run"), p.is("edit"));
    let dir = std::env::current_dir().ok().map(|d| d.display().to_string());
    let mut c = tool(ctx)?;
    if dry {
        for line in c.plumb_dry(ExecCtx::Top, &text, dir, edit_only, TIMEOUT)? {
            println!("{line}");
        }
        return Ok(());
    }
    let before = c.node.state.windows.len();
    c.send(&ClientMsg::Plumb { ctx: ExecCtx::Top, text, dir, edit_only, dry: false, at: None, sel: None, alt: None, reverse: false, verb: None });
    let _ = wait(&mut c, |r| r.node.state.windows.len() > before);
    Ok(())
}

/// plan 9's `B`: each argument to the edit port, from this directory.
fn b(ctx: &Ctx, p: &Parsed) -> R {
    if p.args.is_empty() {
        return Err("usage".into());
    }
    let dir = std::env::current_dir().ok().map(|d| d.display().to_string());
    let mut c = tool(ctx)?;
    for a in &p.args {
        let before = c.node.state.windows.len();
        c.send(&ClientMsg::Plumb { ctx: ExecCtx::Top, text: a.clone(), dir: dir.clone(), edit_only: true, dry: false, at: None, sel: None, alt: None, reverse: false, verb: None });
        let _ = wait(&mut c, |r| r.node.state.windows.len() > before);
    }
    Ok(())
}

/// `apex editor FILE`: editinacme. The absolute path, "editing FILE" on
/// stderr, the file plumbed to edit, then wait for the window named
/// after it to go. The log is read from before the plumb, as
/// editinacme opens acme's log first, so a quick Del is not missed.
fn editor(ctx: &Ctx, p: &Parsed) -> R {
    let [file] = p.args.as_slice() else { return Err("usage".into()) };
    let file = std::path::absolute(file).map_err(|e| format!("{file}: {e}"))?.display().to_string();
    let mut c = tool(ctx)?;
    eprintln!("editor: editing {file}");
    c.send(&ClientMsg::Plumb { ctx: ExecCtx::Top, text: file.clone(), dir: None, edit_only: true, dry: false, at: None, sel: None, alt: None, reverse: false, verb: None });
    let open = |r: &Remote| r.node.state.windows.keys().any(|w| r.node.window_name(*w) == file);
    wait(&mut c, |r| open(r)).map_err(|_| format!("{file}: not opened"))?;
    loop {
        if !open(&c) {
            return Ok(());
        }
        match c.step(Duration::from_millis(100)) {
            Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => return Err("connection closed".into()),
        }
    }
}

/// `apex plumb rule add|rm|ls`: the session's rule table.
fn rule(ctx: &Ctx, args: &[String]) -> R {
    let mut c = tool(ctx)?;
    match args.first().map(|s| s.as_str()) {
        Some("add") => {
            let f = match parse(RULE_FLAGS, &args[1..]) {
                Ok(f) => f,
                Err(e) if e == "help" => {
                    eprintln!("usage: apex plumb rule add FLAGS\nRun 'apex help rules' for the flags.");
                    std::process::exit(0);
                }
                Err(e) => return Err(format!("rule add: {e}")),
            };
            if !f.args.is_empty() {
                return Err(format!("rule add: unexpected argument {:?}", f.args[0]));
            }
            let (rule, priority, mine) = rule_of(&f)?;
            let id = c.rule_add(rule, priority, mine, TIMEOUT)?;
            println!("{id}");
            Ok(())
        }
        Some("rm") => {
            for a in &args[1..] {
                let n: u64 = a.trim_start_matches('r').parse().map_err(|_| format!("rule rm: {a}: not a rule id"))?;
                c.send(&ClientMsg::RuleRm { id: RuleId(n) });
            }
            let _ = c.step(Duration::from_millis(50));
            Ok(())
        }
        Some("ls") | None => {
            let meta = &c.node.state.meta;
            for (id, r) in apex_core::plumb::ordered(&meta.rules) {
                let owner = if r.attachment == SERVER { "session".to_string() } else { meta.attachments.get(&r.attachment).map(|a| format!("{}({})", a.name, r.attachment)).unwrap_or_else(|| r.attachment.to_string()) };
                println!("{id}\t{owner}\tp{}\t{}", r.priority, r.rule.to_flags());
            }
            Ok(())
        }
        Some(_) => Err("usage".into()),
    }
}

/// The flags of `apex plumb rule add`, as a rule.
fn rule_of(f: &Parsed) -> Result<(PlumbRule, i32, bool), String> {
    let mut actions = Vec::new();
    if let Some(v) = f.get("edit") {
        actions.push(RuleAction::Edit(v.to_string()));
    }
    if let Some(v) = f.get("run") {
        actions.push(RuleAction::Run(v.to_string()));
    }
    if let Some(v) = f.get("tool") {
        actions.push(RuleAction::Tool(v.to_string()));
    }
    if let Some(v) = f.get("client") {
        actions.push(RuleAction::Client { verb: v.to_string(), args: f.get("args").unwrap_or("").to_string() });
    }
    let action = match actions.len() {
        1 => actions.remove(0),
        0 => return Err("rule add: one of -edit, -run, -client or -tool".into()),
        _ => return Err("rule add: only one of -edit, -run, -client and -tool".into()),
    };
    let kind = match f.get("kind") {
        Some(k) => Some(WinKind::parse(k).ok_or_else(|| format!("-kind={k}: file, dir, term or errors"))?),
        None => None,
    };
    let to = match f.get("to") {
        Some("errors") => Some(RunTo::Errors),
        Some("window") => Some(RunTo::Window),
        Some(t) => return Err(format!("-to={t}: errors or window")),
        None => None,
    };
    let priority = match f.get("priority") {
        Some(v) => v.parse().map_err(|_| format!("-priority={v}: not a number"))?,
        None => 0,
    };
    let r = PlumbRule {
        verb: f.get("verb").unwrap_or("plumb").to_string(),
        text: f.get("text").map(String::from),
        file: f.get("file").map(String::from),
        kind,
        isfile: f.get("isfile").map(String::from),
        isdir: f.get("isdir").map(String::from),
        action,
        to,
    };
    r.check()?;
    Ok((r, priority, f.is("mine")))
}

/// plan9port's `label`: name the window this terminal shows, through the
/// sequence acme's win reads (`ESC ] ; text BEL`).
fn label(text: &str) -> R {
    to_terminal(&format!("\x1b];{text}\x07"))
}

/// Write an escape sequence to the terminal this runs in.
fn to_terminal(seq: &str) -> R {
    match std::fs::OpenOptions::new().write(true).open("/dev/tty") {
        Ok(mut f) => f.write_all(seq.as_bytes()).map_err(|e| e.to_string())?,
        Err(_) => {
            let mut out = std::io::stdout();
            out.write_all(seq.as_bytes()).and_then(|_| out.flush()).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn label_cmd(_: &Ctx, p: &Parsed) -> R {
    if p.args.is_empty() {
        return Err("usage".into());
    }
    label(&p.args.join(" "))
}

/// plan9port's `awd [label]`: the window named `pwd/-label`, the label
/// being the host unless given. Under the terminals' rule that is the
/// directory reported (OSC 7) and the label as the title.
fn awd(_: &Ctx, p: &Parsed) -> R {
    let sys = match p.args.as_slice() {
        [] => apex_server::term::sysname(),
        [s] => s.clone(),
        _ => return Err("usage".into()),
    };
    let dir = std::env::current_dir().map_err(|e| e.to_string())?.display().to_string();
    let host = apex_server::term::sysname();
    to_terminal(&format!("\x1b]7;file://{host}{dir}\x07\x1b];{sys}\x07"))
}

/// The session's environment: what its terminals and commands get beyond
/// the daemon's own. `KEY=VALUE` sets; nothing prints it.
fn env_cmd(ctx: &Ctx, p: &Parsed) -> R {
    let mut c = tool(ctx)?;
    let set: Vec<(String, String)> = p
        .args
        .iter()
        .map(|a| a.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())).ok_or_else(|| format!("{a}: not KEY=VALUE")))
        .collect::<Result<_, _>>()?;
    let show = set.is_empty() && !p.is("import");
    let vars = if p.is("import") {
        if !set.is_empty() {
            return Err("-import takes no arguments".into());
        }
        let own = std::env::vars_os().map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned())).collect();
        c.env_import(own, TIMEOUT)?
    } else {
        c.env(set, TIMEOUT)?
    };
    if show {
        for (k, v) in vars {
            println!("{k}={v}");
        }
    }
    Ok(())
}

/// `apex set KEY VALUE`: a setting of the session's, or of the attaching
/// client's when run from its attach script (`$apexattachment`). `apex
/// set` alone lists them all.
fn set(ctx: &Ctx, p: &Parsed) -> R {
    let mut c = tool(ctx)?;
    match p.args.as_slice() {
        [] => {
            let meta = &c.node.state.meta;
            for (owner, map) in &meta.settings {
                let who = if *owner == SERVER { "session".to_string() } else { meta.attachments.get(owner).map(|a| format!("{}({owner})", a.name)).unwrap_or_else(|| owner.to_string()) };
                for (k, v) in map {
                    println!("{who}\t{k}\t{v}");
                }
            }
            Ok(())
        }
        [key, value] => {
            let attachment = std::env::var("apexattachment").ok().and_then(|a| a.parse::<u64>().ok()).map(AttachmentId);
            c.send(&ClientMsg::Set { key: key.clone(), value: value.clone(), attachment });
            let _ = c.step(Duration::from_millis(50));
            Ok(())
        }
        _ => Err("usage".into()),
    }
}

/// `apex web open URL`: a web window on URL; `apex web [-name NAME]`:
/// HTML on stdin shown as a page.
fn web(ctx: &Ctx, p: &Parsed) -> R {
    let mut c = tool(ctx)?;
    let col = c.node.state.layout.cols.first().map(|c| c.id).ok_or("no column")?;
    let before: Vec<WindowId> = c.node.state.windows.keys().copied().collect();
    match p.args.as_slice() {
        [open, url] if open == "open" => {
            c.propose(Proposal::OpenWeb { col, url: url.clone() }, TIMEOUT)?;
        }
        [] => {
            let mut text = String::new();
            if !std::io::stdin().is_terminal() {
                std::io::stdin().read_to_string(&mut text).map_err(|e| e.to_string())?;
            }
            let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
            let name = match p.get("name") {
                Some(n) if n.starts_with('/') => n.to_string(),
                Some(n) => cwd.join(n).display().to_string(),
                None => cwd.join("+web").display().to_string(),
            };
            c.propose(Proposal::OpenHtml { col, name, text }, TIMEOUT)?;
        }
        _ => return Err("usage".into()),
    }
    wait(&mut c, |r| r.node.state.windows.keys().any(|w| !before.contains(w)))?;
    let w = c.node.state.windows.keys().find(|w| !before.contains(w)).unwrap();
    println!("{}", w.0);
    Ok(())
}

/// `apex io [-watch] METHOD URL`: one request on the I/O plane.
fn io_cmd(ctx: &Ctx, p: &Parsed) -> R {
    let [method, url] = p.args.as_slice() else { return Err("usage".into()) };
    let mut c = tool(ctx)?;
    let watch = p.get("watch").is_some();
    let headers: Vec<(&str, &str)> = if watch { vec![("Watch", "1")] } else { Vec::new() };
    let stream = c.io_open(method, url, &headers);
    if method == "CONNECT" {
        // nc: stdin in, the far end out, until either side is done
        let status = c.io_response(stream, TIMEOUT)?;
        if status != 200 {
            let (_, body) = c.io_collect_body_pub(stream, TIMEOUT)?;
            std::io::stderr().write_all(&body).map_err(|e| e.to_string())?;
            return Err(format!("{status}"));
        }
        let out = c.outbound();
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            let mut stdin = std::io::stdin().lock();
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if out.send(&ClientMsg::Io { stream, frame: apex_server::proto::IoFrame::Body(buf[..n].to_vec()) }).is_err() {
                            return;
                        }
                    }
                }
            }
            let _ = out.send(&ClientMsg::Io { stream, frame: apex_server::proto::IoFrame::End });
        });
        loop {
            for f in c.io_take(stream) {
                match f {
                    apex_server::proto::IoFrame::Body(b) => {
                        std::io::stdout().write_all(&b).map_err(|e| e.to_string())?;
                        std::io::stdout().flush().map_err(|e| e.to_string())?;
                    }
                    apex_server::proto::IoFrame::End => return Ok(()),
                    apex_server::proto::IoFrame::Reset { reason } => return Err(reason),
                    _ => {}
                }
            }
            match c.step(Duration::from_millis(100)) {
                Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return Err("connection closed".into()),
            }
        }
    }
    let has_body = method == "PUT" || (!matches!(method.as_str(), "GET" | "HEAD" | "DELETE" | "OPTIONS") && !std::io::stdin().is_terminal());
    if has_body {
        let mut body = Vec::new();
        std::io::stdin().read_to_end(&mut body).map_err(|e| e.to_string())?;
        c.io_send(stream, &body);
        c.io_end(stream);
    } else if url.starts_with("http") {
        c.io_end(stream);
    }
    let status = c.io_response(stream, TIMEOUT)?;
    if !watch || status != 200 {
        let (_, body) = c.io_collect_body_pub(stream, TIMEOUT)?;
        std::io::stdout().write_all(&body).map_err(|e| e.to_string())?;
        if status != 200 {
            return Err(format!("{status}"));
        }
        return Ok(());
    }
    // a watch: each version's contents as it comes, until we are killed
    loop {
        while let Some(f) = c.io_next_file(stream) {
            std::io::stdout().write_all(&f.bytes).map_err(|e| e.to_string())?;
            std::io::stdout().flush().map_err(|e| e.to_string())?;
        }
        match c.step(Duration::from_millis(100)) {
            Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => return Err("connection closed".into()),
        }
        if c.io_take_ended(stream) {
            return Ok(());
        }
    }
}

/// The bytes of a file on the session's host, to stdout.
fn cat(ctx: &Ctx, p: &Parsed) -> R {
    let [path] = p.args.as_slice() else { return Err("usage".into()) };
    let mut c = tool(ctx)?;
    let bytes = c.read_file(path, TIMEOUT)?;
    std::io::stdout().write_all(&bytes).map_err(|e| e.to_string())?;
    Ok(())
}
