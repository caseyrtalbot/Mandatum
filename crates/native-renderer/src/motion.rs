//! Pure stateful interpolation for typed native presentation transitions.
//!
//! Product state and transition eligibility arrive in the scene-owned plan.
//! This module retains only adapter-local visual progress. Callers inject the
//! monotonic instant, which keeps tests deterministic and keeps wall-clock
//! policy out of scene compilation.

use std::borrow::Cow;
use std::time::{Duration, Instant};

use mandatum_scene::{
    LogicalRect, PresentationNodeId, SceneMotionPolicy, TransitionProperty, TransitionRole,
    UiColor, UiCubicBezier, UiMotionToken,
};

use crate::{NativeMaterialRole, NativePlanCommand, NativePresentationPlan};

const PROGRESS_SCALE: i64 = 1_000_000;
const FRAME_INTERVAL: Duration = Duration::from_millis(8);
const ENTER_SCALE_THOUSANDTHS: i64 = 985;
const APPROVAL_SCALE_THOUSANDTHS: i64 = 985;
const EMPHASIS_START_OPACITY: i64 = 850_000;

/// Exact monotonic bounds of one active typed presentation role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActiveTransitionWindow {
    pub started_at: Instant,
    pub finishes_at: Instant,
}

impl ActiveTransitionWindow {
    pub fn duration(self) -> Duration {
        self.finishes_at.saturating_duration_since(self.started_at)
    }

    pub fn midpoint(self) -> Instant {
        self.started_at + Duration::from_secs_f64(self.duration().as_secs_f64() / 2.0)
    }
}

#[derive(Clone, Debug)]
struct ActiveNodeTransition {
    node_id: PresentationNodeId,
    role: TransitionRole,
    properties: Vec<TransitionProperty>,
    from: Vec<NativePlanCommand>,
    to: Vec<NativePlanCommand>,
    started_at: Instant,
    timing: UiMotionToken,
}

impl ActiveNodeTransition {
    fn finishes_at(&self) -> Instant {
        self.started_at + Duration::from_millis(u64::from(self.timing.duration_ms))
    }

    fn is_complete(&self, now: Instant) -> bool {
        self.timing.duration_ms == 0 || now >= self.finishes_at()
    }

    fn sampled_commands(&self, now: Instant) -> Vec<NativePlanCommand> {
        let progress = eased_progress(self.started_at, now, self.timing);
        interpolate_node_commands(&self.from, &self.to, &self.properties, self.role, progress)
    }
}

/// Adapter-local visual state over a sequence of pure presentation plans.
#[derive(Clone, Debug, Default)]
pub struct PresentationMotion {
    target: Option<NativePresentationPlan>,
    active: Vec<ActiveNodeTransition>,
    next_deadline: Option<Instant>,
    pointer_geometry_moving: bool,
}

