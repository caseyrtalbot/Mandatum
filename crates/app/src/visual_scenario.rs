//! Deterministic product-state recipes for native visual acceptance.
//!
//! Recipes prepare durable fixtures through `mandatum-core`, then drive the
//! real [`FrontendHost`] exclusively with neutral input. They never construct
//! renderer fixtures or mutate `AppState` through test-only seams.

use std::{
    fmt, fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use mandatum_core::{
    AgentPaneIntent, AgentStatus, ArtifactFit, ArtifactPaneIntent, CoreAction, TaskPaneIntent,
    Workspace,
};
use mandatum_scene::{
    ArtifactState, HitTargetKind, PaneContent, SceneSize,
    input::{InputEvent, Key, KeyCode, Modifiers, PointerButton, PointerEvent, PointerKind},
};

use crate::{AppConfig, FrameSnapshot, FrontendHost};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum VisualScenarioId {
    Typography,
    CalmTerminal,
    DenseWorkspace,
    Attention,
    Palette,
    FullModal,
    Welcome,
    ContextMenu,
    Artifacts,
    Narrow,
    Restored,
}

impl VisualScenarioId {
    pub const ALL: [Self; 11] = [
        Self::Typography,
        Self::CalmTerminal,
        Self::DenseWorkspace,
        Self::Attention,
        Self::Palette,
        Self::FullModal,
        Self::Welcome,
        Self::ContextMenu,
        Self::Artifacts,
        Self::Narrow,
        Self::Restored,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Typography => "typography",
            Self::CalmTerminal => "calm-terminal",
            Self::DenseWorkspace => "dense-workspace",
            Self::Attention => "attention",
            Self::Palette => "palette",
            Self::FullModal => "full-modal",
            Self::Welcome => "welcome",
            Self::ContextMenu => "context-menu",
            Self::Artifacts => "artifacts",
            Self::Narrow => "narrow",
            Self::Restored => "restored",
        }
    }
}

impl std::str::FromStr for VisualScenarioId {
    type Err = VisualScenarioError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.as_str() == value)
            .ok_or_else(|| VisualScenarioError::UnknownScenario(value.to_owned()))
    }
}

impl fmt::Display for VisualScenarioId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VisualScenarioDescriptor {
    pub id: VisualScenarioId,
    pub intended_hierarchy: &'static [&'static str],
    pub component_states: &'static [&'static str],
}

pub const VISUAL_SCENARIOS: &[VisualScenarioDescriptor] = &[
    descriptor(
        VisualScenarioId::Typography,
        &["workspace", "terminal pane", "terminal corpus"],
        &[
            "ansi and true color",
            "styles",
            "unicode",
            "cursor",
            "selection",
        ],
    ),
    descriptor(
        VisualScenarioId::CalmTerminal,
        &["workspace", "focused terminal pane", "status"],
        &["idle", "focused", "clean output"],
    ),
    descriptor(
        VisualScenarioId::DenseWorkspace,
        &["workspace", "three tiled panes", "floating artifact"],
        &["terminal", "task", "agent complete", "artifact ready"],
    ),
    descriptor(
        VisualScenarioId::Attention,
        &[
            "workspace",
            "failed task",
            "waiting agent",
            "attention strip",
        ],
        &["failed", "waiting approval", "focused"],
    ),
    descriptor(
        VisualScenarioId::Palette,
        &["workspace", "modal palette", "command rows"],
        &["selected", "disabled", "filtered", "overflow"],
    ),
    descriptor(
        VisualScenarioId::FullModal,
        &["workspace", "modal timeline", "timeline rows"],
        &["selected", "restored fact", "command fact"],
    ),
    descriptor(
        VisualScenarioId::Welcome,
        &["workspace", "non-modal welcome", "route rows"],
        &["first run", "dismissible"],
    ),
    descriptor(
        VisualScenarioId::ContextMenu,
        &["workspace", "anchored context menu", "command rows"],
        &["selected", "pane scoped"],
    ),
    descriptor(
        VisualScenarioId::Artifacts,
        &[
            "workspace",
            "artifact panes",
            "artifact surfaces",
            "anchored overlay",
        ],
        &[
            "loading transition",
            "landscape ready",
            "portrait ready",
            "failed",
            "overlay occlusion",
        ],
    ),
    descriptor(
        VisualScenarioId::Narrow,
        &["workspace", "narrow tiled panes", "status"],
        &["truncation", "minimum usable geometry"],
    ),
    descriptor(
        VisualScenarioId::Restored,
        &["restored workspace", "tiled panes", "restore status"],
        &["restored", "live runtime detached"],
    ),
];

