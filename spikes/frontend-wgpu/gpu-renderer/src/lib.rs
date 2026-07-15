//! Structurally isolated GPU renderer for the deferred frontend spike.
//!
//! This crate depends on the neutral scene contract and paint/window crates
//! only. PTY and terminal-parser crates cannot enter its dependency closure.

include!("../../src/gpu.rs");
