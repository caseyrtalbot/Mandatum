//! Scene-only native GPU renderer for Mandatum.
//!
//! This crate depends on the neutral scene contract and paint/window crates
//! only. PTY and terminal-parser crates cannot enter its dependency closure.

include!("gpu.rs");
