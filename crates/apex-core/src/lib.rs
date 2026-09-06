//! apex-core: the editor's replicated state machine. Headless: no UI, no
//! I/O. See ../../DESIGN.md.
//!
//! - [`ids`], [`entry`]: shards and the entries of their logs.
//! - [`text`], [`buffer`]: rune-indexed text, views, undo.
//! - [`state`]: the session state and `apply`, deterministic and pure.
//! - [`log`]: the in-memory log store with leases and fencing.
//! - [`node`]: a replica that leads shards: typing, commands, Edit lowering.

pub mod buffer;
pub mod entry;
pub mod ids;
pub mod log;
pub mod node;
pub mod plumb;
pub mod state;
pub mod text;
pub mod tiling;

pub use entry::*;
pub use ids::*;
pub use log::{Log, LogError};
pub use node::{CoreError, EditRun, Executed, Node};
pub use state::{Applied, ApplyError, State};
pub use text::Text;
pub use tiling::{Rect, Warp};
