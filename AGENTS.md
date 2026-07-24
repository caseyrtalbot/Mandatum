# AGENTS.md

Mandatum is a personal GPU-native development workstation with a terminal soul:
shells, tasks, servers, agents, approvals, artifacts, and recovery in one
spatial session surface. Reason from the code on disk and the current doc set.
Active specs describe current state; `docs/decisions.md` and `docs/history/`
preserve dated rationale and evidence.

## Product direction

The native wgpu frontend is the product and Casey's daily-driver target. The
terminal frontend is a maintained tool for SSH, headless use, recovery, and an
explicit escape hatch. The former Phase 7/8 admission framework is retired.
There is no public-release audience.
Sub-20 ms latency, paired percentage improvement, long soak, multi-display,
Linux-native, accessibility/theme parity, and rollout ceremony are not adoption
gates. Keep the probes as regression measurements; do not use them to resist
native polish.

Until the promotion change lands, native source still lives under
`spikes/frontend-wgpu`, `ci/gpu-spike.sh` retains its historical name, and
`ci/conformance.sh` retains the old dependency policy. Those are named
implementation gaps, not current product posture. The authoritative ordered
work is `docs/native-gpu-implementation-plan.md`.

## The gate

`./ci/gate.sh` is the single merge gate: fmt, clippy `-D warnings`, build,
test, `ci/conformance.sh`, `ci/doc-trace.sh`. GitHub Actions runs exactly this
script, so local and remote CI cannot drift. Native promotion must add the
renamed native gate as a stage inside `ci/gate.sh`, not create a second CI
authority. Red means the change does not land. Run it before claiming any
change complete; commits go directly to main, gated by a green run (solo repo,
see docs/decisions.md).

## The Constitution

Five immutable laws in `docs/constitution.md`, each enforced by an executable
gate. Violating one is a defect; `ci/doc-trace.sh` fails the build if any law
loses its documentation or its gate.

- L1 engine/frontend separation: frontend, parser, process, and async-runtime
  crates (ratatui, crossterm, vte, portable-pty, tokio, async-std, winit,
  wgpu, smol, mio) never appear in the dependency closure of engine-side
  crates (`mandatum-core`, `mandatum-commands`, `mandatum-scene`,
  `mandatum-agent-runtime`). Enforced by `ci/conformance.sh`.
- L2 `mandatum-core` is a runtime-free leaf: direct deps frozen to exactly
  `{serde, serde_json}`. If a feature needs more in core, the boundary is
  wrong, not the law.
- L3 durable intent is separate from live runtime: persistence stores intent
  only; events from replaced runtimes are rejected via the
  (restart generation, runtime token) stamp. `[L3-GATE]` tests.
- L4 terminal quality lives behind `TerminalAdapter`: no parser type leaks
  past the terminal engine. `[L4-GATE]` conformance tests plus the L1 scan.
- L5 terminal soul: bytes reach the focused child unless an explicit
  workspace control intercepts them (alt+pointer, copy mode, pane chrome).
  `[L5-GATE]` routing tests.

## Crate boundaries

- `mandatum-scene` owns the renderer-neutral contract: the `WorkspaceScene`
  output model, all pane-rect layout math (`scene::layout`), and the neutral
  input types (`scene::input`). It depends only on `mandatum-core`, serde, and
  pure Unicode segmentation/width policy crates.
  `&WorkspaceScene` alone must suffice to paint a frame; frontends never
  compute layout or derive chrome.
- `mandatum-renderer` is one frontend adapter: a single
  `render(frame, &scene, &theme)` entry. It must not depend on
  `mandatum-terminal-vt` (direct-dep ban in the conformance gate); the
  app-side `scene_builder` owns the grid-to-surface conversion.
- Inside `crates/app`, crossterm may appear only in `app_shell.rs` and
  `frontend.rs` (source-scan gate in `ci/conformance.sh`). Everything else,
  including `app_state` and all dispatch logic, consumes
  `mandatum_scene::input` values.
