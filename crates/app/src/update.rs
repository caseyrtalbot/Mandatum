use std::env;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const INSTALLER: &[u8] = include_bytes!("../../../install.sh");

pub(crate) fn install_latest() -> Result<(), String> {
    let executable = env::current_exe()
        .map_err(|error| format!("could not locate the running executable: {error}"))?;
    let executable = executable.canonicalize().unwrap_or(executable);
    let install_dir = executable_install_dir(&executable)?;

    println!(
        "Updating Mandatum from the latest GitHub release in {}...",
        install_dir.display()
    );
    run_installer(INSTALLER, &install_dir)
}

fn executable_install_dir(executable: &Path) -> Result<PathBuf, String> {
    executable
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            format!(
                "could not determine the install directory for {}",
                executable.display()
            )
        })
}

fn run_installer(installer: &[u8], install_dir: &Path) -> Result<(), String> {
    let mut child = Command::new("/bin/sh")
        .env("MANDATUM_INSTALL_DIR", install_dir)
        .env("MANDATUM_CURRENT_VERSION", env!("CARGO_PKG_VERSION"))
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not start the update installer: {error}"))?;

    let write_result = child
        .stdin
        .take()
        .ok_or_else(|| "could not open the update installer's input".to_owned())
        .and_then(|mut stdin| {
            stdin
                .write_all(installer)
                .map_err(|error| format!("could not send the updater to the installer: {error}"))
        });

    if let Err(problem) = write_result {
        let _ = child.wait();
        return Err(problem);
    }

    let status = child
        .wait()
        .map_err(|error| format!("could not wait for the update installer: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("update installer exited with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_installs_beside_the_running_executable() {
        assert_eq!(
            executable_install_dir(Path::new("/tmp/mandatum/bin/mandatum")),
            Ok(PathBuf::from("/tmp/mandatum/bin"))
        );
    }

    #[test]
    fn embedded_installer_receives_the_selected_install_directory() {
        let script = format!(
            r#"test "$MANDATUM_INSTALL_DIR" = "/tmp/mandatum update/bin" && test "$MANDATUM_CURRENT_VERSION" = "{}""#,
            env!("CARGO_PKG_VERSION")
        );
        run_installer(script.as_bytes(), Path::new("/tmp/mandatum update/bin"))
            .expect("installer receives the exact executable directory");
    }

    #[test]
    fn installer_failure_is_reported() {
        let problem = run_installer(b"exit 23", Path::new("/tmp"))
            .expect_err("non-zero installer status must fail the update");

        assert!(problem.contains("23"), "{problem}");
    }
}
