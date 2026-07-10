//! Config file loading and validation.
//!
//! Two TOML files overlay defaults, user first, then project (project
//! wins): `$XDG_CONFIG_HOME/mandatum/config.toml` (default
//! `~/.config/mandatum/config.toml`) and `<project>/.mandatum/config.toml`.
//!
//! Validation happens here at the boundary: unknown keys, bad chords and
//! bad colors each produce a warning naming the exact problem, and the
//! affected setting keeps its default. A broken config never prevents
//! launch and never panics.

use std::path::{Path, PathBuf};

use mandatum_commands::{CommandId, command_id_for_name};
use mandatum_scene::{SceneColor, Theme};

use crate::app_shell::AgentConnectorKind;
use crate::keymap::{ChordAction, Keymap, format_chord, parse_chord};

/// Everything the config files can influence, resolved against defaults.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadedConfig {
    pub keymap: Keymap,
    pub theme: Theme,
    pub reduced_motion: bool,
    /// Surface byte-level runtime diagnostics ("read N byte(s)") in the
    /// status line. Off by default: they are debugging noise that would
    /// overwrite meaningful status on every PTY read.
    pub debug_status: bool,
    pub shell_program: Option<String>,
    pub task_command: Option<String>,
    pub agent_connector: Option<AgentConnectorKind>,
    pub agent_model: Option<String>,
    pub warnings: Vec<String>,
}

/// The user-level config file honoring `XDG_CONFIG_HOME`, when a home
/// directory can be resolved at all.
pub fn user_config_file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("mandatum").join("config.toml"))
}

/// The project-level config file next to the workspace file.
pub fn project_config_file(project_path: &Path) -> PathBuf {
    project_path.join(".mandatum").join("config.toml")
}

/// Load defaults, then the user file, then the project file (project wins).
/// Missing files are fine; broken files degrade to warnings.
pub fn load_config(user_file: Option<&Path>, project_file: &Path) -> LoadedConfig {
    let mut config = LoadedConfig::default();
    if let Some(user_file) = user_file {
        apply_file(&mut config, user_file, "user config");
    }
    apply_file(&mut config, project_file, "project config");
    config
}

fn apply_file(config: &mut LoadedConfig, path: &Path, label: &str) {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            config
                .warnings
                .push(format!("{label} ({}): unreadable: {error}", path.display()));
            return;
        }
    };

    let table = match text.parse::<toml::Table>() {
        Ok(table) => table,
        Err(error) => {
            config.warnings.push(format!(
                "{label} ({}): not valid TOML, file ignored: {error}",
                path.display()
            ));
            return;
        }
    };

    for (section, value) in table {
        match (section.as_str(), value) {
            ("keymap", toml::Value::Table(keymap)) => apply_keymap(config, keymap, label),
            ("theme", toml::Value::Table(theme)) => apply_theme(config, theme, label),
            ("ui", toml::Value::Table(ui)) => apply_ui(config, ui, label),
            ("shell", toml::Value::Table(shell)) => apply_shell(config, shell, label),
            ("task", toml::Value::Table(task)) => apply_task(config, task, label),
            ("agent", toml::Value::Table(agent)) => apply_agent(config, agent, label),
            ("keymap" | "theme" | "ui" | "shell" | "task" | "agent", _) => {
                config
                    .warnings
                    .push(format!("{label}: [{section}] must be a table"));
            }
            (unknown, _) => {
                config
                    .warnings
                    .push(format!("{label}: unknown config section [{unknown}]"));
            }
        }
    }
}

fn apply_keymap(config: &mut LoadedConfig, table: toml::Table, label: &str) {
    // Which reserved chord this file set last (document order), so a
    // quit/toggle-palette collision can revert the later binding.
    let mut last_reserved_set = None;
    for (key, value) in table {
        match key.as_str() {
            "palette" => match value {
                toml::Value::Table(palette) => apply_palette_letters(config, palette, label),
                _ => config
                    .warnings
                    .push(format!("{label}: [keymap.palette] must be a table")),
            },
            "quit" => {
                if let Some(chord) = expect_chord(config, &key, value, label) {
                    config.keymap.quit = chord;
                    last_reserved_set = Some("quit");
                }
            }
            "toggle-palette" => {
                if let Some(chord) = expect_chord(config, &key, value, label) {
                    config.keymap.toggle_palette = chord;
                    last_reserved_set = Some("toggle-palette");
                }
            }
            name => match command_id_for_name(name) {
                Some(command_id) => {
                    if let Some(chord) = expect_chord(config, name, value, label) {
                        bind_command_chord(config, command_id, chord, name, label);
                    }
                }
                None => config
                    .warnings
                    .push(format!("{label}: unknown command '{name}' in [keymap]")),
            },
        }
    }
    resolve_reserved_chord_collision(config, last_reserved_set, label);
}

