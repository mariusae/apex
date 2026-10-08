//! `apex tool agent`: what every agent in the session is
//! doing -- every Claude Code, Codex and Muse started in one of its
//! terminals -- fed by the hooks those agents offer, and for any of
//! them its transcript, its last answer as a page, its changes as a
//! diff.
//!
//!     apex tool agent [-cwd DIR] [-a] [-all] [-thoughts] [-quiet]
//!     apex tool agent install [claude|codex|muse]...  put the hooks in (all, by default)
//!     apex tool agent uninstall [claude|codex|muse]...
//!     apex tool agent ls                     every agent, as text
//!     apex tool agent wait ID                until the agent's turn ends; the exit status is its state
//!     apex tool agent events [-all]          the events as they come, a line each
//!     apex tool agent hook claude|codex|muse what the agents run; not for typing
//!
//! The hook is this same program: each event the agent has is one line
//! appended to `~/.apex/agent/SESSION.jsonl`, and the tool reads
//! those logs, so nothing need be running when an agent starts and
//! nothing is lost when it is not.
//!
//! It has no window of its own by default: its job is the terminals of
//! its own session that are running agents, and what it does with them
//! it does on their windows. While an agent runs, `Transcript`,
//! `Preview` and `Changes` are in its terminal's tools menu -- the
//! transcript, named for the agent's own directory and read from the
//! agent's own record as it grows; a page with the last exchange it
//! finished, written afresh as each turn ends; its repository's diff
//! since the session began -- and while it asks something, `Allow Deny
//! Ask` are there too, the answer going into the log for the hook that
//! asked to read. An agent that wants you -- its turn over and the next
//! prompt yours, a question to answer, a turn that failed -- raises a
//! notification on its terminal, so the session's square says someone
//! is waiting and a click takes you to them, one agent a click;
//! it goes as soon as the agent is back at work. An agent at work has
//! its terminal marked working, its handle turning -- or, when its plan
//! (Claude's and Muse's todos, Codex's plan) says how far along it is,
//! its handle's circle filled as far as the steps done.
//!
//! `-a` adds the overview window, `DIR/-agent`, a block an agent in
//! the order they want attention: `?` a permission or a question
//! waiting on you, `✗` a turn that failed, `~` a turn over and the next
//! prompt yours, `▶` at work. A question is answered where it stands,
//! `Allow Deny Ask`. B3 anywhere in a block (or `Open`) opens the
//! agent's transcript beside it; `Preview` its page; `Changes` its
//! diff; `Goto` goes to the agent itself, the window it was started in,
//! in whatever session that was; `Send TEXT` types into that window;
//! `CopyContext` in any text window copies its selection, or the current
//! line when there is none, headed by the file and line it came from;
//! `Start` and `Resume` make terminals running agents; `History` lists
//! the directory's past sessions.
//!
//! `-all` widens both from this session to every agent on the machine,
//! whichever terminal or editor it was started from. `ls`, `wait` and
//! `events` are the same logs as text, for scripts.
//!
//! The windows are [`win`]'s, the command line [`cmd`]'s; the rest is
//! what feeds them.

pub mod agent;
pub mod cmd;
pub mod event;
pub mod history;
pub mod hook;
pub mod install;
pub mod page;
pub mod transcript;
pub mod vcs;
pub mod win;