impl PresentationMotion {
    /// Resolve the presentation plan at `now`.
    ///
    /// Equal target plans never restart active motion. Reduced motion snaps
    /// every property. Direct geometry cancels pane interpolation while
    /// leaving unrelated focus/selection/overlay emphasis eligible.
    ///
    /// An unchanged target with no active transitions borrows the caller's
    /// plan untouched: idle frames pay no plan clone. Only frames that
    /// actually interpolate return an owned rewrite.
    pub fn resolve<'a>(
        &mut self,
        next: &'a NativePresentationPlan,
        policy: SceneMotionPolicy,
        now: Instant,
    ) -> Cow<'a, NativePresentationPlan> {
        if policy.reduced_motion || policy.direct_geometry || self.target.is_none() {
            if self.target.as_ref() != Some(next) {
                self.target = Some(next.clone());
            }
            self.active.clear();
            self.finish_schedule();
            return Cow::Borrowed(next);
        }

        if self
            .target
            .as_ref()
            .is_some_and(|previous| previous != next)
        {
            let previous = self.target.take().expect("initial target handled above");
            self.retarget(&previous, next, now);
            self.target = Some(next.clone());
        }

        if self.active.is_empty() {
            // No transitions survive over an unchanged target: the settled
            // plan is the caller's plan, byte for byte.
            self.finish_schedule();
            return Cow::Borrowed(next);
        }

        Cow::Owned(self.sample(now))
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.next_deadline
    }

    pub fn is_active(&self) -> bool {
        !self.active.is_empty()
    }

    /// Pointer hit targets remain at stable scene geometry. The native shell
    /// uses this signal to suspend pointer admission while material geometry
    /// is between those stable endpoints.
    pub fn pointer_geometry_is_moving(&self) -> bool {
        self.pointer_geometry_moving
    }

    pub fn active_transition_window(&self, role: TransitionRole) -> Option<ActiveTransitionWindow> {
        let mut matching = self.active.iter().filter(|active| active.role == role);
        let first = matching.next()?;
        let mut window = ActiveTransitionWindow {
            started_at: first.started_at,
            finishes_at: first.finishes_at(),
        };
        for active in matching {
            window.started_at = window.started_at.min(active.started_at);
            window.finishes_at = window.finishes_at.max(active.finishes_at());
        }
        Some(window)
    }

    pub fn snap(&mut self) {
        self.active.clear();
        self.finish_schedule();
    }

    fn retarget(
        &mut self,
        previous: &NativePresentationPlan,
        next: &NativePresentationPlan,
        now: Instant,
    ) {
        let previous_visual = self.sample_without_scheduling(previous, now);
        let candidates = transition_candidates(previous, next);
        let emphasis_owners = candidates
            .iter()
            .filter(|candidate| {
                matches!(
                    candidate.role,
                    TransitionRole::Focus
                        | TransitionRole::Selection
                        | TransitionRole::ApprovalArrival
                ) && ((!has_transition(previous, &candidate.node_id, candidate.role)
                    && has_transition(next, &candidate.node_id, candidate.role))
                    || (candidate.role == TransitionRole::ApprovalArrival
                        && transition_sequence(previous, &candidate.node_id, candidate.role)
                            != transition_sequence(next, &candidate.node_id, candidate.role)
                        && has_transition(next, &candidate.node_id, candidate.role)))
            })
            .map(|candidate| candidate.node_id.clone())
            .collect::<Vec<_>>();

        for candidate in candidates {
            if candidate.role == TransitionRole::Overlay
                && has_transition(next, &candidate.node_id, TransitionRole::Overlay)
                && let Some(active) = self.active.iter_mut().find(|active| {
                    active.node_id == candidate.node_id && active.role == TransitionRole::Overlay
                })
            {
                // Stable selection changes during entry alter the node's
                // newest material endpoint. Preserve progress and the sampled
                // entrance origin, but converge on current semantic truth.
                active.to = commands_for_node(next, &candidate.node_id);
                continue;
            }
            if matches!(
                candidate.role,
                TransitionRole::Focus | TransitionRole::Selection
            ) && self.active.iter().any(|active| {
                active.node_id == candidate.node_id && active.role == TransitionRole::Overlay
            }) {
                continue;
            }
            if candidate.role == TransitionRole::PaneGeometry
                && emphasis_owners.contains(&candidate.node_id)
            {
                continue;
            }
            let from = commands_for_node(&previous_visual, &candidate.node_id);
            let to = commands_for_node(next, &candidate.node_id);
            let newly_eligible_emphasis =
                matches!(
                    candidate.role,
                    TransitionRole::Focus
                        | TransitionRole::Selection
                        | TransitionRole::ApprovalArrival
                ) && !has_transition(previous, &candidate.node_id, candidate.role)
                    && has_transition(next, &candidate.node_id, candidate.role);
            let repeated_approval_arrival = candidate.role == TransitionRole::ApprovalArrival
                && transition_sequence(previous, &candidate.node_id, candidate.role)
                    != transition_sequence(next, &candidate.node_id, candidate.role)
                && has_transition(next, &candidate.node_id, candidate.role);
            let newly_eligible_emphasis = newly_eligible_emphasis || repeated_approval_arrival;
            let exiting = to.is_empty() && !from.is_empty();
            let entering = from.is_empty() && !to.is_empty();
            let visually_changed = transition_changes_visual(
                &from,
                &to,
                &candidate.properties,
                newly_eligible_emphasis,
            );

            if candidate.role == TransitionRole::ApprovalArrival && !newly_eligible_emphasis {
                self.active
                    .retain(|active| active.node_id != candidate.node_id);
                continue;
            }

            // Overlay glyph rows are rebuilt from the current scene cell
            // program. Once the overlay closes those rows no longer exist, so
            // text exit stays direct. The overlay's materials and scrim are
            // adapter-frozen from the last sampled visual state and fade out
            // on the exit timing token; renderer-owned text retention is a
            // follow-up.
            if candidate.role == TransitionRole::Overlay && exiting {
                self.active
                    .retain(|active| active.node_id != candidate.node_id);
                let from_materials = from
                    .iter()
                    .filter(|command| matches!(command, NativePlanCommand::Material(_)))
                    .cloned()
                    .collect::<Vec<_>>();
                if from_materials.is_empty() || candidate.exit_timing.duration_ms == 0 {
                    continue;
                }
                self.active.push(ActiveNodeTransition {
                    node_id: candidate.node_id,
                    role: TransitionRole::Overlay,
                    properties: vec![TransitionProperty::Opacity],
                    from: from_materials,
                    to: Vec::new(),
                    started_at: now,
                    timing: candidate.exit_timing,
                });
                continue;
            }

            // Pane creation/removal has no safe terminal-content interpolation:
            // new/removed child pixels stay direct while surviving pane shells
            // may interpolate their shared stable identities. Do not cancel a
            // higher-salience track already established for the same node
            // (for example ApprovalArrival on a newly materialized callout).
            if candidate.role == TransitionRole::PaneGeometry && (from.is_empty() || to.is_empty())
            {
                self.active.retain(|active| {
                    active.node_id != candidate.node_id
                        || active.role != TransitionRole::PaneGeometry
                });
                continue;
            }

            // ApprovalArrival is a one-shot trigger. Its target intentionally
            // may remain eligible while the approval is unresolved; an equal
            // target plan does not re-enter this retargeting path.
            if !visually_changed {
                continue;
            }

            self.active
                .retain(|active| active.node_id != candidate.node_id);
            let timing = if exiting {
                candidate.exit_timing
            } else {
                candidate.timing
            };
            if timing.duration_ms == 0 {
                continue;
            }
            // Backdate entrances one frame interval so the frame rendered in
            // direct response to the triggering input samples visible
            // progress instead of alpha zero.
            let started_at = if entering {
                now.checked_sub(FRAME_INTERVAL).unwrap_or(now)
            } else {
                now
            };
            self.active.push(ActiveNodeTransition {
                node_id: candidate.node_id,
                role: candidate.role,
                properties: candidate.properties,
                from,
                to,
                started_at,
                timing,
            });
        }

        // If stable geometry changed without an eligible typed target, it is
        // direct and must cancel any stale geometry interpolation for that
        // node instead of continuing toward an obsolete endpoint.
        let active_nodes = self
            .active
            .iter()
            .map(|active| active.node_id.clone())
            .collect::<Vec<_>>();
        for node_id in active_nodes {
            let old = commands_for_node(previous, &node_id);
            let new = commands_for_node(next, &node_id);
            if old != new
                && !has_any_candidate_for_node(previous, next, &node_id)
                && let Some(index) = self
                    .active
                    .iter()
                    .position(|active| active.node_id == node_id)
            {
                self.active.remove(index);
            }
        }
    }

    fn sample(&mut self, now: Instant) -> NativePresentationPlan {
        self.active.retain(|active| !active.is_complete(now));
        let target = self
            .target
            .take()
            .expect("motion sampling requires a stable target");
        let resolved = self.sample_without_scheduling(&target, now);
        self.target = Some(target);
        // Only pane-geometry motion invalidates hit targets: panes really
        // move, so clicks against the previous layout would land wrong. An
        // overlay's entrance scale is cosmetic — its hit rects are already
        // final — so hover must keep tracking through it.
        self.pointer_geometry_moving = self
            .active
            .iter()
            .any(|active| active.role == TransitionRole::PaneGeometry);
        self.next_deadline = self
            .active
            .iter()
            .map(ActiveNodeTransition::finishes_at)
            .min()
            .map(|finish| (now + FRAME_INTERVAL).min(finish));
        resolved
    }

    fn sample_without_scheduling(
        &self,
        target: &NativePresentationPlan,
        now: Instant,
    ) -> NativePresentationPlan {
        let mut commands = target.commands().to_vec();
        for active in &self.active {
            replace_node_commands(&mut commands, &active.node_id, active.sampled_commands(now));
        }
        NativePresentationPlan::from_resolved_commands(commands, target.transitions().to_vec())
    }

    fn finish_schedule(&mut self) {
        self.next_deadline = None;
        self.pointer_geometry_moving = false;
    }
}

#[derive(Clone, Debug)]
struct TransitionCandidate {
    node_id: PresentationNodeId,
    role: TransitionRole,
    properties: Vec<TransitionProperty>,
    timing: UiMotionToken,
    exit_timing: UiMotionToken,
}

fn transition_candidates(
    previous: &NativePresentationPlan,
    next: &NativePresentationPlan,
) -> Vec<TransitionCandidate> {
    let mut candidates = Vec::<TransitionCandidate>::new();
    for transition in previous
        .transitions()
        .iter()
        .chain(next.transitions().iter())
    {
        if let Some(candidate) = candidates.iter_mut().find(|candidate| {
            candidate.node_id == transition.node_id && candidate.role == transition.role
        }) {
            if !candidate.properties.contains(&transition.property) {
                candidate.properties.push(transition.property);
            }
            if has_transition(next, &transition.node_id, transition.role) {
                candidate.timing = transition.timing;
                candidate.exit_timing = transition.exit_timing;
            }
        } else {
            candidates.push(TransitionCandidate {
                node_id: transition.node_id.clone(),
                role: transition.role,
                properties: vec![transition.property],
                timing: transition.timing,
                exit_timing: transition.exit_timing,
            });
        }
    }
    candidates
}

fn has_transition(
    plan: &NativePresentationPlan,
    node_id: &PresentationNodeId,
    role: TransitionRole,
) -> bool {
    plan.transitions()
        .iter()
        .any(|transition| &transition.node_id == node_id && transition.role == role)
}

fn transition_sequence(
    plan: &NativePresentationPlan,
    node_id: &PresentationNodeId,
    role: TransitionRole,
) -> Option<u64> {
    plan.transitions()
        .iter()
        .find(|transition| &transition.node_id == node_id && transition.role == role)
        .map(|transition| transition.sequence)
}

fn has_any_candidate_for_node(
    previous: &NativePresentationPlan,
    next: &NativePresentationPlan,
    node_id: &PresentationNodeId,
) -> bool {
    previous
        .transitions()
        .iter()
        .chain(next.transitions().iter())
        .any(|transition| &transition.node_id == node_id)
}