/// If quit and toggle-palette resolve to the same chord, quit always wins
/// and the palette becomes unreachable from the keyboard — a silent lockout
/// of the primary control surface. Revert the later-set binding to its
/// default (both if the default still collides) and warn.
fn resolve_reserved_chord_collision(
    config: &mut LoadedConfig,
    last_reserved_set: Option<&str>,
    label: &str,
) {
    let collides = |keymap: &Keymap| {
        matches!(
            keymap.chord_action(keymap.toggle_palette),
            Some(ChordAction::Quit)
        )
    };
    if !collides(&config.keymap) {
        return;
    }

    let shared = format_chord(config.keymap.toggle_palette);
    let defaults = Keymap::default();
    let reverted = if last_reserved_set == Some("toggle-palette") {
        config.keymap.toggle_palette = defaults.toggle_palette;
        "toggle-palette"
    } else {
        config.keymap.quit = defaults.quit;
        "quit"
    };
    if collides(&config.keymap) {
        config.keymap.quit = defaults.quit;
        config.keymap.toggle_palette = defaults.toggle_palette;
    }
    config.warnings.push(format!(
        "{label}: quit and toggle-palette both resolve to '{shared}' (quit would win and the \
         palette would be unreachable); {reverted} reverted to its default '{}'",
        if reverted == "quit" {
            format_chord(config.keymap.quit)
        } else {
            format_chord(config.keymap.toggle_palette)
        }
    ));
}

fn bind_command_chord(
    config: &mut LoadedConfig,
    command_id: CommandId,
    chord: mandatum_scene::input::Key,
    name: &str,
    label: &str,
) {
    for (reserved, reserved_name) in [
        (config.keymap.quit, "quit"),
        (config.keymap.toggle_palette, "toggle-palette"),
    ] {
        if reserved == chord {
            config.warnings.push(format!(
                "{label}: chord '{}' for {name} is taken by {reserved_name}, which wins",
                format_chord(chord)
            ));
        }
    }
    if let Some(displaced) = config.keymap.bind_chord(command_id, chord) {
        config.warnings.push(format!(
            "{label}: chord '{}' moved to {name} (was {}); later binding wins",
            format_chord(chord),
            command_name(displaced),
        ));
    }
}

fn apply_palette_letters(config: &mut LoadedConfig, table: toml::Table, label: &str) {
    for (name, value) in table {
        let Some(command_id) = command_id_for_name(&name) else {
            config.warnings.push(format!(
                "{label}: unknown command '{name}' in [keymap.palette]"
            ));
            continue;
        };
        let Some(text) = expect_string(config, &format!("keymap.palette.{name}"), value, label)
        else {
            continue;
        };
        let mut characters = text.chars();
        match (characters.next(), characters.next()) {
            (Some(letter), None) => {
                if let Some(displaced) = config.keymap.palette.rebind(command_id, letter) {
                    config.warnings.push(format!(
                        "{label}: palette key '{letter}' moved to {name} (was {}); later \
                         binding wins",
                        command_name(displaced),
                    ));
                }
            }
            _ => config.warnings.push(format!(
                "{label}: palette key for {name} must be one character, got '{text}'"
            )),
        }
    }
}

fn apply_theme(config: &mut LoadedConfig, mut table: toml::Table, label: &str) {
    // The base theme applies first so color overrides layer on top of it
    // regardless of key order in the file.
    if let Some(value) = table.remove("name")
        && let Some(name) = expect_string(config, "theme.name", value, label)
    {
        match Theme::builtin(&name) {
            Some(theme) => config.theme = theme,
            None => config.warnings.push(format!(
                "{label}: unknown theme '{name}' (built-ins: {})",
                Theme::BUILTIN_NAMES.join(", ")
            )),
        }
    }

    for (key, value) in table {
        if theme_slot(&mut config.theme, &key).is_none() {
            config
                .warnings
                .push(format!("{label}: unknown key 'theme.{key}'"));
            continue;
        }
        let target = format!("theme.{key}");
        let Some(text) = expect_string(config, &target, value, label) else {
            continue;
        };
        match parse_color(&text) {
            Ok(color) => {
                if let Some(slot) = theme_slot(&mut config.theme, &key) {
                    *slot = color;
                }
            }
            Err(problem) => config
                .warnings
                .push(format!("{label}: {target}: {problem}")),
        }
    }
}

