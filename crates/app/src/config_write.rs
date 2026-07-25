//! The write half of the config boundary: appearance edits back to disk.
//!
//! This is the codebase's only production config writer. It edits exactly
//! the keys the appearance overlay owns and nothing else: the user's
//! comments, formatting, and unrelated settings survive untouched
//! (toml_edit round-trips the document). Writes are atomic — a temp file in
//! the target directory replaces the config by rename — so a crash can
//! never leave a half-written config. Failures return a message for the
//! status line; they never panic and never block the running session.

use std::path::Path;

use toml_edit::{DocumentMut, Item, Table, value};

/// The header written above a config file this module has to create.
const CREATED_HEADER: &str = "# Mandatum user configuration.\n\
     # Managed keys under [theme] and [font] are updated in place by the\n\
     # in-app Appearance overlay; everything else is yours.\n";

/// The appearance values worth persisting. `None` leaves the key as the
/// file has it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct AppearanceUpdate {
    pub(crate) theme_name: Option<String>,
    pub(crate) terminal_background: Option<[u8; 3]>,
    pub(crate) font_family: Option<String>,
    pub(crate) font_size: Option<f32>,
}

/// Apply `update` to the user config file at `path`, creating the file (and
/// its directory) when missing.
pub(crate) fn write_appearance_update(
    path: &Path,
    update: &AppearanceUpdate,
) -> Result<(), String> {
    let (existing, created) = match std::fs::read_to_string(path) {
        Ok(text) => (text, false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (String::new(), true),
        Err(error) => return Err(format!("config not saved: {error}")),
    };
    let mut document = existing
        .parse::<DocumentMut>()
        .map_err(|error| format!("config not saved: existing file is not valid TOML: {error}"))?;

    if let Some(name) = &update.theme_name {
        section(&mut document, "theme")["name"] = value(name.as_str());
    }
    if let Some(rgb) = update.terminal_background {
        let hex = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
        let theme = section(&mut document, "theme");
        if !theme.contains_table("terminal") {
            theme["terminal"] = Item::Table(Table::new());
        }
        theme["terminal"]["background"] = value(hex);
    }
    if let Some(family) = &update.font_family {
        section(&mut document, "font")["family"] = value(family.as_str());
    }
    if let Some(size) = update.font_size {
        section(&mut document, "font")["size"] = value(f64::from(size));
    }

    let rendered = if created {
        format!("{CREATED_HEADER}\n{document}")
    } else {
        document.to_string()
    };
    write_atomically(path, rendered.as_bytes())
}

/// The named top-level table, created empty when absent. A same-named
/// non-table value is replaced — the loader already rejects that shape with
/// a "must be a table" warning, so nothing meaningful is lost.
fn section<'a>(document: &'a mut DocumentMut, name: &str) -> &'a mut Table {
    if !document.contains_table(name) {
        document[name] = Item::Table(Table::new());
    }
    document[name]
        .as_table_mut()
        .expect("the section was just ensured to be a table")
}

/// Temp-then-rename in the target directory, mirroring the workspace
/// persistence contract: readers only ever observe a complete file.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let directory = path
        .parent()
        .ok_or_else(|| "config not saved: config path has no parent directory".to_owned())?;
    std::fs::create_dir_all(directory).map_err(|error| format!("config not saved: {error}"))?;
    let temp = directory.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("config.toml"),
        std::process::id()
    ));
    std::fs::write(&temp, bytes).map_err(|error| format!("config not saved: {error}"))?;
    std::fs::rename(&temp, path).map_err(|error| {
        let _ = std::fs::remove_file(&temp);
        format!("config not saved: {error}")
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::config::load_config;

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(1);

    struct TempConfig {
        directory: PathBuf,
    }

    impl TempConfig {
        fn new() -> Self {
            let counter = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
            let directory = std::env::temp_dir().join(format!(
                "mandatum-config-write-test-{}-{counter}",
                std::process::id()
            ));
            std::fs::create_dir_all(&directory).expect("test dir");
            Self { directory }
        }

        fn path(&self) -> PathBuf {
            self.directory.join("config.toml")
        }
    }

    impl Drop for TempConfig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    fn update_all() -> AppearanceUpdate {
        AppearanceUpdate {
            theme_name: Some("mandatum-light".to_owned()),
            terminal_background: Some([16, 32, 48]),
            font_family: None,
            font_size: None,
        }
    }

    #[test]
    fn creates_the_file_and_the_loaded_config_round_trips_the_values() {
        let temp = TempConfig::new();
        write_appearance_update(&temp.path(), &update_all()).expect("write succeeds");

        let text = std::fs::read_to_string(temp.path()).unwrap();
        assert!(text.starts_with("# Mandatum user configuration."), "{text}");

        let loaded = load_config(Some(&temp.path()), &temp.directory.join("missing.toml"));
        assert_eq!(loaded.theme.name, "mandatum-light");
        assert_eq!(loaded.theme.terminal_palette.background, [16, 32, 48]);
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    }

    #[test]
    fn preserves_unrelated_keys_comments_and_updates_managed_keys_in_place() {
        let temp = TempConfig::new();
        std::fs::write(
            temp.path(),
            r##"# my precious comment
[keymap]
quit = "ctrl+shift+q" # inline note

[theme]
name = "mandatum-dark"
attention = "bright-yellow"

[theme.terminal]
background = "#000000"
red = "#aa0000"
"##,
        )
        .unwrap();

        write_appearance_update(&temp.path(), &update_all()).expect("write succeeds");
        let text = std::fs::read_to_string(temp.path()).unwrap();

        assert!(text.contains("# my precious comment"), "{text}");
        assert!(
            text.contains("quit = \"ctrl+shift+q\" # inline note"),
            "{text}"
        );
        assert!(text.contains("attention = \"bright-yellow\""), "{text}");
        assert!(text.contains("red = \"#aa0000\""), "{text}");
        assert!(text.contains("name = \"mandatum-light\""), "{text}");
        assert!(text.contains("background = \"#102030\""), "{text}");
        assert!(!text.contains("#000000"), "old value replaced: {text}");
    }

    #[test]
    fn font_updates_write_family_and_size_the_loader_accepts() {
        let temp = TempConfig::new();
        let update = AppearanceUpdate {
            font_family: Some("JetBrains Mono".to_owned()),
            font_size: Some(14.5),
            ..AppearanceUpdate::default()
        };
        write_appearance_update(&temp.path(), &update).expect("write succeeds");

        let loaded = load_config(Some(&temp.path()), &temp.directory.join("missing.toml"));
        assert_eq!(loaded.font_family.as_deref(), Some("JetBrains Mono"));
        assert_eq!(loaded.font_size, Some(14.5));
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    }

    #[test]
    fn an_invalid_existing_file_is_left_untouched_and_reported() {
        let temp = TempConfig::new();
        std::fs::write(temp.path(), "this {{{ is not toml").unwrap();

        let error = write_appearance_update(&temp.path(), &update_all())
            .expect_err("invalid TOML must not be overwritten");
        assert!(error.contains("not valid TOML"), "{error}");
        assert_eq!(
            std::fs::read_to_string(temp.path()).unwrap(),
            "this {{{ is not toml",
            "the user's file survives byte for byte"
        );
    }
}
