//! Domain types for Kookaburra.
//!
//! This crate contains no I/O, no rendering, and no async runtime. It is safe
//! to depend on from every other crate in the workspace.

pub mod action;
pub mod config;
pub mod ids;
pub mod layout;
pub mod snapshot;
pub mod state;
pub mod worktree;
