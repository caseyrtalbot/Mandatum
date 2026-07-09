# Agent Runtime

## Product Role

Agents are session actors. They run work, request approvals, change files, run
commands, surface verification, and produce handoffs.

An agent pane is not a chat sidebar. It is a work surface that makes an
agent's state inspectable in the same spatial model as terminals and tasks.

## Agent Actor Model

Each agent actor has:

- actor id
- objective
- workspace/project scope
- current state
- latest action
- pending approvals
- changed files
- commands run
- verification results
- blocker summary
- latest message summary
- handoff output
- external thread reference when applicable

## Agent States

Required states:

- draft
- queued
- running
- waiting for approval
- blocked
- failed
- complete
- stopped

States must be visible in the pane, session map, and command palette context.

## Agent Pane Requirements

An agent pane should show:

- objective
- state
- current action
- last update time
- pending approval count
- changed-file count
- latest verification result
- latest summary
- commands for open, approve, reject, stop, restart, and handoff

The pane should be useful at a glance and expandable for detail.

## Approval Surface

Approvals must be first-class:

- pending approval visible in status strip
- approval tied to agent actor
- approval shows command, scope, and risk
- user can approve, reject, or inspect
- approved/rejected decisions remain in execution history

## Changed Files Surface

Agents should report:

- files added
- files modified
- files deleted
- tests run
- checks failed
- checks not run

The user should be able to jump from an agent pane to a changed-file summary.

## Runtime Boundaries

The runtime engine owns:

- live agent process/thread handles
- approval wait state
- external connector state
- log streaming
- runtime errors

The workspace engine persists:

- agent pane intent
- objective
- thread reference
- last known summary
- durable status summary

Live handles and transient output streams are not durable truth.

## Commands

Required command vocabulary:

- Start Agent
- Stop Agent
- Open Agent Thread
- Show Agent Changes
- Show Agent Checks
- Approve Agent Action
- Reject Agent Action
- Write Agent Handoff
- Pin Agent Pane
- Focus Next Waiting Agent

## Verification

Agent runtime work should prove:

- agent pane can be created from durable intent
- running state updates the pane
- pending approval is visible globally
- changed files are summarized
- verification results are attached
- restore preserves intent without inventing live runtime state
