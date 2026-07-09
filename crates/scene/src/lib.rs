//! Mandatum's frontend contract.
//!
//! This crate defines the renderer-neutral scene model every frontend
//! consumes and the neutral input events every frontend emits. Product
//! behavior lives behind this boundary; frontends translate scenes into
//! pixels or cells and translate platform events into [`input`] values.
//!
//! No frontend, parser, process, or async-runtime type may appear here
//! (Constitution L1/L2/L4; enforced by `ci/conformance.sh`).
