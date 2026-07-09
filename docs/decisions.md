# Decisions

## Format

Each decision should record:

- status: proposed, accepted, or rejected
- decision
- context
- rationale
- consequences
- verification impact

## Accepted: Engine And Frontend Separation

Status: accepted

Decision: Mandatum is structured as a workstation engine, runtime engine,
terminal engine, scene layer, workflow layer, command layer, and frontend
adapters.

Rationale: This keeps durable product behavior testable and lets the product
support terminal, native, GPU-backed, or platform-specific frontends without
duplicating session logic.

Consequences:

- frontend adapters render scenes and emit input
- product behavior belongs in engine/runtime modules
- `core` remains free of runtime, parser, and frontend dependencies
- scene types become the central interface for presentation

Verification:

- architecture boundary scans
- scene/frontend tests once scene types exist

## Accepted: Durable Intent Is Separate From Live Runtime

Status: accepted

Decision: Workspace persistence stores durable intent only. Live PTYs, parser
instances, process handles, runtime tokens, thread handles, output buffers, and
frontend resources are runtime state.

Rationale: Durable state must survive restarts without pretending that live
processes can be serialized.

Consequences:

- restore can recreate useful layout and command intent
- side-effecting work requires explicit relaunch policy
- events from replaced runtimes must be rejected

Verification:

- saved JSON exclusion tests
- restore transaction tests
- replaced-runtime event rejection tests

## Accepted: Agents Are Session Actors

Status: accepted

Decision: Agents are represented as session actors with objective, state,
approvals, changed files, commands, checks, blockers, and handoff data.

Rationale: Agent state should be visible alongside terminals and tasks without
turning the product into a chat-first surface.

Consequences:

- agent panes need compact state, detail expansion, and global attention signals
- approvals are first-class runtime events
- changed files and checks attach to the agent actor

Verification:

- agent pane state tests
- approval attention tests
- restore-with-agent-intent tests

## Accepted: Terminal Quality Lives Behind The Terminal Engine

Status: accepted

Decision: Terminal parser/backend choices stay behind the terminal engine
interface.

Rationale: Terminal correctness matters, but the workstation product should not
inherit another terminal emulator's application architecture.

Consequences:

- backend swaps require conformance tests
- parser details do not leak into `core`
- frontend adapters consume snapshots, not parser internals

Verification:

- terminal conformance suite
- backend fixture parity
- dependency boundary checks

## Accepted: Apache-2.0 License

Status: accepted (2026-07-09)

Decision: The repository is licensed Apache-2.0.

Rationale: Standard permissive license for the Rust ecosystem with an
explicit patent grant. The repo is pre-release; relicensing before any
public release remains possible, so this is a low-cost reversible default.

Consequences: LICENSE at repo root; contributions inherit it.

## Accepted: One Gate Script For Local And Remote CI

Status: accepted (2026-07-09)

Decision: `ci/gate.sh` is the single source of truth for the merge gate
(fmt, clippy -D warnings, build, test, conformance, doc-trace). GitHub
Actions (`.github/workflows/ci.yml`) runs exactly that script.

Rationale: Local runs and CI cannot drift if they execute the same script.
Constitution laws are executable gates: L1/L2 as dependency scans
(`ci/conformance.sh`), L3/L4/L5 as `[Lx-GATE]`-tagged tests, and
`ci/doc-trace.sh` fails the build if any law loses its docs or its gate.

Consequences: a merge that reddens a conformance gate does not land.

## Accepted: Commit Directly To main

Status: accepted (2026-07-09)

Decision: This solo repository commits directly to main, gated by
`ci/gate.sh` before each push, matching the repo's existing history.

Rationale: No collaborators; the gate script provides the protection a PR
flow would. Revisit when a second contributor appears.