fn theme_slot<'a>(theme: &'a mut Theme, key: &str) -> Option<&'a mut SceneColor> {
    Some(match key {
        "focus_border" => &mut theme.focus_border,
        "pane_border" => &mut theme.pane_border,
        "pane_title" => &mut theme.pane_title,
        "header" => &mut theme.header,
        "header_background" => &mut theme.header_background,
        "status" => &mut theme.status,
        "attention" => &mut theme.attention,
        "palette_border" => &mut theme.palette_border,
        "palette_selection" => &mut theme.palette_selection,
        "selection_highlight" => &mut theme.selection_highlight,
        "agent_running" => &mut theme.agent_running,
        "agent_waiting" => &mut theme.agent_waiting,
        "agent_failed" => &mut theme.agent_failed,
        "agent_complete" => &mut theme.agent_complete,
        "agent_idle" => &mut theme.agent_idle,
        _ => return None,
    })
}

fn apply_ui(config: &mut LoadedConfig, table: toml::Table, label: &str) {
    for (key, value) in table {
        match (key.as_str(), value) {
            ("reduced_motion", toml::Value::Boolean(flag)) => config.reduced_motion = flag,
            ("reduced_motion", other) => config.warnings.push(format!(
                "{label}: ui.reduced_motion must be true or false, got {other}"
            )),
            ("debug_status", toml::Value::Boolean(flag)) => config.debug_status = flag,
            ("debug_status", other) => config.warnings.push(format!(
                "{label}: ui.debug_status must be true or false, got {other}"
            )),
            (unknown, _) => config
                .warnings
                .push(format!("{label}: unknown key 'ui.{unknown}'")),
        }
    }
}

fn apply_shell(config: &mut LoadedConfig, table: toml::Table, label: &str) {
    for (key, value) in table {
        match key.as_str() {
            "program" => {
                config.shell_program = expect_string(config, "shell.program", value, label)
                    .or(config.shell_program.take());
            }
            unknown => config
                .warnings
                .push(format!("{label}: unknown key 'shell.{unknown}'")),
        }
    }
}

fn apply_task(config: &mut LoadedConfig, table: toml::Table, label: &str) {
    for (key, value) in table {
        match key.as_str() {
            "default_command" => {
                config.task_command = expect_string(config, "task.default_command", value, label)
                    .or(config.task_command.take());
            }
            unknown => config
                .warnings
                .push(format!("{label}: unknown key 'task.{unknown}'")),
        }
    }
}

fn apply_agent(config: &mut LoadedConfig, table: toml::Table, label: &str) {
    for (key, value) in table {
        match key.as_str() {
            "connector" => {
                let Some(text) = expect_string(config, "agent.connector", value, label) else {
                    continue;
                };
                match text.as_str() {
                    "claude" => config.agent_connector = Some(AgentConnectorKind::Claude),
                    "fake" => config.agent_connector = Some(AgentConnectorKind::Fake),
                    other => config.warnings.push(format!(
                        "{label}: agent.connector must be 'claude' or 'fake', got '{other}'"
                    )),
                }
            }
            "model" => {
                config.agent_model = expect_string(config, "agent.model", value, label)
                    .or(config.agent_model.take());
            }
            unknown => config
                .warnings
                .push(format!("{label}: unknown key 'agent.{unknown}'")),
        }
    }
}

fn expect_string(
    config: &mut LoadedConfig,
    target: &str,
    value: toml::Value,
    label: &str,
) -> Option<String> {
    match value {
        toml::Value::String(text) => Some(text),
        other => {
            config
                .warnings
                .push(format!("{label}: {target} must be a string, got {other}"));
            None
        }
    }
}

fn expect_chord(
    config: &mut LoadedConfig,
    target: &str,
    value: toml::Value,
    label: &str,
) -> Option<mandatum_scene::input::Key> {
    let text = expect_string(config, target, value, label)?;
    match parse_chord(&text) {
        Ok(chord) => Some(chord),
        Err(problem) => {
            config
                .warnings
                .push(format!("{label}: {target}: {problem}"));
            None
        }
    }
}

fn command_name(command_id: CommandId) -> &'static str {
    mandatum_commands::command_for_id(command_id)
        .map(|command| command.name)
        .unwrap_or("unknown-command")
}

