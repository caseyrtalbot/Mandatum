use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Output};

#[test]
fn public_executable_is_named_mandatum() {
    let binary = Path::new(env!("CARGO_BIN_EXE_mandatum"));
    assert_eq!(binary.file_name(), Some(OsStr::new("mandatum")));
}

#[test]
fn help_flags_print_usage_to_stdout_and_exit_successfully() {
    for flag in ["--help", "-h"] {
        let output = run_mandatum(flag);

        assert!(output.status.success(), "{flag}: {output:?}");
        assert!(output.stderr.is_empty(), "{flag}: {output:?}");
        let stdout = String::from_utf8(output.stdout).expect("help output is UTF-8");
        assert!(stdout.contains("Usage:\n  mandatum"), "{flag}: {stdout}");
        assert!(stdout.contains("-h, --help"), "{flag}: {stdout}");
        assert!(stdout.contains("-V, --version"), "{flag}: {stdout}");
    }
}

#[test]
fn version_flags_print_package_version_to_stdout_and_exit_successfully() {
    for flag in ["--version", "-V"] {
        let output = run_mandatum(flag);

        assert!(output.status.success(), "{flag}: {output:?}");
        assert!(output.stderr.is_empty(), "{flag}: {output:?}");
        assert_eq!(
            String::from_utf8(output.stdout).expect("version output is UTF-8"),
            format!("mandatum {}\n", env!("CARGO_PKG_VERSION")),
            "{flag}"
        );
    }
}

#[test]
fn unknown_option_reports_the_problem_without_entering_the_tui() {
    let output = run_mandatum("--definitely-not-a-mandatum-option");

    assert!(!output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("error output is UTF-8");
    assert!(
        stderr.contains("unrecognized argument '--definitely-not-a-mandatum-option'"),
        "{stderr}"
    );
    assert!(stderr.contains("mandatum --help"), "{stderr}");
}

#[test]
fn excess_arguments_report_usage_error_without_entering_the_tui() {
    let output = run_mandatum_args(["--help", "--version"]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("error output is UTF-8");
    assert!(
        stderr.contains("only one option may be supplied"),
        "{stderr}"
    );
    assert!(stderr.contains("mandatum --help"), "{stderr}");
}

fn run_mandatum(argument: &str) -> Output {
    run_mandatum_args([argument])
}

fn run_mandatum_args(arguments: impl IntoIterator<Item = impl AsRef<OsStr>>) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mandatum"))
        .args(arguments)
        .output()
        .expect("mandatum subprocess starts")
}
