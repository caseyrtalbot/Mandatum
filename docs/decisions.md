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
