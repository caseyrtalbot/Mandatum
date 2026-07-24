//! Scene-only native GPU renderer for Mandatum.
//!
//! This crate depends on the neutral scene contract and paint/window crates
//! only. PTY and terminal-parser crates cannot enter its dependency closure.

mod font;
pub mod row_run;

pub use font::{
    BUNDLED_FAMILY, DEFAULT_FONT_SIZE, FallbackRecord, FallbackReport, FontFacesInfo, FontInfo,
    FontProfileSource, FontProvisionError, FontRequest, ResolvedFontProfile, SelectedFontFaces,
};

include!("gpu.rs");
