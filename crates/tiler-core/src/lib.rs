//! Platform-independent core of the tiler.
//!
//! Nothing in this crate calls Win32. Window handles are an opaque generic `W`,
//! so every piece of logic here can be unit-tested on any OS.

pub mod command;
pub mod config;
pub mod geom;
pub mod hotkey;
pub mod model;
pub mod nav;
pub mod rules;
pub mod tree;

pub use command::Command;
pub use config::Config;
pub use geom::{Axis, Direction, Rect};
pub use model::{Added, InsertMode, Loc, Model, OverflowMode, StackStyle, Target};
pub use tree::{Gaps, Node};
