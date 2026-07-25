//! Scene-only native GPU renderer for Mandatum.
//!
//! This crate depends on the neutral scene contract and paint/window crates
//! only. PTY and terminal-parser crates cannot enter its dependency closure.

mod font;
mod motion;
mod presentation_plan;
pub mod row_run;
mod shaping_cache;
mod text_metrics;

pub use font::{
    BUNDLED_FAMILY, DEFAULT_FONT_SIZE, FallbackRecord, FallbackReport, FontFacesInfo, FontInfo,
    FontProfileSource, FontProvisionError, FontRequest, ResolvedFontProfile, SelectedFontFaces,
};
pub use motion::{ActiveTransitionWindow, PresentationMotion};
pub use presentation_plan::{
    MAX_NATIVE_PLAN_COMMANDS, MAX_NATIVE_PLAN_NODES, MAX_NATIVE_PLAN_TEXT_SCOPES,
    MAX_NATIVE_PLAN_TRANSITIONS, NativeBoundary, NativeMaterial, NativeMaterialRole,
    NativePlanCommand, NativePresentationPlan, NativePresentationPlanError, NativeTextScope,
    NativeTokenColorRole, NativeTokenSamplerPlan, NativeTokenSwatch, NativeTransition,
    prepare_native_presentation, prepare_token_sampler,
};
pub use text_metrics::{
    NativeFontFace, NativeTextMetricError, NativeTextMetricIdentity, NativeTextMetricRole,
    NativeTextMetricSet, NativeTextMetrics, NativeTextPlacement, NativeTextStyle,
};

include!("gpu.rs");