const fn descriptor(
    id: VisualScenarioId,
    intended_hierarchy: &'static [&'static str],
    component_states: &'static [&'static str],
) -> VisualScenarioDescriptor {
    VisualScenarioDescriptor {
        id,
        intended_hierarchy,
        component_states,
    }
}

#[derive(Debug)]
pub enum VisualScenarioError {
    Io(std::io::Error),
    Workspace(String),
    UnknownScenario(String),
    Timeout {
        scenario: VisualScenarioId,
        expectation: &'static str,
    },
    MissingHitTarget(&'static str),
}

impl fmt::Display for VisualScenarioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Workspace(error) => formatter.write_str(error),
            Self::UnknownScenario(value) => write!(formatter, "unknown visual scenario: {value}"),
            Self::Timeout {
                scenario,
                expectation,
            } => write!(
                formatter,
                "visual scenario {scenario} did not reach {expectation}"
            ),
            Self::MissingHitTarget(kind) => {
                write!(
                    formatter,
                    "visual scenario did not expose a {kind} hit target"
                )
            }
        }
    }
}

impl std::error::Error for VisualScenarioError {}

impl From<std::io::Error> for VisualScenarioError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

pub struct PreparedVisualScenario {
    id: VisualScenarioId,
    fixture_root: PathBuf,
    config: AppConfig,
}

impl PreparedVisualScenario {
    pub fn id(&self) -> VisualScenarioId {
        self.id
    }

