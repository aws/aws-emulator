Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: devops-agent.delete-trigger-of-unknown-goal-id-succeeds-without-effect devops-agent.list-goals-reflects-goal-schedule-disabled-by-update-goal devops-agent.list-goals-rejects-unissued-next-token devops-agent.update-goal-disabled-schedule-reflected-in-list-goals-filters devops-agent.update-goal-rejects-unknown-and-malformed-goal-ids devops-agent.update-goal-rejects-unknown-goal-id-and-leaves-space-goal-unchanged devops-agent.update-goal-rejects-unknown-goal-id-rejected
Issue-Shape-Id: com.amazonaws.devopsagent#DevOpsAgent
Issue-Key: devops-agent|ListGoals|NotImplemented
Issue-Commit: 4621a6ee7f1fe7bbce8e80b2d15ffa4da92b4e77

# Implement devops-agent ListGoals: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `4621a6ee7f1fe7bbce8e80b2d15ffa4da92b4e77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListGoals

## Description
`ListGoals` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-devopsagent-devopsagent/src/ops/list_goals.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `ListGoals` call, so each one unblocks only when `ListGoals` works: `devops-agent.delete-trigger-of-unknown-goal-id-succeeds-without-effect`, `devops-agent.list-goals-reflects-goal-schedule-disabled-by-update-goal`, `devops-agent.list-goals-rejects-unissued-next-token`, `devops-agent.update-goal-disabled-schedule-reflected-in-list-goals-filters`, `devops-agent.update-goal-rejects-unknown-and-malformed-goal-ids`, `devops-agent.update-goal-rejects-unknown-goal-id-and-leaves-space-goal-unchanged`, `devops-agent.update-goal-rejects-unknown-goal-id-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-devopsagent-devopsagent/src/ops/list_goals.rs`. Replaying the corpus, the emulator refused the `ListGoals` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListGoals operation: operation ListGoals is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the devops-agent emulator
Call ListGoals and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
