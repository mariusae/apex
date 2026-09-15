//! apex-agent as a library, so that its windows can be driven from a
//! test: what the binary does is in [`win`], the rest is what feeds it.
//! See `src/main.rs` for the account of it.

pub mod agents;
pub mod event;
pub mod hook;
pub mod install;
pub mod page;
pub mod transcript;
pub mod win;
