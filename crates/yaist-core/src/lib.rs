//! YAiST core: the action model every client shares.
//!
//! An action is defined once ([`ActionSpec`]) and appears three ways: as an
//! LLM tool, as a panel button with a generated form, and as a `/command`.

mod action;
pub mod ask;
pub mod command;
mod context;
pub mod date;
pub mod log;
mod output;
mod registry;
pub mod validate;

pub use action::{ActionSpec, Args, Param, ParamKind, Risk, Validator};
pub use context::{last_lines, logged, shell_quote, total_memory_kib, Context, Distro, TtyProcess};
pub use output::{Column, Output, Table};
pub use registry::{BoxFuture, ModuleSpec, Registry, ShellFn};