fn commands_for_node(
    plan: &NativePresentationPlan,
    node_id: &PresentationNodeId,
) -> Vec<NativePlanCommand> {
    plan.commands()
        .iter()
        .filter(|command| command_node_id(command) == node_id)
        .cloned()
        .collect()
}

fn command_node_id(command: &NativePlanCommand) -> &PresentationNodeId {
    match command {
        NativePlanCommand::BeginClip { node_id, .. }
        | NativePlanCommand::EndClip { node_id, .. } => node_id,
        NativePlanCommand::Material(material) => &material.node_id,
        NativePlanCommand::Text(text) => &text.node_id,
    }
}

fn replace_node_commands(
    commands: &mut Vec<NativePlanCommand>,
    node_id: &PresentationNodeId,
    replacement: Vec<NativePlanCommand>,
) {
    let insert_at = commands
        .iter()
        .position(|command| command_node_id(command) == node_id)
        .unwrap_or(commands.len());
    commands.retain(|command| command_node_id(command) != node_id);
    commands.splice(
        insert_at.min(commands.len())..insert_at.min(commands.len()),
        replacement,
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommandKey {
    Begin,
    Material(NativeMaterialRole),
    Text,
    End,
}

fn command_key(command: &NativePlanCommand) -> CommandKey {
    match command {
        NativePlanCommand::BeginClip { .. } => CommandKey::Begin,
        NativePlanCommand::Material(material) => CommandKey::Material(material.role),
        NativePlanCommand::Text(_) => CommandKey::Text,
        NativePlanCommand::EndClip { .. } => CommandKey::End,
    }
}

fn transition_changes_visual(
    from: &[NativePlanCommand],
    to: &[NativePlanCommand],
    properties: &[TransitionProperty],
    one_shot_trigger: bool,
) -> bool {
    (one_shot_trigger
        && properties
            .iter()
            .any(|property| property_has_rendered_command(from, to, *property)))
        || properties.iter().any(|property| match property {
            TransitionProperty::Geometry => geometry_commands_differ(from, to),
            TransitionProperty::Opacity => drawable_command_presence_differs(from, to),
            TransitionProperty::Scale => material_command_presence_differs(from, to),
        })
}

fn property_has_rendered_command(
    from: &[NativePlanCommand],
    to: &[NativePlanCommand],
    property: TransitionProperty,
) -> bool {
    from.iter().chain(to).any(|command| match property {
        TransitionProperty::Geometry | TransitionProperty::Scale => {
            matches!(command, NativePlanCommand::Material(_))
        }
        TransitionProperty::Opacity => matches!(
            command,
            NativePlanCommand::Material(_) | NativePlanCommand::Text(_)
        ),
    })
}

fn geometry_commands_differ(from: &[NativePlanCommand], to: &[NativePlanCommand]) -> bool {
    let mut keys = from
        .iter()
        .filter(|command| matches!(command, NativePlanCommand::Material(_)))
        .map(command_key)
        .collect::<Vec<_>>();
    for key in to
        .iter()
        .filter(|command| matches!(command, NativePlanCommand::Material(_)))
        .map(command_key)
    {
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys.into_iter().any(|key| {
        let from_command = from.iter().find(|command| command_key(command) == key);
        let to_command = to.iter().find(|command| command_key(command) == key);
        match (from_command, to_command) {
            (
                Some(NativePlanCommand::BeginClip { clip: from, .. }),
                Some(NativePlanCommand::BeginClip { clip: to, .. }),
            ) => from != to,
            (Some(NativePlanCommand::Material(from)), Some(NativePlanCommand::Material(to))) => {
                from.logical_rect != to.logical_rect || from.clip != to.clip
            }
            (None, None) => false,
            _ => true,
        }
    })
}

fn material_command_presence_differs(from: &[NativePlanCommand], to: &[NativePlanCommand]) -> bool {
    let from = from
        .iter()
        .any(|command| matches!(command, NativePlanCommand::Material(_)));
    let to = to
        .iter()
        .any(|command| matches!(command, NativePlanCommand::Material(_)));
    from != to
}

fn drawable_command_presence_differs(from: &[NativePlanCommand], to: &[NativePlanCommand]) -> bool {
    let mut keys = from
        .iter()
        .filter(|command| {
            matches!(
                command,
                NativePlanCommand::Material(_) | NativePlanCommand::Text(_)
            )
        })
        .map(command_key)
        .collect::<Vec<_>>();
    for key in to
        .iter()
        .filter(|command| {
            matches!(
                command,
                NativePlanCommand::Material(_) | NativePlanCommand::Text(_)
            )
        })
        .map(command_key)
    {
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys.into_iter().any(|key| {
        from.iter().any(|command| command_key(command) == key)
            != to.iter().any(|command| command_key(command) == key)
    })
}

fn interpolate_node_commands(
    from: &[NativePlanCommand],
    to: &[NativePlanCommand],
    properties: &[TransitionProperty],
    role: TransitionRole,
    progress: i64,
) -> Vec<NativePlanCommand> {
    let mut keys = to.iter().map(command_key).collect::<Vec<_>>();
    for key in from.iter().map(command_key) {
        if !keys.contains(&key) {
            keys.push(key);
        }
    }

    keys.into_iter()
        .filter_map(|key| {
            let from_command = from.iter().find(|command| command_key(command) == key);
            let to_command = to.iter().find(|command| command_key(command) == key);
            let mut command = to_command.or(from_command)?.clone();

            for property in properties {
                match property {
                    TransitionProperty::Geometry => {
                        interpolate_command_geometry(
                            &mut command,
                            from_command,
                            to_command,
                            progress,
                        );
                    }
                    TransitionProperty::Opacity => {
                        interpolate_command_opacity(
                            &mut command,
                            from_command.is_some(),
                            to_command.is_some(),
                            role,
                            progress,
                        );
                    }
                    TransitionProperty::Scale => {
                        interpolate_command_scale(
                            &mut command,
                            from_command.is_some(),
                            to_command.is_some(),
                            role,
                            progress,
                        );
                    }
                }
            }
            Some(command)
        })
        .collect()
}

fn interpolate_command_geometry(
    command: &mut NativePlanCommand,
    from: Option<&NativePlanCommand>,
    to: Option<&NativePlanCommand>,
    progress: i64,
) {
    if let (
        NativePlanCommand::Material(current),
        Some(NativePlanCommand::Material(start)),
        Some(NativePlanCommand::Material(end)),
    ) = (command, from, to)
    {
        current.logical_rect = lerp_rect(start.logical_rect, end.logical_rect, progress);
        current.clip = lerp_rect(start.clip, end.clip, progress);
    }
}

fn interpolate_command_opacity(
    command: &mut NativePlanCommand,
    exists_at_start: bool,
    exists_at_end: bool,
    role: TransitionRole,
    progress: i64,
) {
    let opacity = match (exists_at_start, exists_at_end) {
        (false, true) => progress,
        (true, false) => PROGRESS_SCALE - progress,
        (true, true)
            if matches!(
                role,
                TransitionRole::Focus | TransitionRole::Selection | TransitionRole::ApprovalArrival
            ) =>
        {
            lerp_i64(EMPHASIS_START_OPACITY, PROGRESS_SCALE, progress)
        }
        _ => PROGRESS_SCALE,
    };
    match command {
        NativePlanCommand::Material(material) => {
            material.color = with_opacity(material.color, opacity);
            if let Some(boundary) = &mut material.boundary {
                boundary.color = with_opacity(boundary.color, opacity);
            }
            if let Some(shadows) = &mut material.raised_shadows {
                for shadow in shadows {
                    shadow.color = with_opacity(shadow.color, opacity);
                }
            }
        }
        NativePlanCommand::Text(text) => {
            text.color = with_opacity(text.color, opacity);
        }
        NativePlanCommand::BeginClip { .. } | NativePlanCommand::EndClip { .. } => {}
    }
}

fn interpolate_command_scale(
    command: &mut NativePlanCommand,
    exists_at_start: bool,
    exists_at_end: bool,
    role: TransitionRole,
    progress: i64,
) {
    let start_scale = if role == TransitionRole::ApprovalArrival {
        APPROVAL_SCALE_THOUSANDTHS
    } else {
        ENTER_SCALE_THOUSANDTHS
    };
    let scale = match (exists_at_start, exists_at_end) {
        (false, true) => lerp_i64(start_scale, 1_000, progress),
        (true, false) => lerp_i64(1_000, ENTER_SCALE_THOUSANDTHS, progress),
        (true, true) if role == TransitionRole::ApprovalArrival => {
            lerp_i64(APPROVAL_SCALE_THOUSANDTHS, 1_000, progress)
        }
        _ => 1_000,
    };
    match command {
        NativePlanCommand::Material(material) => {
            if material.role != NativeMaterialRole::ModalScrim {
                material.logical_rect = scale_rect(material.logical_rect, scale);
            }
        }
        NativePlanCommand::Text(_) => {}
        NativePlanCommand::BeginClip { .. } | NativePlanCommand::EndClip { .. } => {}
    }
}

fn with_opacity(color: UiColor, opacity: i64) -> UiColor {
    UiColor::rgba(
        color.red,
        color.green,
        color.blue,
        lerp_i64(0, i64::from(color.alpha), opacity).clamp(0, 255) as u8,
    )
}

fn scale_rect(rect: LogicalRect, scale_thousandths: i64) -> LogicalRect {
    let width = ((i128::from(rect.size.width_units()) * i128::from(scale_thousandths) + 500)
        / 1_000)
        .clamp(0, i128::from(u64::MAX)) as u64;
    let height = ((i128::from(rect.size.height_units()) * i128::from(scale_thousandths) + 500)
        / 1_000)
        .clamp(0, i128::from(u64::MAX)) as u64;
    let dx = (i128::from(rect.size.width_units()) - i128::from(width)) / 2;
    let dy = (i128::from(rect.size.height_units()) - i128::from(height)) / 2;
    LogicalRect::from_units(
        (i128::from(rect.origin.x_units()) + dx).clamp(i128::from(i64::MIN), i128::from(i64::MAX))
            as i64,
        (i128::from(rect.origin.y_units()) + dy).clamp(i128::from(i64::MIN), i128::from(i64::MAX))
            as i64,
        width,
        height,
    )
}

fn lerp_rect(start: LogicalRect, end: LogicalRect, progress: i64) -> LogicalRect {
    LogicalRect::from_units(
        lerp_i64(start.origin.x_units(), end.origin.x_units(), progress),
        lerp_i64(start.origin.y_units(), end.origin.y_units(), progress),
        lerp_u64(start.size.width_units(), end.size.width_units(), progress),
        lerp_u64(start.size.height_units(), end.size.height_units(), progress),
    )
}

fn lerp_i64(start: i64, end: i64, progress: i64) -> i64 {
    let delta = i128::from(end) - i128::from(start);
    let scaled = delta * i128::from(progress);
    let rounded = if scaled < 0 {
        scaled - 500_000
    } else {
        scaled + 500_000
    };
    (i128::from(start) + rounded / 1_000_000).clamp(i128::from(i64::MIN), i128::from(i64::MAX))
        as i64
}

fn lerp_u64(start: u64, end: u64, progress: i64) -> u64 {
    let delta = i128::from(end) - i128::from(start);
    let scaled = delta * i128::from(progress);
    let rounded = if scaled < 0 {
        scaled - 500_000
    } else {
        scaled + 500_000
    };
    (i128::from(start) + rounded / 1_000_000).clamp(0, i128::from(u64::MAX)) as u64
}

fn eased_progress(start: Instant, now: Instant, timing: UiMotionToken) -> i64 {
    if timing.duration_ms == 0 {
        return PROGRESS_SCALE;
    }
    let elapsed = now.saturating_duration_since(start).as_nanos();
    let duration = Duration::from_millis(u64::from(timing.duration_ms)).as_nanos();
    let linear = ((elapsed.min(duration) * PROGRESS_SCALE as u128) / duration) as i64;
    match timing.easing {
        Some(curve) => cubic_bezier_progress(linear, curve),
        None => linear,
    }
}

fn cubic_bezier_progress(linear: i64, curve: UiCubicBezier) -> i64 {
    if linear <= 0 || linear >= PROGRESS_SCALE {
        return linear.clamp(0, PROGRESS_SCALE);
    }
    if curve.x1_thousandths == curve.y1_thousandths && curve.x2_thousandths == curve.y2_thousandths
    {
        return linear;
    }
    let mut low = 0i64;
    let mut high = PROGRESS_SCALE;
    for _ in 0..24 {
        let parameter = (low + high) / 2;
        if cubic_coordinate(
            parameter,
            i64::from(curve.x1_thousandths) * 1_000,
            i64::from(curve.x2_thousandths) * 1_000,
        ) < linear
        {
            low = parameter.saturating_add(1);
        } else {
            high = parameter;
        }
    }
    cubic_coordinate(
        high,
        i64::from(curve.y1_thousandths) * 1_000,
        i64::from(curve.y2_thousandths) * 1_000,
    )
    .clamp(0, PROGRESS_SCALE)
}

fn cubic_coordinate(parameter: i64, first: i64, second: i64) -> i64 {
    let p = i128::from(parameter);
    let inverse = i128::from(PROGRESS_SCALE - parameter);
    let scale = i128::from(PROGRESS_SCALE);
    let numerator = 3 * inverse * inverse * p * i128::from(first)
        + 3 * inverse * p * p * i128::from(second)
        + p * p * p * scale;
    (numerator / (scale * scale * scale)) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandatum_scene::{
        PresentationNodeId, SceneRect, TransitionRole, UiCubicBezier, WorkspaceNodePart,
    };

    use crate::{
        NativeFontFace, NativeMaterial, NativePresentationPlan, NativeTextMetricIdentity,
        NativeTextMetricRole, NativeTextScope, NativeTextStyle, NativeTransition,
    };

    fn material_plan(
        rect: LogicalRect,
        transitions: Vec<NativeTransition>,
    ) -> NativePresentationPlan {
        let node_id = PresentationNodeId::workspace(WorkspaceNodePart::Status);
        NativePresentationPlan::from_resolved_commands(
            vec![NativePlanCommand::Material(NativeMaterial {
                node_id,
                role: NativeMaterialRole::ChromeSurface,
                logical_rect: rect,
                clip: rect,
                color: UiColor::rgba(10, 20, 30, 200),
                corner_radius_units: 0,
                boundary: None,
                raised_shadows: None,
                z_order: 1,
            })],
            transitions,
        )
    }

    fn transition(
        role: TransitionRole,
        property: TransitionProperty,
        duration_ms: u16,
    ) -> NativeTransition {
        NativeTransition {
            node_id: PresentationNodeId::workspace(WorkspaceNodePart::Status),
            role,
            property,
            sequence: 0,
            timing: UiMotionToken::new(duration_ms, UiCubicBezier::new(0, 0, 1_000, 1_000)),
            exit_timing: UiMotionToken::new(duration_ms, UiCubicBezier::new(0, 0, 1_000, 1_000)),
        }
    }

    fn text_scope(rect: LogicalRect) -> NativePlanCommand {
        NativePlanCommand::Text(NativeTextScope {
            node_id: PresentationNodeId::workspace(WorkspaceNodePart::Status),
            logical_rect: rect,
            cell_rect: Some(SceneRect::new(0, 0, 10, 1)),
            clip: rect,
            color: UiColor::rgba(10, 20, 30, 200),
            metrics: NativeTextMetricIdentity {
                generation: 1,
                role: NativeTextMetricRole::Body,
                style: NativeTextStyle {
                    point_size_x64: 13 * 64,
                    line_height_units: 16 * 64,
                    face: NativeFontFace::Regular,
                },
            },
            z_order: 2,
        })
    }

    fn text_plan(rect: LogicalRect, transitions: Vec<NativeTransition>) -> NativePresentationPlan {
        NativePresentationPlan::from_resolved_commands(vec![text_scope(rect)], transitions)
    }

    fn material_and_text_plan(
        rect: LogicalRect,
        transitions: Vec<NativeTransition>,
    ) -> NativePresentationPlan {
        let mut commands = material_plan(rect, Vec::new()).commands().to_vec();
        commands.push(text_scope(rect));
        NativePresentationPlan::from_resolved_commands(commands, transitions)
    }

    fn overlay_item_plan(
        rect: LogicalRect,
        selected: bool,
        transitions: Vec<NativeTransition>,
    ) -> NativePresentationPlan {
        let mut commands = material_plan(rect, Vec::new()).commands().to_vec();
        if selected {
            commands.push(NativePlanCommand::Material(NativeMaterial {
                node_id: PresentationNodeId::workspace(WorkspaceNodePart::Status),
                role: NativeMaterialRole::Selection,
                logical_rect: rect,
                clip: rect,
                color: UiColor::rgba(40, 80, 120, 200),
                corner_radius_units: 0,
                boundary: None,
                raised_shadows: None,
                z_order: 2,
            }));
        }
        commands.push(text_scope(rect));
        NativePresentationPlan::from_resolved_commands(commands, transitions)
    }

    fn has_material(plan: &NativePresentationPlan, role: NativeMaterialRole) -> bool {
        plan.commands().iter().any(
            |command| matches!(command, NativePlanCommand::Material(value) if value.role == role),
        )
    }

    fn material_rect(plan: &NativePresentationPlan) -> LogicalRect {
        plan.commands()
            .iter()
            .find_map(|command| match command {
                NativePlanCommand::Material(material) => Some(material.logical_rect),
                _ => None,
            })
            .expect("material")
    }

    fn material_alpha(plan: &NativePresentationPlan) -> Option<u8> {
        plan.commands().iter().find_map(|command| match command {
            NativePlanCommand::Material(material) => Some(material.color.alpha),
            _ => None,
        })
    }

    fn text_rect(plan: &NativePresentationPlan) -> Option<LogicalRect> {
        plan.commands().iter().find_map(|command| match command {
            NativePlanCommand::Text(text) => Some(text.logical_rect),
            _ => None,
        })
    }

    fn text_alpha(plan: &NativePresentationPlan) -> Option<u8> {
        plan.commands().iter().find_map(|command| match command {
            NativePlanCommand::Text(text) => Some(text.color.alpha),
            _ => None,
        })
    }

    #[test]
    fn geometry_has_exact_start_midpoint_end_and_converges() {
        let origin = Instant::now();
        let start = LogicalRect::from_units(0, 0, 100, 100);
        let end = LogicalRect::from_units(100, 200, 300, 500);
        let spec = transition(
            TransitionRole::PaneGeometry,
            TransitionProperty::Geometry,
            100,
        );
        let mut motion = PresentationMotion::default();
        assert_eq!(
            material_rect(&motion.resolve(
                &material_plan(start, vec![spec.clone()]),
                SceneMotionPolicy::default(),
                origin,
            )),
            start
        );
        assert_eq!(
            material_rect(&motion.resolve(
                &material_plan(end, vec![spec]),
                SceneMotionPolicy::default(),
                origin,
            )),
            start
        );
        let target = motion.target.clone().unwrap();
        let midpoint = motion.resolve(
            &target,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(50),
        );
        assert_eq!(
            material_rect(&midpoint),
            LogicalRect::from_units(50, 100, 200, 300)
        );
        let final_plan = motion.resolve(
            &target,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(100),
        );
        assert_eq!(material_rect(&final_plan), end);
        assert!(!motion.is_active());
        assert_eq!(motion.next_deadline(), None);
    }

    #[test]
    fn text_only_geometry_and_scale_are_direct_and_schedule_no_frames() {
        let origin = Instant::now();
        let start = LogicalRect::from_units(0, 0, 100, 100);
        let end = LogicalRect::from_units(100, 200, 300, 500);
        let transitions = vec![
            transition(
                TransitionRole::PaneGeometry,
                TransitionProperty::Geometry,
                100,
            ),
            transition(TransitionRole::Selection, TransitionProperty::Scale, 100),
        ];
        let mut motion = PresentationMotion::default();
        motion.resolve(
            &text_plan(start, transitions.clone()),
            SceneMotionPolicy::default(),
            origin,
        );
        let end_plan = text_plan(end, transitions);
        let resolved = motion.resolve(
            &end_plan,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(1),
        );

        assert_eq!(text_rect(&resolved), Some(end));
        assert!(!motion.is_active());
        assert_eq!(motion.next_deadline(), None);
    }

    #[test]
    fn text_opacity_interpolates_while_cell_owned_placement_stays_direct() {
        let origin = Instant::now();
        let rect = LogicalRect::from_units(0, 0, 100, 100);
        let opacity = transition(TransitionRole::Overlay, TransitionProperty::Opacity, 100);
        let empty = NativePresentationPlan::from_resolved_commands(Vec::new(), Vec::new());
        let target = text_plan(rect, vec![opacity]);
        let mut motion = PresentationMotion::default();
        motion.resolve(&empty, SceneMotionPolicy::default(), origin);
        let start = motion.resolve(&target, SceneMotionPolicy::default(), origin);
        // Entrances are backdated one FRAME_INTERVAL (8ms of 100ms linear),
        // so the first frame samples 8% progress: alpha 200 * 0.08 = 16.
        assert_eq!(text_alpha(&start), Some(16));
        assert_eq!(text_rect(&start), Some(rect));

        let midpoint = motion.resolve(
            &target,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(50),
        );
        // 58ms of 100ms elapsed: alpha 200 * 0.58 = 116.
        assert_eq!(text_alpha(&midpoint), Some(116));
        assert_eq!(text_rect(&midpoint), Some(rect));

        let end = motion.resolve(
            &target,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(100),
        );
        assert_eq!(text_alpha(&end), Some(200));
        assert!(!motion.is_active());
    }

    #[test]
    fn mixed_material_and_text_geometry_moves_only_the_native_material() {
        let origin = Instant::now();
        let start = LogicalRect::from_units(0, 0, 100, 100);
        let end = LogicalRect::from_units(100, 200, 300, 500);
        let geometry = transition(
            TransitionRole::PaneGeometry,
            TransitionProperty::Geometry,
            100,
        );
        let mut motion = PresentationMotion::default();
        motion.resolve(
            &material_and_text_plan(start, vec![geometry.clone()]),
            SceneMotionPolicy::default(),
            origin,
        );
        motion.resolve(
            &material_and_text_plan(end, vec![geometry.clone()]),
            SceneMotionPolicy::default(),
            origin,
        );
        let end_plan = material_and_text_plan(end, vec![geometry]);
        let midpoint = motion.resolve(
            &end_plan,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(50),
        );

        assert_eq!(
            material_rect(&midpoint),
            LogicalRect::from_units(50, 100, 200, 300)
        );
        assert_eq!(text_rect(&midpoint), Some(end));
        assert!(motion.is_active());
    }

    #[test]
    fn equal_target_does_not_restart_and_direct_or_reduced_motion_snap() {
        let origin = Instant::now();
        let start = LogicalRect::from_units(0, 0, 100, 100);
        let end = LogicalRect::from_units(100, 0, 100, 100);
        let spec = transition(
            TransitionRole::PaneGeometry,
            TransitionProperty::Geometry,
            100,
        );
        let mut motion = PresentationMotion::default();
        motion.resolve(
            &material_plan(start, vec![spec.clone()]),
            SceneMotionPolicy::default(),
            origin,
        );
        let target = material_plan(end, vec![spec]);
        motion.resolve(&target, SceneMotionPolicy::default(), origin);
        let midpoint = motion.resolve(
            &target,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(50),
        );
        assert_eq!(material_rect(&midpoint).origin.x_units(), 50);
        let later = motion.resolve(
            &target,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(75),
        );
        assert_eq!(material_rect(&later).origin.x_units(), 75);

        let snap_plan = material_plan(LogicalRect::from_units(200, 0, 100, 100), Vec::new());
        let snapped = motion.resolve(
            &snap_plan,
            SceneMotionPolicy {
                direct_geometry: true,
                ..SceneMotionPolicy::default()
            },
            origin + Duration::from_millis(76),
        );
        assert_eq!(material_rect(&snapped).origin.x_units(), 200);
        assert!(!motion.pointer_geometry_is_moving());

        let reduced_plan = material_plan(LogicalRect::from_units(300, 0, 100, 100), Vec::new());
        let reduced = motion.resolve(
            &reduced_plan,
            SceneMotionPolicy {
                reduced_motion: true,
                direct_geometry: false,
            },
            origin + Duration::from_millis(77),
        );
        assert_eq!(material_rect(&reduced).origin.x_units(), 300);
        assert_eq!(motion.next_deadline(), None);
    }

    #[test]
    fn overlay_enter_finishes_at_exact_target_and_exit_fades_the_material() {
        let origin = Instant::now();
        let rect = LogicalRect::from_units(100, 100, 1_000, 800);
        let opacity = transition(TransitionRole::Overlay, TransitionProperty::Opacity, 100);
        let scale = transition(TransitionRole::Overlay, TransitionProperty::Scale, 100);
        let empty = NativePresentationPlan::from_resolved_commands(Vec::new(), Vec::new());
        let overlay = material_plan(rect, vec![opacity, scale]);
        let mut motion = PresentationMotion::default();
        motion.resolve(&empty, SceneMotionPolicy::default(), origin);

        let entered = motion.resolve(&overlay, SceneMotionPolicy::default(), origin);
        // Backdated entrance: 8ms of 100ms linear => alpha 200 * 0.08 = 16.
        assert_eq!(material_alpha(&entered), Some(16));
        assert!(material_rect(&entered).size.width_units() < rect.size.width_units());
        assert!(
            !motion.pointer_geometry_is_moving(),
            "an overlay's entrance scale is cosmetic: its hit rects are \
             already final, so hover keeps tracking through it"
        );

        let midpoint = motion.resolve(
            &overlay,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(50),
        );
        // 58ms of 100ms elapsed: alpha 200 * 0.58 = 116.
        assert_eq!(material_alpha(&midpoint), Some(116));

        let stable = motion.resolve(
            &overlay,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(100),
        );
        assert_eq!(*stable, overlay);
        assert!(!motion.is_active());

        // Exit fades the frozen material on the exit timing token, starting
        // from full alpha and unscaled geometry.
        let exit_started = motion.resolve(
            &empty,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(110),
        );
        assert_eq!(material_alpha(&exit_started), Some(200));
        assert_eq!(material_rect(&exit_started), rect);
        assert!(motion.is_active());
        assert!(motion.next_deadline().is_some());
        assert_eq!(
            motion.active_transition_window(TransitionRole::Overlay),
            Some(ActiveTransitionWindow {
                started_at: origin + Duration::from_millis(110),
                finishes_at: origin + Duration::from_millis(210),
            })
        );

        let exit_midpoint = motion.resolve(
            &empty,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(160),
        );
        // 50ms of the 100ms exit elapsed: alpha 200 * 0.5 = 100.
        assert_eq!(material_alpha(&exit_midpoint), Some(100));

        let exited = motion.resolve(
            &empty,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(210),
        );
        assert_eq!(*exited, empty);
        assert_eq!(material_alpha(&exited), None);
        assert!(!motion.is_active());
        assert_eq!(motion.next_deadline(), None);
    }

    #[test]
    fn overlay_exit_drops_text_immediately_and_fades_only_the_material() {
        let origin = Instant::now();
        let rect = LogicalRect::from_units(100, 100, 1_000, 800);
        let opacity = transition(TransitionRole::Overlay, TransitionProperty::Opacity, 100);
        let empty = NativePresentationPlan::from_resolved_commands(Vec::new(), Vec::new());
        let overlay = material_and_text_plan(rect, vec![opacity]);
        let mut motion = PresentationMotion::default();
        motion.resolve(&empty, SceneMotionPolicy::default(), origin);
        motion.resolve(&overlay, SceneMotionPolicy::default(), origin);
        motion.resolve(
            &overlay,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(100),
        );

        // Overlay glyph rows are rebuilt from the scene cell program and no
        // longer exist after close, so text exit is direct by design.
        let exit_started = motion.resolve(
            &empty,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(110),
        );
        assert_eq!(text_alpha(&exit_started), None);
        assert_eq!(material_alpha(&exit_started), Some(200));

        let exit_midpoint = motion.resolve(
            &empty,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(160),
        );
        assert_eq!(text_alpha(&exit_midpoint), None);
        assert_eq!(material_alpha(&exit_midpoint), Some(100));

        let exited = motion.resolve(
            &empty,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(210),
        );
        assert_eq!(*exited, empty);
        assert!(!motion.is_active());
        assert_eq!(motion.next_deadline(), None);
    }

    #[test]
    fn direct_geometry_suppresses_overlay_exit_and_all_followup_frames() {
        let origin = Instant::now();
        let rect = LogicalRect::from_units(100, 100, 1_000, 800);
        let opacity = transition(TransitionRole::Overlay, TransitionProperty::Opacity, 100);
        let scale = transition(TransitionRole::Overlay, TransitionProperty::Scale, 100);
        let empty = NativePresentationPlan::from_resolved_commands(Vec::new(), Vec::new());
        let overlay = material_plan(rect, vec![opacity, scale]);
        let mut motion = PresentationMotion::default();
        motion.resolve(&overlay, SceneMotionPolicy::default(), origin);

        let resolved = motion.resolve(
            &empty,
            SceneMotionPolicy {
                direct_geometry: true,
                ..SceneMotionPolicy::default()
            },
            origin + Duration::from_millis(1),
        );

        assert_eq!(*resolved, empty);
        assert!(!motion.is_active());
        assert_eq!(motion.next_deadline(), None);
        assert_eq!(
            motion.active_transition_window(TransitionRole::Overlay),
            None
        );
        assert!(!motion.pointer_geometry_is_moving());
    }

    #[test]
    fn closing_during_overlay_entry_fades_out_from_the_sampled_alpha() {
        let origin = Instant::now();
        let rect = LogicalRect::from_units(100, 100, 1_000, 800);
        let overlay = transition(TransitionRole::Overlay, TransitionProperty::Opacity, 180);
        let empty = NativePresentationPlan::from_resolved_commands(Vec::new(), Vec::new());
        let open = material_plan(rect, vec![overlay]);
        let mut motion = PresentationMotion::default();
        motion.resolve(&empty, SceneMotionPolicy::default(), origin);
        motion.resolve(&open, SceneMotionPolicy::default(), origin);

        // The exit fade resumes from the sampled entrance alpha, never the
        // full endpoint: 68ms of the backdated 180ms linear entrance elapsed,
        // so alpha = round(200 * 68/180) = 76.
        let closed = motion.resolve(
            &empty,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(60),
        );
        assert_eq!(material_alpha(&closed), Some(76));
        assert!(motion.is_active());
        assert_eq!(
            motion.active_transition_window(TransitionRole::Overlay),
            Some(ActiveTransitionWindow {
                started_at: origin + Duration::from_millis(60),
                finishes_at: origin + Duration::from_millis(240),
            })
        );

        let fade_midpoint = motion.resolve(
            &empty,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(150),
        );
        // Halfway through the 180ms exit: alpha = round(76 * 0.5) = 38.
        assert_eq!(material_alpha(&fade_midpoint), Some(38));

        let settled = motion.resolve(
            &empty,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(240),
        );
        assert_eq!(*settled, empty);
        assert!(!motion.is_active());
        assert_eq!(motion.next_deadline(), None);
        assert_eq!(
            motion.active_transition_window(TransitionRole::Overlay),
            None
        );
    }

    #[test]
    fn overlay_owns_presence_change_then_selection_owns_stable_emphasis() {
        let origin = Instant::now();
        let rect = LogicalRect::from_units(100, 100, 1_000, 800);
        let selection = transition(TransitionRole::Selection, TransitionProperty::Scale, 100);
        let overlay = transition(TransitionRole::Overlay, TransitionProperty::Scale, 180);
        let empty = NativePresentationPlan::from_resolved_commands(Vec::new(), Vec::new());
        let selected_overlay = material_plan(rect, vec![selection.clone(), overlay.clone()]);
        let stable_overlay = material_plan(rect, vec![overlay]);
        let mut motion = PresentationMotion::default();
        motion.resolve(&empty, SceneMotionPolicy::default(), origin);

        motion.resolve(&selected_overlay, SceneMotionPolicy::default(), origin);
        assert!(
            motion
                .active_transition_window(TransitionRole::Overlay)
                .is_some()
        );
        assert_eq!(
            motion.active_transition_window(TransitionRole::Selection),
            None,
            "overlay must own a selected row's presence-changing entrance"
        );

        motion.resolve(
            &selected_overlay,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(180),
        );
        motion.resolve(
            &stable_overlay,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(181),
        );
        motion.resolve(
            &selected_overlay,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(182),
        );
        assert_eq!(
            motion.active_transition_window(TransitionRole::Overlay),
            None
        );
        assert!(
            motion
                .active_transition_window(TransitionRole::Selection)
                .is_some(),
            "selection must own emphasis when overlay drawable presence is stable"
        );
    }

    #[test]
    fn selection_changes_refresh_active_overlay_endpoint_without_replacing_progress() {
        let origin = Instant::now();
        let rect = LogicalRect::from_units(100, 100, 1_000, 800);
        let overlay = transition(TransitionRole::Overlay, TransitionProperty::Opacity, 180);
        let selection = transition(TransitionRole::Selection, TransitionProperty::Scale, 120);
        let empty = NativePresentationPlan::from_resolved_commands(Vec::new(), Vec::new());
        let unselected = overlay_item_plan(rect, false, vec![overlay.clone()]);
        let selected = overlay_item_plan(rect, true, vec![selection.clone(), overlay.clone()]);
        let mut motion = PresentationMotion::default();
        motion.resolve(&empty, SceneMotionPolicy::default(), origin);
        motion.resolve(&unselected, SceneMotionPolicy::default(), origin);

        let down = motion.resolve(
            &selected,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(60),
        );
        assert!(has_material(&down, NativeMaterialRole::Selection));
        assert!(
            motion
                .active_transition_window(TransitionRole::Overlay)
                .is_some()
        );
        assert_eq!(
            motion.active_transition_window(TransitionRole::Selection),
            None
        );

        let up = motion.resolve(
            &unselected,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(90),
        );
        assert!(!has_material(&up, NativeMaterialRole::Selection));
        assert!(
            motion
                .active_transition_window(TransitionRole::Overlay)
                .is_some()
        );

        let settled = motion.resolve(
            &unselected,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(180),
        );
        assert_eq!(*settled, unselected);
        assert!(!has_material(&settled, NativeMaterialRole::Selection));
        assert!(!motion.is_active());
    }

    #[test]
    fn approval_arrival_owns_new_callout_presence_over_pane_geometry() {
        let origin = Instant::now();
        let rect = LogicalRect::from_units(100, 100, 1_000, 800);
        let empty = NativePresentationPlan::from_resolved_commands(Vec::new(), Vec::new());
        let arrival = material_plan(
            rect,
            vec![
                transition(
                    TransitionRole::ApprovalArrival,
                    TransitionProperty::Scale,
                    120,
                ),
                transition(
                    TransitionRole::PaneGeometry,
                    TransitionProperty::Geometry,
                    180,
                ),
            ],
        );
        let mut motion = PresentationMotion::default();
        motion.resolve(&empty, SceneMotionPolicy::default(), origin);
        motion.resolve(&arrival, SceneMotionPolicy::default(), origin);

        assert_eq!(
            motion.active_transition_window(TransitionRole::PaneGeometry),
            None
        );
        // The callout materializes from nothing, so its start is backdated
        // one frame interval like every entrance.
        assert_eq!(
            motion.active_transition_window(TransitionRole::ApprovalArrival),
            Some(ActiveTransitionWindow {
                started_at: origin - FRAME_INTERVAL,
                finishes_at: origin + Duration::from_millis(120) - FRAME_INTERVAL,
            })
        );
        let settled = motion.resolve(
            &arrival,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(120),
        );
        assert_eq!(
            *settled, arrival,
            "the end checkpoint must equal the stable arrival plan exactly"
        );
        assert!(!motion.is_active());
    }

    #[test]
    fn stable_overlay_resize_snaps_without_scheduling_noop_opacity_or_scale_frames() {
        let origin = Instant::now();
        let start = LogicalRect::from_units(100, 100, 1_000, 800);
        let resized = LogicalRect::from_units(80, 80, 1_200, 900);
        let transitions = vec![
            transition(TransitionRole::Overlay, TransitionProperty::Opacity, 180),
            transition(TransitionRole::Overlay, TransitionProperty::Scale, 180),
        ];
        let mut motion = PresentationMotion::default();
        motion.resolve(
            &material_plan(start, transitions.clone()),
            SceneMotionPolicy::default(),
            origin,
        );

        let target = material_plan(resized, transitions);
        let resolved = motion.resolve(
            &target,
            SceneMotionPolicy {
                direct_geometry: true,
                ..SceneMotionPolicy::default()
            },
            origin + Duration::from_millis(1),
        );

        assert_eq!(*resolved, target);
        assert!(!motion.is_active());
        assert_eq!(motion.next_deadline(), None);
        assert!(!motion.pointer_geometry_is_moving());
    }

    #[test]
    fn interrupted_geometry_reverses_from_the_sampled_position_and_converges() {
        let origin = Instant::now();
        let left = LogicalRect::from_units(0, 0, 100, 100);
        let right = LogicalRect::from_units(100, 0, 100, 100);
        let spec = transition(
            TransitionRole::PaneGeometry,
            TransitionProperty::Geometry,
            100,
        );
        let mut motion = PresentationMotion::default();
        motion.resolve(
            &material_plan(left, vec![spec.clone()]),
            SceneMotionPolicy::default(),
            origin,
        );
        motion.resolve(
            &material_plan(right, vec![spec.clone()]),
            SceneMotionPolicy::default(),
            origin,
        );
        let forward_target = motion.target.clone().unwrap();
        let forward = motion.resolve(
            &forward_target,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(40),
        );
        assert_eq!(material_rect(&forward).origin.x_units(), 40);

        let reversed_target = material_plan(left, vec![spec]);
        let reversal_start = motion.resolve(
            &reversed_target,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(40),
        );
        assert_eq!(material_rect(&reversal_start).origin.x_units(), 40);
        let reversal_midpoint = motion.resolve(
            &reversed_target,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(90),
        );
        assert_eq!(material_rect(&reversal_midpoint).origin.x_units(), 20);
        let converged = motion.resolve(
            &reversed_target,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(140),
        );
        assert_eq!(*converged, reversed_target);
    }

    #[test]
    fn approval_arrival_is_one_shot_while_eligible_and_removal_is_direct() {
        let origin = Instant::now();
        let rect = LogicalRect::from_units(100, 100, 1_000, 800);
        let stable = material_plan(rect, Vec::new());
        let arrival = material_plan(
            rect,
            vec![transition(
                TransitionRole::ApprovalArrival,
                TransitionProperty::Scale,
                100,
            )],
        );
        let mut motion = PresentationMotion::default();
        motion.resolve(&stable, SceneMotionPolicy::default(), origin);
        let start = motion.resolve(&arrival, SceneMotionPolicy::default(), origin);
        assert!(material_rect(&start).size.width_units() < rect.size.width_units());
        let approval_window = motion
            .active_transition_window(TransitionRole::ApprovalArrival)
            .expect("real approval transition window");
        assert_eq!(
            approval_window,
            ActiveTransitionWindow {
                started_at: origin,
                finishes_at: origin + Duration::from_millis(100),
            }
        );
        assert_eq!(
            approval_window.midpoint(),
            origin + Duration::from_millis(50)
        );

        let repeated_target = motion.target.clone().unwrap();
        let repeated = motion.resolve(
            &repeated_target,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(25),
        );
        assert!(material_rect(&repeated).size.width_units() < rect.size.width_units());
        assert_eq!(
            motion.active_transition_window(TransitionRole::ApprovalArrival),
            Some(ActiveTransitionWindow {
                started_at: origin,
                finishes_at: origin + Duration::from_millis(100),
            }),
            "persistent eligibility must not restart the one-shot"
        );

        // Resolution/removal is authoritative and cancels visual emphasis.
        let cleared = motion.resolve(
            &stable,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(50),
        );
        assert_eq!(*cleared, stable);
        assert!(!motion.is_active());
        assert_eq!(motion.next_deadline(), None);
    }

    #[test]
    fn hidden_approval_target_schedules_no_pixels_and_same_sequence_reveal_does_not_retrigger() {
        let origin = Instant::now();
        let rect = LogicalRect::from_units(100, 100, 1_000, 800);
        let mut approval = transition(
            TransitionRole::ApprovalArrival,
            TransitionProperty::Scale,
            100,
        );
        approval.sequence = 7;
        let stable = NativePresentationPlan::from_resolved_commands(Vec::new(), Vec::new());
        let hidden =
            NativePresentationPlan::from_resolved_commands(Vec::new(), vec![approval.clone()]);
        let visible = material_plan(rect, vec![approval]);
        let mut motion = PresentationMotion::default();
        motion.resolve(&stable, SceneMotionPolicy::default(), origin);

        let hidden_frame = motion.resolve(
            &hidden,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(1),
        );
        assert!(hidden_frame.commands().is_empty());
        assert!(!motion.is_active());
        assert_eq!(motion.next_deadline(), None);

        let revealed = motion.resolve(
            &visible,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(2),
        );
        assert_eq!(*revealed, visible);
        assert!(!motion.is_active());
        assert_eq!(motion.next_deadline(), None);
    }

    #[test]
    fn approval_sequence_change_restarts_emphasis_for_same_pane_and_role() {
        let origin = Instant::now();
        let rect = LogicalRect::from_units(100, 100, 1_000, 800);
        let mut first = transition(
            TransitionRole::ApprovalArrival,
            TransitionProperty::Scale,
            100,
        );
        first.sequence = 41;
        let mut second = first.clone();
        second.sequence = 42;
        let first_plan = material_plan(rect, vec![first]);
        let second_plan = material_plan(rect, vec![second]);
        let mut motion = PresentationMotion::default();
        motion.resolve(&first_plan, SceneMotionPolicy::default(), origin);

        let restarted = motion.resolve(
            &second_plan,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(10),
        );
        assert!(material_rect(&restarted).size.width_units() < rect.size.width_units());
        assert_eq!(
            motion.active_transition_window(TransitionRole::ApprovalArrival),
            Some(ActiveTransitionWindow {
                started_at: origin + Duration::from_millis(10),
                finishes_at: origin + Duration::from_millis(110),
            })
        );
    }

    /// P3(a) coverage proof: idle frames — an unchanged target with no active
    /// transitions — must borrow the caller's plan with zero clones, while
    /// frames that actually interpolate return an owned rewrite. The borrowed
    /// pointer identity is the allocation-freedom proof.
    #[test]
    fn resolve_borrows_idle_frames_and_owns_only_interpolated_ones() {
        let origin = Instant::now();
        let start = LogicalRect::from_units(0, 0, 100, 100);
        let end = LogicalRect::from_units(100, 0, 100, 100);
        let spec = transition(
            TransitionRole::PaneGeometry,
            TransitionProperty::Geometry,
            100,
        );
        let start_plan = material_plan(start, vec![spec.clone()]);
        let end_plan = material_plan(end, vec![spec]);
        let mut motion = PresentationMotion::default();

        let initial = motion.resolve(&start_plan, SceneMotionPolicy::default(), origin);
        assert!(
            matches!(initial, Cow::Borrowed(plan) if std::ptr::eq(plan, &start_plan)),
            "the initial snap must return the caller's plan uncloned"
        );

        let idle = motion.resolve(
            &start_plan,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(1),
        );
        assert!(
            matches!(idle, Cow::Borrowed(plan) if std::ptr::eq(plan, &start_plan)),
            "an unchanged motionless target must take the borrowed fast path"
        );
        assert_eq!(motion.next_deadline(), None);

        let interpolated = motion.resolve(
            &end_plan,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(1),
        );
        assert!(
            matches!(interpolated, Cow::Owned(_)),
            "active interpolation must return an owned rewrite"
        );
        assert!(motion.is_active());

        let finishing = motion.resolve(
            &end_plan,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(200),
        );
        assert_eq!(
            *finishing, end_plan,
            "the completion frame still resolves to the exact target"
        );
        let settled = motion.resolve(
            &end_plan,
            SceneMotionPolicy::default(),
            origin + Duration::from_millis(201),
        );
        assert!(
            matches!(settled, Cow::Borrowed(plan) if std::ptr::eq(plan, &end_plan)),
            "after completion the borrowed fast path must resume"
        );
        assert!(!motion.is_active());
    }
}
