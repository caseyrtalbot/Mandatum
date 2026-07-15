# Roadmap

All six execution gates below are complete; each landed behind a green
`./ci/gate.sh` (commit hashes in PLAN.md, "Shipped"). This file records what
each gate delivered and where its proof lives, then the forward horizon.
PLAN.md is the canonical forward plan; on divergence, PLAN.md wins.

## Gate 1: Documentation Source Of Truth: DONE

Outcome: agents can read the docs and build toward the workstation vision
without contradictory instructions.

Delivered: the docs/ spec series (product principles, architecture,
frontend platform, terminal engine, rendering, agent runtime, interaction,
workflows, verification, repo structure, decisions) plus the Constitution
(docs/constitution.md) with executable gates: `ci/doc-trace.sh` fails the
build if any law loses its documentation or its gate.

## Gate 2: Runtime Decomposition: DONE

Outcome: live runtime responsibilities are isolated behind clear modules.

Delivered: terminal runtime registry (`terminal_runtime.rs`), task runtime
registry (`task_runtime.rs`), agent runtime registry (`agent_runtime.rs`),
process event router with flow-credit backpressure (`process_events.rs`),
persistence coordinator (`persistence.rs`), input router (`input.rs`), and
the app shell orchestrator (`app_shell.rs`), all under `crates/app`.
`core` remains a runtime-free leaf, enforced by the L2 gate in
`ci/conformance.sh`.

## Gate 3: Scene Contract: DONE

Outcome: frontends consume a renderer-neutral scene.

Delivered: `mandatum-scene` owns `WorkspaceScene` (pane bounds, terminal/
task/agent surfaces, palette and overlay views, header/status, hit
targets), all pane-rect layout math, and the neutral input types. The
ratatui renderer is one adapter; a test-only plain-text frontend renders
the same scenes (`crates/app/tests/frontend_parity.rs`). The L1 gate bans a
direct renderer -> terminal-engine dependency.

## Gate 4: Frontend Platform Spike: DONE

Outcome: decide whether a native/GPU frontend materially improves the
product.

Delivered: the winit+wgpu spike (`spikes/frontend-wgpu`) rendered a live
PTY with typing, paste, resize, scrollback, selection, and a status strip,
measured latency and frame pacing, and bound to the scene contract.
Verdict: measurable quality gain proven, terminal frontend stays v1
(docs/frontend-platform.md carries the decision record and numbers;
evidence in `spikes/frontend-wgpu/RESULTS.md`).

## Gate 5: Workstation Visibility Slice: DONE

Outcome: the product can supervise a real development session.

Delivered: multiple terminal panes, task panes with timeline history, a
dev-server stand-in, agent panes, the session map, the execution timeline,
and the header attention strip. Validated by the stranger test
(docs/verification.md, "The Stranger Test") over the driven demo
`examples/live-slice/run.sh`.

## Gate 6: Brilliance Pass: DONE

Outcome: the experience feels exceptional under real load.

Delivered: event-driven main loop (key-to-bytes-out p50 42.6 ms -> 13.3 ms,
docs/verification.md "Input Latency Regression Check"), PTY flood
backpressure (bounded memory, quittable under `yes`; test
`pty_flood_stays_bounded_responsive_and_quittable`), session output search,
generated help/first-run/legend surfaces, calm failure states, and the
accessibility floor (keyboard-only completeness, reduced motion, visible
focus) in docs/interaction-model.md.

## Post-Charter Runtime Engine Deepening: DONE

Outcome: terminal, task, and agent lifecycle policy has one deep Module and a
narrow, product-shaped Interface.

Delivered: `crates/app/src/runtime_engine.rs` owns all three live registries,
the unified event channel, runtime tokens, identity checks, reconciliation,
replacement, approval control, child-event application, shutdown, and
transactional restore. It returns typed effects to `AppState` and typed
lifecycle facts describing fresh, deferred, detached, and not-replayed
outcomes. Restore staging failures return a typed error and commit no facts.
Concrete registry mutation remains inside the Module; durable workspace intent
and presentation/timeline folds remain outside it.

## Next Horizon

Matches PLAN.md ("Next horizon"); see it for detail:

- Named task and dev-server recipe catalog with durable run facts.
- Recovery cockpit with itemized restore outcomes and failure acknowledgement.
- Capability-described connector catalog plus a scriptable project/session/
  recipe control surface and explicit approval profiles.
- GPU adapter, when the roadmap needs GPU-only capability or a sub-20 ms
  end-to-end latency goal (known remaining work listed in PLAN.md).
- Rewrap on resize, in the `mandatum-terminal-vt` grid if adopted.
- Damage tracking, when profiling says per-frame conversion costs too much.
- Minors: `approval_history` growth cap, ratatui bump, idle-heartbeat
  repaint scope.
