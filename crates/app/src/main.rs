use std::ffi::{OsStr, OsString};

mod update;

const HELP: &str = "Mandatum — a development workstation for terminal-centered builders

Usage:
  mandatum
  mandatum update
  mandatum [OPTIONS]

Commands:
  update          Install the latest published release

Options:
  -h, --help     Print help
  -V, --version  Print version

With no options, Mandatum opens a workspace in the current directory.
";

#[derive(Debug, Eq, PartialEq)]
enum Command {
    Run,
    Help,
    Version,
    Update,
}

fn main() {
    let command = match parse_command(std::env::args_os().skip(1)) {
        Ok(command) => command,
        Err(problem) => {
            eprintln!("mandatum: {problem}");
            eprintln!("Try 'mandatum --help' for more information.");
            std::process::exit(2);
        }
    };

    match command {
        Command::Help => print!("{HELP}"),
        Command::Version => println!("mandatum {}", env!("CARGO_PKG_VERSION")),
        Command::Update => {
            if let Err(error) = update::install_latest() {
                eprintln!("mandatum: update failed: {error}");
                std::process::exit(1);
            }
        }
        Command::Run => {
            if let Err(error) = mandatum_app::run() {
                eprintln!("mandatum: {error}");
                std::process::exit(1);
            }
        }
    }
}

fn parse_command(args: impl IntoIterator<Item = OsString>) -> Result<Command, String> {
    let args = args.into_iter().collect::<Vec<_>>();
    match args.as_slice() {
        [] => Ok(Command::Run),
        [flag] if flag == OsStr::new("--help") || flag == OsStr::new("-h") => Ok(Command::Help),
        [flag] if flag == OsStr::new("--version") || flag == OsStr::new("-V") => {
            Ok(Command::Version)
        }
        [command] if command == OsStr::new("update") => Ok(Command::Update),
        [argument] => Err(format!(
            "unrecognized argument '{}'",
            argument.to_string_lossy()
        )),
        _ => Err("only one option may be supplied".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_is_a_non_interactive_subcommand() {
        assert_eq!(
            parse_command([OsString::from("update")]),
            Ok(Command::Update)
        );
    }
}
