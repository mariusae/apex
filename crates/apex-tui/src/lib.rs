//! apex-tui: the view server behind the TermKit UI.
//!
//! It keeps the replica and the session's state exactly as the gpui
//! client does, lays the row out on a cell grid, and serves one JSON
//! view model per change (`model`), taking the pointer and the keyboard
//! back (`input`). The UI itself — every window, tag, terminal, markdown
//! page, page and finder — is the TermKit app in `swift/ApexTUI`.
//!
//! The split follows DESIGN.md §1: state on the server, rendering on the
//! client. What is here is not rendering; it is the part of the old
//! client that was never about gpui.

pub mod cells;
pub mod input;
pub mod model;
pub mod ui;
pub mod wire;