    pub fn descriptor(&self) -> &'static VisualScenarioDescriptor {
        VISUAL_SCENARIOS
            .iter()
            .find(|descriptor| descriptor.id == self.id)
            .expect("every visual scenario has a descriptor")
    }

    pub fn fixture_root(&self) -> &Path {
        &self.fixture_root
    }

    pub fn app_config(&self) -> AppConfig {
        self.config.clone()
    }

    /// Remove per-run fixture paths from a catalog frame before pixel capture.
    ///
    /// The real host still runs against its isolated temporary project. Only
    /// review-facing scene strings are normalized, so two runs produce the
    /// same semantic pixels without weakening runtime isolation.
    pub fn stabilize_snapshot(
        &self,
        snapshot: &mut FrameSnapshot,
    ) -> Result<(), VisualScenarioError> {
        let root = self.fixture_root.display().to_string();
        let basename = self
            .fixture_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&root);
        let mut value = serde_json::to_value(&snapshot.scene)
            .map_err(|error| VisualScenarioError::Workspace(error.to_string()))?;
        normalize_fixture_strings(&mut value, &root, basename);
        snapshot.scene = serde_json::from_value(value)
            .map_err(|error| VisualScenarioError::Workspace(error.to_string()))?;
        Ok(())
    }

    pub fn drive(
        &self,
        host: &mut FrontendHost,
        size: SceneSize,
        timeout: Duration,
    ) -> Result<FrameSnapshot, VisualScenarioError> {
        host.handle_input(InputEvent::Resize(size));
        let initial = host.frame(size);

        match self.id {
            VisualScenarioId::Typography => {
                settle(host, size, timeout, self.id, "typography corpus", |frame| {
                    scene_text(frame).contains("TYPOGRAPHY_CORPUS_READY")
                })?;
                dispatch_palette_command(host, '[');
                host.handle_input(InputEvent::Key(Key::plain(KeyCode::Char('v'))));
                host.handle_input(InputEvent::Key(Key::plain(KeyCode::Right)));
                host.handle_input(InputEvent::Key(Key::plain(KeyCode::Right)));
                Ok(host.frame(size))
            }
            VisualScenarioId::CalmTerminal => settle(
                host,
                size,
                timeout,
                self.id,
                "calm terminal output",
                |frame| scene_text(frame).contains("CALM_TERMINAL_READY"),
            ),
            VisualScenarioId::DenseWorkspace => settle(
                host,
                size,
                timeout,
                self.id,
                "dense workspace artifact",
                |frame| {
                    frame.scene.panes.iter().any(|pane| {
                    matches!(&pane.content, PaneContent::Artifact(content) if matches!(content.state, ArtifactState::Ready(_)))
                })
                },
            ),
            VisualScenarioId::Attention => {
                dispatch_palette_command(host, 'g');
                let waiting = settle(host, size, timeout, self.id, "waiting approval", |frame| {
                    frame.scene.panes.iter().any(|pane| {
                        matches!(&pane.content, PaneContent::Agent(agent) if agent.status_role == AgentStatus::WaitingForApproval)
                    })
                })?;
                let task_id = waiting
                    .scene
                    .panes
                    .iter()
                    .find(|pane| matches!(pane.content, PaneContent::Task(_)))
                    .map(|pane| pane.id.clone())
                    .ok_or(VisualScenarioError::MissingHitTarget("task pane"))?;
                let task_target = waiting
                    .scene
                    .hit_targets
                    .iter()
                    .find(|target| {
                        matches!(&target.kind, HitTargetKind::PaneBody(pane_id) if pane_id == &task_id)
                    })
                    .ok_or(VisualScenarioError::MissingHitTarget("task pane body"))?;
                host.handle_input(InputEvent::Pointer(PointerEvent {
                    kind: PointerKind::Down,
                    button: Some(PointerButton::Left),
                    column: task_target.rect.x,
                    row: task_target.rect.y,
                    mods: Modifiers::NONE,
                }));
                dispatch_palette_command(host, 'r');
                settle(
                    host,
                    size,
                    timeout,
                    self.id,
                    "failed task and waiting approval",
                    |frame| {
                        let failed = frame.scene.panes.iter().any(|pane| {
                        matches!(&pane.content, PaneContent::Task(task) if task.status_label.as_deref() == Some("failed: exit 3"))
                    });
                        let waiting = frame.scene.panes.iter().any(|pane| {
                        matches!(&pane.content, PaneContent::Agent(agent) if agent.status_role == AgentStatus::WaitingForApproval)
                    });
                        failed && waiting
                    },
                )
            }
            VisualScenarioId::Palette => {
                host.handle_input(InputEvent::Key(Key::ctrl('p')));
                host.handle_input(InputEvent::Key(Key::new(
                    KeyCode::Char('E'),
                    Modifiers {
                        shift: true,
                        ..Modifiers::NONE
                    },
                )));
                Ok(host.frame(size))
            }
            VisualScenarioId::FullModal => {
                dispatch_palette_command(host, '/');
                Ok(host.frame(size))
            }
            VisualScenarioId::Welcome | VisualScenarioId::Restored => Ok(initial),
            VisualScenarioId::ContextMenu => {
                let pane_body = initial
                    .scene
                    .hit_targets
                    .iter()
                    .find(|target| matches!(target.kind, HitTargetKind::PaneBody(_)))
                    .ok_or(VisualScenarioError::MissingHitTarget("pane body"))?;
                host.handle_input(InputEvent::Pointer(PointerEvent {
                    kind: PointerKind::Down,
                    button: Some(PointerButton::Right),
                    column: pane_body.rect.x,
                    row: pane_body.rect.y,
                    mods: Modifiers::NONE,
                }));
                Ok(host.frame(size))
            }
            VisualScenarioId::Artifacts => {
                let settled = settle(
                    host,
                    size,
                    timeout,
                    self.id,
                    "two ready and one failed artifact",
                    |frame| {
                        let ready = frame.scene.panes.iter().filter(|pane| {
                    matches!(&pane.content, PaneContent::Artifact(content) if matches!(content.state, ArtifactState::Ready(_)))
                }).count();
                        let failed = frame.scene.panes.iter().any(|pane| {
                    matches!(&pane.content, PaneContent::Artifact(content) if matches!(content.state, ArtifactState::Failed { .. }))
                });
                        ready == 2 && failed
                    },
                )?;
                let artifact_id = settled
                    .scene
                    .panes
                    .iter()
                    .find(|pane| matches!(pane.content, PaneContent::Artifact(_)))
                    .map(|pane| pane.id.clone())
                    .ok_or(VisualScenarioError::MissingHitTarget("artifact pane"))?;
                let artifact_target = settled
                    .scene
                    .hit_targets
                    .iter()
                    .find(|target| {
                        matches!(&target.kind, HitTargetKind::PaneBody(pane_id) if pane_id == &artifact_id)
                    })
                    .ok_or(VisualScenarioError::MissingHitTarget(
                        "artifact pane body",
                    ))?;
                host.handle_input(InputEvent::Pointer(PointerEvent {
                    kind: PointerKind::Down,
                    button: Some(PointerButton::Right),
                    column: artifact_target.rect.x,
                    row: artifact_target.rect.y,
                    mods: Modifiers::NONE,
                }));
                Ok(host.frame(size))
            }
            VisualScenarioId::Narrow => Ok(initial),
        }
    }
}

