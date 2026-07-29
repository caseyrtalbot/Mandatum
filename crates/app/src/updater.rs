//! The in-app update door.
//!
//! The heavy machinery — download, checksum verification, downgrade refusal,
//! atomic swap with backup and rollback — lives in the shipped installer and
//! is exercised by `ci/distribution-smoke.sh`. This module only finds that
//! door: it resolves the installed updater command for the Update Mandatum
//! task pane, and reads the latest release version for the launch-time
//! availability note. (`src/update.rs` is the terminal binary's embedded
//! `mandatum update` subcommand; this module serves the running app.)

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::events::{AppEvent, AppEventSender};

/// GitHub resolves this to the latest release tag via redirect; the check
/// reads the version from the redirect target without downloading anything.
const RELEASES_LATEST_URL: &str = "https://github.com/caseyrtalbot/Mandatum/releases/latest";

/// At most one release check per interval, across launches.
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// The running app's version, from the workspace package version — the same
/// value the release workflow requires the tag to match.
pub(crate) const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The resolved in-app update door: the shell command the Update Mandatum
/// task pane runs, and the app bundle it will replace (`None` for a
/// PATH-resolved launcher, which pins its own destination).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedUpdater {
    pub(crate) command: String,
    pub(crate) bundle: Option<PathBuf>,
}

/// Resolve the updater from the running binary.
///
/// Installed app: the bundle's own launcher, with the command install
/// directory and the app destination pinned — the launcher must replace
/// exactly this installation, never an ambient override's. Dev binary: a
/// `mandatum` on PATH (updating the installed app from a dev build is
/// deliberate and visible — the task pane names the command). `None` means
/// no updater exists on this machine. Every embedded path must be
/// shell-quotable; a path this module cannot embed safely resolves to no
/// updater rather than to a differently-interpreted command.
pub(crate) fn resolve_updater() -> Option<ResolvedUpdater> {
    resolve_updater_from(std::env::current_exe().ok(), std::env::var_os("PATH"))
}

fn resolve_updater_from(
    current_exe: Option<PathBuf>,
    path_var: Option<std::ffi::OsString>,
) -> Option<ResolvedUpdater> {
    if let Some(exe) = current_exe {
        // <bundle>/Contents/MacOS/<exe> -> <bundle>/Contents/Resources/mandatum
        if let Some(contents) = exe.parent().and_then(Path::parent) {
            let launcher = contents.join("Resources").join("mandatum");
            let bundle = contents.parent();
            if launcher.is_file()
                && let Some(bundle) = bundle
                && let Some(quoted_launcher) = shell_quotable(&launcher)
                && let Some(quoted_app_dir) = bundle.parent().and_then(shell_quotable)
            {
                return Some(ResolvedUpdater {
                    command: format!(
                        "MANDATUM_INSTALL_DIR=\"$HOME/.local/bin\" \
                         MANDATUM_APP_DIR=\"{quoted_app_dir}\" \
                         \"{quoted_launcher}\" update"
                    ),
                    bundle: Some(bundle.to_path_buf()),
                });
            }
        }
    }
    let path_var = path_var?;
    std::env::split_paths(&path_var)
        .map(|dir| dir.join("mandatum"))
        .find(|candidate| is_executable_file(candidate))
        .and_then(|launcher| {
            Some(ResolvedUpdater {
                command: format!("\"{}\" update", shell_quotable(&launcher)?),
                bundle: None,
            })
        })
}

/// A path rendered for embedding between double quotes in a `sh -c` string,
/// or `None` when the path contains any character the double-quoted context
/// would interpret (`$`, backtick, `"`, `\`) or a control character. No
/// legitimate install path contains these; refusing beats escaping.
fn shell_quotable<P: AsRef<Path>>(path: P) -> Option<String> {
    let rendered = path.as_ref().to_str()?.to_owned();
    let safe = !rendered
        .chars()
        .any(|c| matches!(c, '$' | '`' | '"' | '\\') || c.is_control());
    safe.then_some(rendered)
}

fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// The conventional install locations, for verifying an update driven by a
/// PATH-resolved launcher whose destination bundle is not known up front.
pub(crate) fn default_bundle() -> Option<PathBuf> {
    let mut candidates = vec![PathBuf::from("/Applications/Mandatum.app")];
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(
            PathBuf::from(home)
                .join("Applications")
                .join("Mandatum.app"),
        );
    }
    candidates
        .into_iter()
        .find(|bundle| bundle.join("Contents").join("Info.plist").is_file())
}

