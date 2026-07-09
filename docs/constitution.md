# The Constitution

Five laws govern Mandatum. They are immutable: violating any of them is a
defect, and each is enforced by an executable CI gate, not by intention.
`ci/doc-trace.sh` fails the build if any law loses its documentation or its
gate.

## L1 — Engine/Frontend Separation

Product behavior lives in engines and the scene model, never in a frontend.
Terminal, native, and GPU frontends must all be possible against the same
model. Frontend, parser, process, and async-runtime crates never appear in
the dependency closure of engine-side crates.

Gate: `ci/conformance.sh` (`[L1-GATE]`) — forbidden-crate scan over the
transitive dependency closure of engine-side crates.

## L2 — `core` Is a Runtime-Free Leaf

`mandatum-core` depends on serde only. Never process handles, parsers,
threads, render resources, frontend types, network clients, or async
runtimes. If a feature needs more in core, the boundary is wrong, not the
law.

Gate: `ci/conformance.sh` (`[L2-GATE]`) — fails if core's direct dependency
set is anything but `{serde, serde_json}`.

## L3 — Durable Intent Is Separate From Live Runtime

Persistence stores intent, objective, thread reference, last summary, and
durable status only. Live handles and streams are never durable truth.
Events from a replaced runtime are rejected.

Gate: `[L3-GATE]`-tagged tests — saved-state exclusion and
replaced-runtime event-rejection tests.

## L4 — Terminal Quality Lives Behind `TerminalAdapter`

Terminal parser/backend choices stay behind the terminal engine interface.
Backend swaps require conformance tests; no parser type leaks into core.

Gate: `[L4-GATE]`-tagged adapter conformance tests, plus the L1 dependency
scan (vte is a forbidden crate for engine-side code).

## L5 — Terminal Soul

The workspace never steals input from a child terminal except through
explicit workspace control. If a child app requests mouse capture, the
workspace honors it until the user invokes workspace-level control.

Gate: `[L5-GATE]`-tagged input-routing tests — bytes reach the focused
child unless an explicit workspace-control binding intercepts them.

## Non-Goals

Not before the workstation loop is strong: a general source editor, a PM
board, a chat-first agent UI, a decorative dashboard hiding raw output, a
plugin marketplace, cloud collaboration.