- No async runtime anywhere in the workspace: OS threads plus
  `std::sync::mpsc`, mirroring the PTY runtime. All runtime event streams
  (input, PTY, agent) feed one unified channel (`crates/app/src/events.rs`).
- Live runtime state (process handles, sessions, tokens, output tails,
  pending approval detail) lives behind the app's `RuntimeEngine` and is never
  serialized. The engine owns the terminal, task, and agent registries, the
  unified event channel, identity checks, replacement, reconciliation, and
  restore lifecycle facts. `AppState` folds accepted typed effects into core
  intent, the timeline, status, and presentation state.
- Experiments live in `spikes/`, outside the Cargo workspace. The current
  native frontend remains there only until the signed promotion work moves it
  into a production workspace package. Until then, run `./ci/gpu-spike.sh`
  after native or scene-contract changes.

## Test conventions

- Agent behavior is tested with `FakeConnector` (deterministic scripted
  flows, including pathological ones). No live model in the gate, ever.
- Tests that enforce a Constitution law carry the `[Lx-GATE]` tag in their
  name or comment; `ci/doc-trace.sh` requires at least one per law.
- PTY-harness pattern for runtime truth: liveness, flood, and scrollback
  behavior are proven against real PTYs (for example
  `pty_flood_stays_bounded_responsive_and_quittable` runs a live `yes` and
  asserts bounded memory and a timely quit).
- Latency regression: after any change to the run loop, input path, PTY event
  plumbing, or redraw policy, run the relevant procedure in
  `docs/verification.md`. The terminal key-to-app-output bar remains p50 well
  under 25 ms for regression detection; it is not a native adoption gate.
- Generated surfaces (help overlay, first-run note, glyph legends) are
  derived from live data with drift-failing completeness tests. Never
  hand-write key or glyph text; extend the source tables.
- Negative-test new gates: prove a conformance ban actually fails when the
  banned edge is reintroduced.

## Doc-sync duty

- Treat active-document drift as a defect. Update source-of-truth docs in the
  same work slice as the code or decision they describe; never knowingly leave
  the repo with stale implementation status, paths, interfaces, verification
  claims, or next-step guidance.
- Every judgment call lands as an entry in `docs/decisions.md` (status,
  decision, context, rationale, consequences, verification).
- `PLAN.md` points forward; `docs/decisions.md` points backward. Update both
  when an outcome ships or a deferral changes.
- `docs/verification.md` owns standing procedures (latency check, stranger
  test) plus dated one-line evidence; a claim in any doc must trace to a run
  that happened.
- Update `README.md` and `docs/repo-structure.md` when crates or the doc set
  change. Remove references to files that no longer exist.
- Do not let historical decisions or frozen spike evidence overwrite the active
  native-first direction.

## Capability-family completion protocol

Plan and review related variants as one capability family, implemented behind
one deep component. A tracer may use a focused RED/GREEN cycle, but do not run
the full documentation, displayed-smoke, aggregate-review, gate, handoff, and
commit lifecycle separately for every layout, overlay, input, or style variant.

When a capability family reaches its stop point and its focused tests are
green:

1. run one aggregate review over the complete family and correct the findings;
2. run one representative displayed scenario matrix when visual behavior
   changed;
3. update every affected source-of-truth document with only verified facts;
4. create or replace the project handoff with the verified stop point, any
   remaining unknowns, and one exact next capability family;
5. rerun `./ci/gate.sh` after the final repo documentation changes;
6. inspect `git diff --check` and `git status --short`; and
7. commit the code, tests, and synchronized repo documentation together.

A capability family is not complete while its docs or handoff are stale. Do
not defer doc sync or the handoff to a later family or commit.

## Done means

The artifact exists on disk, `./ci/gate.sh` is green (or the skipped steps
are explicitly scoped out), source-of-truth docs match the artifact, the next
handoff is current, remaining risks are named, and the completed slice is
committed. Never claim a verification that did not happen.