/// The version recorded in an app bundle's Info.plist, read with the system
/// plutil — the same source `mandatum --version` reports. `None` when the
/// bundle or key is unreadable.
pub(crate) fn bundle_version(bundle: &Path) -> Option<String> {
    let output = std::process::Command::new("/usr/bin/plutil")
        .args(["-extract", "CFBundleShortVersionString", "raw", "-o", "-"])
        .arg(bundle.join("Contents").join("Info.plist"))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    parse_version(&version).map(|_| version)
}

/// The throttle stamp lives beside the user config: the check is app-global,
/// not per-project.
pub fn update_check_stamp_file() -> Option<PathBuf> {
    Some(crate::config::user_config_file()?.with_file_name("update-check"))
}

/// Extract `x.y.z` from a `releases/latest` redirect target
/// (`.../releases/tag/v0.5.0`). Anything that is not a numeric triplet is
/// rejected: a parse failure must read as "no update", never as one.
fn parse_release_tag_version(redirect_url: &str) -> Option<String> {
    let trimmed = redirect_url.trim();
    let tag = trimmed.rsplit("/tag/").next()?;
    if tag == trimmed {
        return None;
    }
    let version = tag.strip_prefix('v').unwrap_or(tag);
    parse_version(version).map(|_| version.to_owned())
}

fn parse_version(version: &str) -> Option<[u64; 3]> {
    let mut parts = version.split('.');
    let triplet = [parts.next()?, parts.next()?, parts.next()?];
    if parts.next().is_some() {
        return None;
    }
    let mut numbers = [0u64; 3];
    for (slot, part) in numbers.iter_mut().zip(triplet) {
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        *slot = part.parse().ok()?;
    }
    Some(numbers)
}

pub(crate) fn version_is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

/// True when the stamp is absent, unreadable, or older than the interval.
/// A missing or corrupt stamp reads as "check now": the failure mode is one
/// extra HEAD request, never a silent stop.
fn should_check(stamp_file: &Path, now_secs: u64) -> bool {
    let Ok(contents) = std::fs::read_to_string(stamp_file) else {
        return true;
    };
    let Ok(last) = contents.trim().parse::<u64>() else {
        return true;
    };
    now_secs.saturating_sub(last) >= CHECK_INTERVAL.as_secs()
}

