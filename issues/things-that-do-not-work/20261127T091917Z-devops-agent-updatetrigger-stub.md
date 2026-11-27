Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: devops-agent.update-trigger-rejects-unknown-goal-id-rejected
Issue-Shape-Id: com.amazonaws.devopsagent#DevOpsAgent
Issue-Key: devops-agent.update-trigger-rejects-unknown-goal-id-rejected|run#6|AssertionError|assert (code, message) == ("ValidationException", _GOAL_PATTERN_MESSAGE), (
Issue-Commit: 4621a6ee7f1fe7bbce8e80b2d15ffa4da92b4e77

# Implement devops-agent UpdateTrigger: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `4621a6ee7f1fe7bbce8e80b2d15ffa4da92b4e77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateTrigger

## Description
`UpdateTrigger` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-devopsagent-devopsagent/src/ops/update_trigger.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `UpdateTrigger` call, so each one unblocks only when `UpdateTrigger` works: `devops-agent.update-trigger-rejects-unknown-goal-id-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-devopsagent-devopsagent/src/ops/update_trigger.rs`. Replaying the corpus, the emulator refused the `UpdateTrigger` call: File "run.py", line 461, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_devops_agent_update_trigger_rejects_unknown_goal_id_rejected.py", line 66, in run
    assert (code, message) == ("ValidationException", _GOAL_PATTERN_MESSAGE), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: a non-goal trigger id answered 'NotImplemented': 'operation UpdateTrigger is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the devops-agent emulator
Call UpdateTrigger and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
