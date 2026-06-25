//! Placeholder terminal parser adapter boundary.
//!
//! Milestone 1 does not bind libghostty-vt or another parser. The first fake
//! parser and concrete adapter traits are deferred to Milestone 2.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalVtBoundary;
