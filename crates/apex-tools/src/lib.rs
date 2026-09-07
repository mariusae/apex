//! `apex tool NAME`: the tools that come with apex. None is privileged:
//! each attaches to a session as a tool, keeps a replica, reads the
//! entry stream and proposes, exactly as anything else on the CLI would.
//!
//! - `win`: acme's win, a shell in a text window (the editable transcript).
//! - `lsp`: language servers, with definitions, hover, formatting and
//!   diagnostics through the plumbing rules and the tools menu.

pub mod lsp;
pub mod win;