/// Parse `#rrggbb`, `default`, or a named ANSI color.
fn parse_color(text: &str) -> Result<SceneColor, String> {
    if let Some(hex) = text.strip_prefix('#') {
        if hex.len() == 6
            && let Ok(value) = u32::from_str_radix(hex, 16)
        {
            return Ok(SceneColor::Rgb(
                (value >> 16) as u8,
                (value >> 8) as u8,
                value as u8,
            ));
        }
        return Err(format!("'{text}' is not a #rrggbb color"));
    }
    let index = match text.to_ascii_lowercase().as_str() {
        "default" => return Ok(SceneColor::Default),
        "black" => 0,
        "red" => 1,
        "green" => 2,
        "yellow" => 3,
        "blue" => 4,
        "magenta" => 5,
        "cyan" => 6,
        "gray" | "grey" => 7,
        "dark-gray" | "dark-grey" => 8,
        "bright-red" => 9,
        "bright-green" => 10,
        "bright-yellow" => 11,
        "bright-blue" => 12,
        "bright-magenta" => 13,
        "bright-cyan" => 14,
        "white" => 15,
        _ => {
            return Err(format!(
                "unknown color '{text}' (use #rrggbb, 'default', or a named ANSI color)"
            ));
        }
    };
    Ok(SceneColor::Ansi(index))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use mandatum_scene::input::{Key, KeyCode, Modifiers};

    use super::*;
    use crate::keymap::ChordAction;

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(1);

    struct TestConfigDir {
        path: PathBuf,
    }

    impl TestConfigDir {
        fn new() -> Self {
            let counter = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "mandatum-config-test-{}-{counter}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("test temp dir should be created");
            Self { path }
        }

        fn write(&self, name: &str, contents: &str) -> PathBuf {
            let file = self.path.join(name);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(&file, contents).unwrap();
            file
        }

        fn missing(&self, name: &str) -> PathBuf {
            self.path.join(name)
        }
    }

    impl Drop for TestConfigDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn missing_files_load_pure_defaults_without_warnings() {
        let dir = TestConfigDir::new();
        let config = load_config(
            Some(&dir.missing("user.toml")),
            &dir.missing("project.toml"),
        );
        assert_eq!(config, LoadedConfig::default());
    }

    #[test]
    fn garbage_config_falls_back_to_defaults_with_a_warning() {
        let dir = TestConfigDir::new();
        let user = dir.write("user.toml", "this is {{{ not toml");
        let config = load_config(Some(&user), &dir.missing("project.toml"));

        assert_eq!(config.keymap, Keymap::default());
        assert_eq!(config.theme, Theme::default());
        assert_eq!(config.warnings.len(), 1);
        assert!(config.warnings[0].contains("not valid TOML"));
        assert!(config.warnings[0].contains("user config"));
    }

    #[test]
    fn full_config_parses_into_every_section() {
        let dir = TestConfigDir::new();
        let user = dir.write(
            "user.toml",
            r##"
[keymap]
quit = "ctrl+shift+q"
split-right = "ctrl+shift+r"

[keymap.palette]
split-right = "i"

[theme]
name = "mandatum-light"
focus_border = "#ff8800"
attention = "bright-yellow"

[ui]
reduced_motion = true
debug_status = true

[shell]
program = "/bin/zsh"

[task]
default_command = "cargo check"

[agent]
connector = "fake"
model = "claude-opus-4-6"
"##,
        );
        let config = load_config(Some(&user), &dir.missing("project.toml"));

        assert_eq!(config.warnings, Vec::<String>::new());
        assert_eq!(
            config.keymap.quit,
            Key::new(
                KeyCode::Char('q'),
                Modifiers {
                    control: true,
                    shift: true,
                    ..Modifiers::NONE
                }
            )
        );
        assert_eq!(
            config.keymap.chord_for(CommandId::SplitRight),
            Some(parse_chord("ctrl+shift+r").unwrap())
        );
        assert_eq!(
            config.keymap.palette.resolve_char('i'),
            Some(CommandId::SplitRight)
        );
        // The old default letter was released by the rebind.
        assert_eq!(config.keymap.palette.resolve_char('v'), None);
        assert_eq!(config.theme.name, "mandatum-light");
        assert_eq!(config.theme.focus_border, SceneColor::Rgb(0xff, 0x88, 0x00));
        assert_eq!(config.theme.attention, SceneColor::Ansi(11));
        assert!(config.reduced_motion);
        assert!(config.debug_status);
        assert_eq!(config.shell_program.as_deref(), Some("/bin/zsh"));
        assert_eq!(config.task_command.as_deref(), Some("cargo check"));
        assert_eq!(config.agent_connector, Some(AgentConnectorKind::Fake));
        assert_eq!(config.agent_model.as_deref(), Some("claude-opus-4-6"));
    }

    #[test]
    fn project_config_overlays_user_config() {
        let dir = TestConfigDir::new();
        let user = dir.write(
            "user.toml",
            "[task]\ndefault_command = \"user-task\"\n[ui]\nreduced_motion = true\n",
        );
        let project = dir.write(
            "project.toml",
            "[task]\ndefault_command = \"project-task\"\n",
        );
        let config = load_config(Some(&user), &project);

        assert_eq!(config.task_command.as_deref(), Some("project-task"));
        // Settings the project file does not touch keep the user value.
        assert!(config.reduced_motion);
        assert!(config.warnings.is_empty());
    }

    #[test]
    fn bad_values_warn_with_the_exact_problem_and_keep_defaults() {
        let dir = TestConfigDir::new();
        let user = dir.write(
            "user.toml",
            r##"
[keymap]
split-right = "banana+r"
close-pane = "x"
fly-mode = "ctrl+f"

[theme]
name = "solarized"
focus_border = "#zzz"

[unknown-section]
x = 1
"##,
        );
        let config = load_config(Some(&user), &dir.missing("project.toml"));

        let joined = config.warnings.join("\n");
        assert!(joined.contains("banana"), "warnings: {joined}");
        assert!(
            joined.contains("close-pane") && joined.contains("modifier"),
            "warnings: {joined}"
        );
        assert!(
            joined.contains("unknown command 'fly-mode'"),
            "warnings: {joined}"
        );
        assert!(
            joined.contains("unknown theme 'solarized'"),
            "warnings: {joined}"
        );
        assert!(joined.contains("#zzz"), "warnings: {joined}");
        assert!(
            joined.contains("unknown config section [unknown-section]"),
            "warnings: {joined}"
        );
        // Every failed setting keeps its default.
        assert_eq!(config.keymap, Keymap::default());
        assert_eq!(config.theme, Theme::default());
    }

    #[test]
    fn conflicting_bindings_let_the_later_one_win_with_a_warning() {
        let dir = TestConfigDir::new();
        let user = dir.write(
            "user.toml",
            r##"
[keymap]
split-right = "ctrl+r"
split-down = "ctrl+r"

[keymap.palette]
zoom-pane = "v"
"##,
        );
        let config = load_config(Some(&user), &dir.missing("project.toml"));

        assert_eq!(
            config.keymap.chord_action(Key::ctrl('r')),
            Some(ChordAction::Dispatch(CommandId::SplitDown))
        );
        assert_eq!(config.keymap.chord_for(CommandId::SplitRight), None);
        assert_eq!(
            config.keymap.palette.resolve_char('v'),
            Some(CommandId::ZoomPane)
        );
        let joined = config.warnings.join("\n");
        assert!(joined.contains("later binding wins"), "warnings: {joined}");
        assert!(
            joined.contains("palette key 'v' moved to zoom-pane"),
            "warnings: {joined}"
        );
    }

    #[test]
    fn quit_and_toggle_palette_sharing_a_chord_warns_and_keeps_both_reachable() {
        // Both bound to the same chord: the later binding reverts.
        let dir = TestConfigDir::new();
        let user = dir.write(
            "user.toml",
            "[keymap]\nquit = \"ctrl+g\"\ntoggle-palette = \"ctrl+g\"\n",
        );
        let config = load_config(Some(&user), &dir.missing("project.toml"));

        assert_eq!(config.keymap.quit, Key::ctrl('g'));
        assert_eq!(config.keymap.toggle_palette, Key::ctrl('p'));
        let joined = config.warnings.join("\n");
        assert!(
            joined.contains("quit and toggle-palette both resolve to 'ctrl+g'"),
            "warnings: {joined}"
        );
        assert!(joined.contains("unreachable"), "warnings: {joined}");

        // Rebinding quit onto the palette's default chord collides too.
        let user = dir.write("quit-on-p.toml", "[keymap]\nquit = \"ctrl+p\"\n");
        let config = load_config(Some(&user), &dir.missing("project.toml"));
        assert_eq!(config.keymap, Keymap::default());
        assert!(!config.warnings.is_empty());

        // Distinct chords never warn.
        let user = dir.write(
            "distinct.toml",
            "[keymap]\nquit = \"ctrl+g\"\ntoggle-palette = \"ctrl+space\"\n",
        );
        let config = load_config(Some(&user), &dir.missing("project.toml"));
        assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    }

    #[test]
    fn colors_parse_hex_named_and_default_forms() {
        assert_eq!(
            parse_color("#102030"),
            Ok(SceneColor::Rgb(0x10, 0x20, 0x30))
        );
        assert_eq!(parse_color("bright-blue"), Ok(SceneColor::Ansi(12)));
        assert_eq!(parse_color("default"), Ok(SceneColor::Default));
        assert!(parse_color("chartreuse").is_err());
        assert!(parse_color("#12345").is_err());
    }
}
