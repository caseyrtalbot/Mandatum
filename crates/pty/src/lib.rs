//! Placeholder PTY boundary.
//!
//! Milestone 1 intentionally avoids process lifecycle, PTY handles, stream
//! backpressure, and child restart behavior. Those belong here in Milestone 2.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PtyBoundary;
