//! Terminal application runtime for Mandatum.
//!
//! This crate coordinates the current terminal frontend with runtime modules for
//! PTYs, tasks, process events, persistence, and input routing.

mod app_shell;
mod app_state;
mod clipboard;
mod copy_mode;
mod input;
mod persistence;
mod process_events;
mod task_runtime;
mod terminal_runtime;

pub use app_shell::{AppConfig, AppError, default_workspace_file, run, run_with_config};
pub use app_state::AppState;
pub use input::{
    RuntimeInput, key_to_input, key_to_input_with_palette_context, key_to_terminal_input,
};
