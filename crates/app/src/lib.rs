//! Terminal application runtime for Mandatum.
//!
//! This crate coordinates the current terminal frontend with runtime modules for
//! PTYs, tasks, process events, persistence, and input routing.

mod agent_runtime;
mod app_shell;
mod app_state;
mod attention;
mod clipboard;
mod config;
mod copy_mode;
mod events;
mod frontend;
mod help;
mod input;
mod keymap;
mod palette;
mod persistence;
mod pointer;
mod process_events;
mod scene_builder;
mod search;
mod session_map;
mod task_runtime;
mod terminal_runtime;
mod timeline;
mod timeline_view;

pub use app_shell::{
    AgentConnectorKind, AppConfig, AppError, default_workspace_file, run, run_with_config,
};
pub use app_state::AppState;
pub use config::{LoadedConfig, load_config, project_config_file, user_config_file};
pub use input::{RuntimeInput, key_to_input, key_to_input_with_keymap, key_to_terminal_input};
pub use keymap::{ChordAction, Keymap, format_chord, parse_chord};
pub use scene_builder::build_workspace_scene;