fn record_check(stamp_file: &Path, now_secs: u64) {
    if let Some(parent) = stamp_file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(stamp_file, format!("{now_secs}\n"));
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// Fetch the latest release version by reading the redirect target of the
/// `releases/latest` URL. Runs `curl` exactly like the shipped installer
/// (same TLS floor); no release bytes are downloaded.
fn fetch_latest_version() -> Option<String> {
    // The system curl by absolute path: this runs automatically at launch,
    // so it must never resolve a curl planted earlier on PATH.
    let output = std::process::Command::new("/usr/bin/curl")
        .args([
            "--fail",
            "--silent",
            "--head",
            "--proto",
            "=https",
            "--tlsv1.2",
            "--max-time",
            "10",
            "--output",
            "/dev/null",
            "--write-out",
            "%{redirect_url}",
            RELEASES_LATEST_URL,
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_release_tag_version(&String::from_utf8_lossy(&output.stdout))
}

/// Launch-time release check: at most once per [`CHECK_INTERVAL`], on its own
/// OS thread, reporting through the unified event channel. Failures are
/// silent by design — an offline launch must not surface noise.
pub(crate) fn spawn_release_check(sender: AppEventSender, stamp_file: PathBuf) {
    std::thread::Builder::new()
        .name("update-check".to_owned())
        .spawn(move || {
            let now = now_secs();
            if !should_check(&stamp_file, now) {
                return;
            }
            // Stamp the attempt before fetching: a failing endpoint is
            // retried next interval, not hammered every launch.
            record_check(&stamp_file, now);
            let Some(latest) = fetch_latest_version() else {
                return;
            };
            if version_is_newer(&latest, CURRENT_VERSION) {
                let _ = sender.send(AppEvent::UpdateAvailable(latest));
            }
        })
        .expect("spawning the update-check thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_tags_parse_to_numeric_triplets_only() {
        assert_eq!(
            parse_release_tag_version("https://github.com/x/y/releases/tag/v0.5.0"),
            Some("0.5.0".to_owned())
        );
        assert_eq!(
            parse_release_tag_version("https://github.com/x/y/releases/tag/1.2.3\n"),
            Some("1.2.3".to_owned())
        );
        // No redirect (empty write-out), junk tags, and non-triplets all
        // read as "no update".
        assert_eq!(parse_release_tag_version(""), None);
        assert_eq!(
            parse_release_tag_version("https://github.com/x/y/releases"),
            None
        );
        assert_eq!(
            parse_release_tag_version("https://github.com/x/y/releases/tag/beta"),
            None
        );
        assert_eq!(
            parse_release_tag_version("https://github.com/x/y/releases/tag/v1.2"),
            None
        );
        assert_eq!(
            parse_release_tag_version("https://github.com/x/y/releases/tag/v1.2.3.4"),
            None
        );
    }

    #[test]
    fn version_comparison_is_numeric_and_fails_closed() {
        assert!(version_is_newer("0.5.0", "0.4.0"));
        assert!(version_is_newer("0.4.10", "0.4.9"));
        assert!(version_is_newer("1.0.0", "0.99.99"));
        assert!(!version_is_newer("0.4.0", "0.4.0"));
        assert!(!version_is_newer("0.3.9", "0.4.0"));
        // Unparsable versions never claim to be newer.
        assert!(!version_is_newer("nightly", "0.4.0"));
        assert!(!version_is_newer("0.5.0", "unknown"));
        // The compiled-in version is a valid triplet.
        assert!(parse_version(CURRENT_VERSION).is_some());
    }

    #[test]
    fn throttle_checks_once_per_interval_and_fails_open() {
        let dir =
            std::env::temp_dir().join(format!("mandatum-update-throttle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let stamp = dir.join("update-check");

        // Missing stamp: check, then record.
        assert!(should_check(&stamp, 1_000_000));
        record_check(&stamp, 1_000_000);
        assert!(!should_check(&stamp, 1_000_000 + 60));
        assert!(should_check(&stamp, 1_000_000 + CHECK_INTERVAL.as_secs()));

        // A corrupt stamp fails open.
        std::fs::write(&stamp, "not a number").unwrap();
        assert!(should_check(&stamp, 1_000_000));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn updater_resolution_prefers_the_bundle_and_falls_back_to_path() {
        let dir =
            std::env::temp_dir().join(format!("mandatum-update-resolve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        // Bundle layout: Contents/MacOS/Mandatum + Contents/Resources/mandatum.
        let contents = dir.join("Mandatum.app/Contents");
        std::fs::create_dir_all(contents.join("MacOS")).unwrap();
        std::fs::create_dir_all(contents.join("Resources")).unwrap();
        std::fs::write(contents.join("Resources/mandatum"), "#!/bin/sh\n").unwrap();
        let exe = contents.join("MacOS/Mandatum");
        std::fs::write(&exe, "").unwrap();

        let resolved = resolve_updater_from(Some(exe.clone()), None).unwrap();
        assert!(
            resolved.command.ends_with("\" update"),
            "{}",
            resolved.command
        );
        assert!(
            resolved.command.contains("Resources/mandatum"),
            "{}",
            resolved.command
        );
        // The bundle path pins both destinations so the launcher can only
        // replace this installation and installs the command to the
        // installer's default, never into Contents/Resources.
        assert!(
            resolved
                .command
                .starts_with("MANDATUM_INSTALL_DIR=\"$HOME/.local/bin\""),
            "{}",
            resolved.command
        );
        assert!(
            resolved.command.contains("MANDATUM_APP_DIR="),
            "{}",
            resolved.command
        );
        assert_eq!(
            resolved.bundle.as_deref(),
            Some(dir.join("Mandatum.app").as_path())
        );

        // A path a double-quoted shell context would interpret resolves to
        // no updater, never to a differently-parsed command.
        let hostile = dir.join("evil$(touch pwned)/Mandatum.app/Contents");
        std::fs::create_dir_all(hostile.join("MacOS")).unwrap();
        std::fs::create_dir_all(hostile.join("Resources")).unwrap();
        std::fs::write(hostile.join("Resources/mandatum"), "#!/bin/sh\n").unwrap();
        let hostile_exe = hostile.join("MacOS/Mandatum");
        std::fs::write(&hostile_exe, "").unwrap();
        assert_eq!(resolve_updater_from(Some(hostile_exe), None), None);

        // No bundle: an executable `mandatum` on PATH wins; a bare file
        // without the executable bit does not.
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let launcher = bin.join("mandatum");
        std::fs::write(&launcher, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o644)).unwrap();
        let plain_exe = dir.join("mandatum-dev");
        std::fs::write(&plain_exe, "").unwrap();
        let path_var = Some(std::ffi::OsString::from(bin.as_os_str()));

        assert_eq!(
            resolve_updater_from(Some(plain_exe.clone()), path_var.clone()),
            None,
            "a non-executable PATH candidate is not an updater"
        );
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755)).unwrap();
        let resolved = resolve_updater_from(Some(plain_exe), path_var).expect("PATH fallback");
        assert!(
            resolved.command.contains("bin/mandatum"),
            "{}",
            resolved.command
        );
        assert!(
            !resolved.command.starts_with("MANDATUM_INSTALL_DIR="),
            "{}",
            resolved.command
        );
        assert_eq!(resolved.bundle, None);

        // Nothing anywhere: no updater.
        assert_eq!(resolve_updater_from(None, None), None);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
