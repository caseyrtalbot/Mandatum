//! Generate the live-slice demo workspace file through the real core API,
//! so the committed example can never drift from the persistence schema.
//!
//! Usage: `cargo run -p mandatum-app --example make_live_slice -- <dir>`
//! writes `<dir>/.mandatum/workspace.json`. The recorded project path is
//! `"."` so the file is portable: launch Mandatum from `<dir>` and every
//! cwd resolves there — task intents leave `cwd` unset on purpose, and the
//! spawn path resolves an unset cwd to the project path (never `$HOME`).
//! `examples/live-slice/run.sh` drives the whole demo.

use std::{env, fs, path::PathBuf, process::ExitCode};

use mandatum_core::{AgentPaneIntent, CoreAction, PaneId, TaskPaneIntent, Workspace};

fn main() -> ExitCode {
    let Some(dir) = env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: make_live_slice <project-dir>");
        return ExitCode::FAILURE;
    };

    let mut workspace = Workspace::new("Mandatum", PathBuf::from("."));

    // (a) A task pane with a rerunnable check that alternates pass/fail
    // (the driver script writes flaky-check.sh). Docked into the tiled tree.
    apply(
        &mut workspace,
        CoreAction::CreateTaskPane {
            title: "checks".to_owned(),
            intent: TaskPaneIntent {
                recipe_id: Some("checks".to_owned()),
                command: "sh ./flaky-check.sh".to_owned(),
                cwd: None,
            },
        },
    );
    apply(&mut workspace, CoreAction::DockFocused);

    // (b) A long-running dev-server-style pane: a heartbeat loop.
    apply(
        &mut workspace,
        CoreAction::CreateTaskPane {
            title: "dev server".to_owned(),
            intent: TaskPaneIntent {
                recipe_id: Some("dev-server".to_owned()),
                command: "i=0; while :; do i=$((i+1)); echo \"heartbeat $i\"; sleep 2; done"
                    .to_owned(),
                cwd: None,
            },
        },
    );
    apply(&mut workspace, CoreAction::DockFocused);

    // (c) An agent pane; the fake connector's built-in script runs, asks to
    // remove the flaky marker (an approval), and waits for the verdict.
    apply(
        &mut workspace,
        CoreAction::CreateAgentPane {
            title: "agent".to_owned(),
            intent: AgentPaneIntent::draft(
                "remove the flaky marker file so the checks pass (fake connector demo)",
            ),
            cwd: None,
        },
    );

    // A stranger's first look lands on the checks pane.
    apply(
        &mut workspace,
        CoreAction::FocusPane {
            pane_id: PaneId::new("pane-2"),
        },
    );

    let file = dir.join(".mandatum").join("workspace.json");
    if let Some(parent) = file.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        eprintln!("cannot create {}: {error}", parent.display());
        return ExitCode::FAILURE;
    }
    let json = match workspace.to_json() {
        Ok(json) => json,
        Err(error) => {
            eprintln!("serialize failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = fs::write(&file, json) {
        eprintln!("cannot write {}: {error}", file.display());
        return ExitCode::FAILURE;
    }
    println!("wrote {}", file.display());
    ExitCode::SUCCESS
}

fn apply(workspace: &mut Workspace, action: CoreAction) {
    workspace
        .apply_action(action)
        .expect("live-slice actions are valid by construction");
}