fn normalize_fixture_strings(value: &mut serde_json::Value, root: &str, basename: &str) {
    match value {
        serde_json::Value::String(text) => {
            *text = text
                .replace(root, "$VISUAL_PROJECT")
                .replace(basename, "visual-project");
        }
        serde_json::Value::Array(values) => {
            for value in values {
                normalize_fixture_strings(value, root, basename);
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values_mut() {
                normalize_fixture_strings(value, root, basename);
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

pub fn prepare_visual_scenario(
    id: VisualScenarioId,
    fixture_root: &Path,
) -> Result<PreparedVisualScenario, VisualScenarioError> {
    fs::create_dir_all(fixture_root)?;
    fs::create_dir_all(fixture_root.join(".mandatum"))?;
    write_shell_fixture(fixture_root)?;
    write_artifact_fixture(&fixture_root.join("landscape.png"), 4, 2)?;
    write_artifact_fixture(&fixture_root.join("portrait.png"), 2, 4)?;

    let workspace_file = fixture_root.join(".mandatum").join("workspace.json");
    if id != VisualScenarioId::Welcome {
        let workspace = workspace_for(id, fixture_root)?;
        fs::write(
            &workspace_file,
            workspace
                .to_json()
                .map_err(|error| VisualScenarioError::Workspace(error.to_string()))?,
        )?;
    } else if workspace_file.exists() {
        fs::remove_file(&workspace_file)?;
    }

    let spawn_pty = matches!(
        id,
        VisualScenarioId::Typography
            | VisualScenarioId::CalmTerminal
            | VisualScenarioId::DenseWorkspace
            | VisualScenarioId::Attention
    );
    let config = AppConfig {
        workspace_name: "Mandatum Visual Acceptance".to_owned(),
        project_path: fixture_root.to_path_buf(),
        workspace_file,
        shell_program: fixture_root.join("visual-shell.sh").display().to_string(),
        spawn_pty,
        restore_on_startup: true,
        ..AppConfig::default()
    };

    Ok(PreparedVisualScenario {
        id,
        fixture_root: fixture_root.to_path_buf(),
        config,
    })
}

fn workspace_for(
    id: VisualScenarioId,
    fixture_root: &Path,
) -> Result<Workspace, VisualScenarioError> {
    let mut workspace = Workspace::new("Visual Acceptance", fixture_root.to_path_buf());
    match id {
        VisualScenarioId::DenseWorkspace
        | VisualScenarioId::Palette
        | VisualScenarioId::FullModal => {
            add_dense_workspace(&mut workspace)?;
        }
        VisualScenarioId::Attention => {
            apply(
                &mut workspace,
                CoreAction::CreateTaskPane {
                    title: "failing checks".to_owned(),
                    intent: TaskPaneIntent {
                        recipe_id: Some("checks".to_owned()),
                        command: "printf 'CHECK_FAILED\\n'; exit 3".to_owned(),
                        cwd: None,
                    },
                },
            )?;
            apply(&mut workspace, CoreAction::DockFocused)?;
            apply(
                &mut workspace,
                CoreAction::CreateAgentPane {
                    title: "release agent".to_owned(),
                    intent: AgentPaneIntent::draft("repair the failing check"),
                    cwd: None,
                },
            )?;
        }
        VisualScenarioId::Artifacts => {
            for (source, title, alt) in [
                ("landscape.png", "landscape artifact", "landscape fixture"),
                ("portrait.png", "portrait artifact", "portrait fixture"),
                ("missing.png", "failed artifact", "missing fixture"),
            ] {
                apply(
                    &mut workspace,
                    CoreAction::CreateArtifactPane {
                        intent: ArtifactPaneIntent {
                            source: PathBuf::from(source),
                            title: title.to_owned(),
                            alt_text: alt.to_owned(),
                            fit: ArtifactFit::Contain,
                        },
                    },
                )?;
                if source != "missing.png" {
                    apply(&mut workspace, CoreAction::DockFocused)?;
                }
            }
        }
        VisualScenarioId::Narrow => {
            for _ in 0..4 {
                apply(&mut workspace, CoreAction::SplitRight)?;
            }
        }
        VisualScenarioId::Restored => {
            apply(&mut workspace, CoreAction::SplitRight)?;
            apply(&mut workspace, CoreAction::SplitDown)?;
        }
        VisualScenarioId::Typography
        | VisualScenarioId::CalmTerminal
        | VisualScenarioId::ContextMenu => {}
        VisualScenarioId::Welcome => unreachable!("welcome has no saved workspace"),
    }
    Ok(workspace)
}

fn add_dense_workspace(workspace: &mut Workspace) -> Result<(), VisualScenarioError> {
    apply(
        workspace,
        CoreAction::CreateTaskPane {
            title: "checks".to_owned(),
            intent: TaskPaneIntent {
                recipe_id: Some("checks".to_owned()),
                command: "printf 'CHECKS_READY\\n'".to_owned(),
                cwd: None,
            },
        },
    )?;
    apply(workspace, CoreAction::DockFocused)?;

    let mut agent = AgentPaneIntent::draft("polish the native workspace");
    agent.status = AgentStatus::Complete;
    agent.latest_summary = Some("reviewed the current visual acceptance contract".to_owned());
    agent.changed_files = vec![PathBuf::from("docs/visual-polish-plan.md")];
    apply(
        workspace,
        CoreAction::CreateAgentPane {
            title: "visual agent".to_owned(),
            intent: agent,
            cwd: None,
        },
    )?;
    apply(workspace, CoreAction::DockFocused)?;
    apply(
        workspace,
        CoreAction::CreateArtifactPane {
            intent: ArtifactPaneIntent {
                source: PathBuf::from("landscape.png"),
                title: "reference artifact".to_owned(),
                alt_text: "fixed visual acceptance fixture".to_owned(),
                fit: ArtifactFit::Contain,
            },
        },
    )?;
    Ok(())
}

fn apply(workspace: &mut Workspace, action: CoreAction) -> Result<(), VisualScenarioError> {
    workspace
        .apply_action(action)
        .map(|_| ())
        .map_err(|error| VisualScenarioError::Workspace(error.to_string()))
}

fn dispatch_palette_command(host: &mut FrontendHost, key: char) {
    host.handle_input(InputEvent::Key(Key::ctrl('p')));
    host.handle_input(InputEvent::Key(Key::plain(KeyCode::Char(key))));
}

fn settle(
    host: &mut FrontendHost,
    size: SceneSize,
    timeout: Duration,
    scenario: VisualScenarioId,
    expectation: &'static str,
    predicate: impl Fn(&FrameSnapshot) -> bool,
) -> Result<FrameSnapshot, VisualScenarioError> {
    let deadline = Instant::now() + timeout;
    loop {
        let _ = host.wait_event(Duration::from_millis(20));
        while host.drain_runtime() > 0 {}
        host.heartbeat();
        let frame = host.frame(size);
        if predicate(&frame) {
            return Ok(frame);
        }
        if Instant::now() >= deadline {
            return Err(VisualScenarioError::Timeout {
                scenario,
                expectation,
            });
        }
    }
}

fn scene_text(frame: &FrameSnapshot) -> String {
    let mut text = String::new();
    for pane in &frame.scene.panes {
        text.push_str(&pane.detail_lines().join("\n"));
        if let PaneContent::Terminal(surface) = &pane.content {
            for row in &surface.rows {
                for cell in row {
                    text.push_str(cell.grapheme_text());
                }
                text.push('\n');
            }
        }
    }
    text
}

fn write_shell_fixture(root: &Path) -> Result<(), VisualScenarioError> {
    let path = root.join("visual-shell.sh");
    fs::write(
        &path,
        "#!/bin/sh\n\
         if [ \"$#\" -gt 0 ]; then exec /bin/sh \"$@\"; fi\n\
         printf 'TYPOGRAPHY_CORPUS_READY\\n'\n\
         printf 'CALM_TERMINAL_READY · cargo test · 128 passed\\n'\n\
         printf '\\033[1mBold\\033[0m \\033[3mItalic\\033[0m \\033[4mUnderline\\033[0m \\\n+         fi fl -> != <= >=\\n'\n\
         printf 'Latin é · Ελληνικά · Кириллица · العربية · हिन्दी · 界 · 🙂\\n'\n\
         exec /bin/cat\n",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions)?;
    }
    Ok(())
}

fn write_artifact_fixture(path: &Path, width: u32, height: u32) -> Result<(), VisualScenarioError> {
    let file = fs::File::create(path)?;
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            pixels.extend_from_slice(if (x + y) % 2 == 0 {
                &[120, 169, 255, 255]
            } else {
                &[16, 20, 26, 255]
            });
        }
    }
    encoder
        .write_header()
        .map_err(|error| VisualScenarioError::Workspace(error.to_string()))?
        .write_image_data(&pixels)
        .map_err(|error| VisualScenarioError::Workspace(error.to_string()))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use mandatum_scene::OverlayScene;

    use super::*;

    fn fixture(label: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let next = NEXT.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "mandatum-visual-scenario-{label}-{}-{next}",
            std::process::id()
        ))
    }

    #[test]
    fn catalog_has_exactly_the_stable_canonical_slugs() {
        assert_eq!(
            VisualScenarioId::ALL.map(VisualScenarioId::as_str),
            [
                "typography",
                "calm-terminal",
                "dense-workspace",
                "attention",
                "palette",
                "full-modal",
                "welcome",
                "context-menu",
                "artifacts",
                "narrow",
                "restored",
            ]
        );
        assert_eq!(VISUAL_SCENARIOS.len(), VisualScenarioId::ALL.len());
    }

    #[test]
    fn palette_recipe_reaches_typed_overlay_through_real_host() {
        let root = fixture("palette");
        let prepared = prepare_visual_scenario(VisualScenarioId::Palette, &root).unwrap();
        let mut host = FrontendHost::new(prepared.app_config());
        let frame = prepared
            .drive(&mut host, SceneSize::new(102, 35), Duration::from_secs(2))
            .unwrap();

        assert!(matches!(
            frame.scene.overlay,
            Some(OverlayScene::Palette(_))
        ));
        host.shutdown();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn review_snapshot_normalizes_isolated_fixture_identity() {
        let first_root = fixture("first");
        let second_root = fixture("second");
        let first_prepared =
            prepare_visual_scenario(VisualScenarioId::Palette, &first_root).unwrap();
        let second_prepared =
            prepare_visual_scenario(VisualScenarioId::Palette, &second_root).unwrap();
        let mut first_host = FrontendHost::new(first_prepared.app_config());
        let mut second_host = FrontendHost::new(second_prepared.app_config());
        let mut first = first_prepared
            .drive(
                &mut first_host,
                SceneSize::new(102, 35),
                Duration::from_secs(2),
            )
            .unwrap();
        let mut second = second_prepared
            .drive(
                &mut second_host,
                SceneSize::new(102, 35),
                Duration::from_secs(2),
            )
            .unwrap();

        first_prepared.stabilize_snapshot(&mut first).unwrap();
        second_prepared.stabilize_snapshot(&mut second).unwrap();
        assert_eq!(first.scene, second.scene);
        assert!(first.scene.header.text.contains("visual-project"));
        assert!(
            !first
                .scene
                .header
                .text
                .contains(&first_root.display().to_string())
        );

        first_host.shutdown();
        second_host.shutdown();
        let _ = fs::remove_dir_all(first_root);
        let _ = fs::remove_dir_all(second_root);
    }

    #[test]
    fn every_catalog_recipe_reaches_its_named_semantic_state() {
        for id in VisualScenarioId::ALL {
            let root = fixture(id.as_str());
            let prepared = prepare_visual_scenario(id, &root).unwrap();
            let mut host = FrontendHost::new(prepared.app_config());
            let frame = prepared
                .drive(&mut host, SceneSize::new(102, 35), Duration::from_secs(3))
                .unwrap_or_else(|error| {
                    let last = host.frame(SceneSize::new(102, 35));
                    let panes = last
                        .scene
                        .panes
                        .iter()
                        .map(|pane| format!("{}:{:?}", pane.id, pane.kind))
                        .collect::<Vec<_>>();
                    panic!(
                        "{id}: {error}; status={}; panes={panes:?}",
                        last.scene.status.text
                    )
                });

            match id {
                VisualScenarioId::Attention => {
                    assert!(frame.scene.header.attention.len() >= 2);
                }
                VisualScenarioId::Palette => {
                    assert!(matches!(
                        frame.scene.overlay,
                        Some(OverlayScene::Palette(_))
                    ));
                }
                VisualScenarioId::FullModal => {
                    assert!(matches!(
                        frame.scene.overlay,
                        Some(OverlayScene::Timeline(_))
                    ));
                }
                VisualScenarioId::Welcome => {
                    assert!(matches!(
                        frame.scene.overlay,
                        Some(OverlayScene::Welcome(_))
                    ));
                }
                VisualScenarioId::ContextMenu => {
                    assert!(matches!(
                        frame.scene.overlay,
                        Some(OverlayScene::ContextMenu(_))
                    ));
                }
                VisualScenarioId::Artifacts => {
                    assert_eq!(
                        frame
                            .scene
                            .panes
                            .iter()
                            .filter(|pane| matches!(pane.content, PaneContent::Artifact(_)))
                            .count(),
                        3
                    );
                }
                VisualScenarioId::Narrow => assert_eq!(frame.scene.panes.len(), 5),
                VisualScenarioId::Restored => {
                    assert!(frame.scene.status.text.contains("workspace restored"));
                }
                VisualScenarioId::Typography
                | VisualScenarioId::CalmTerminal
                | VisualScenarioId::DenseWorkspace => {}
            }

            host.shutdown();
            let _ = fs::remove_dir_all(root);
        }
    }
}
