Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: deadline__logs.queue-without-role-read-credentials-rejected
Issue-Key: deadline__logs.queue-without-role-read-credentials-rejected|run#16|AssertionError|assert got == expected, f"role-less queue answered {got!r}"
Issue-Commit: 4621a6ee7f1fe7bbce8e80b2d15ffa4da92b4e77

# Implement deadline AssumeQueueRoleForRead: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `4621a6ee7f1fe7bbce8e80b2d15ffa4da92b4e77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssumeQueueRoleForRead

## Description
`AssumeQueueRoleForRead` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-deadline-deadline/src/ops/assume_queue_role_for_read.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `AssumeQueueRoleForRead` call, so each one unblocks only when `AssumeQueueRoleForRead` works: `deadline__logs.queue-without-role-read-credentials-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-deadline-deadline/src/ops/assume_queue_role_for_read.rs`. Replaying the corpus, the emulator refused the `AssumeQueueRoleForRead` call: File "run.py", line 461, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_deadline__logs_queue_without_role_read_credentials_rejected.py", line 166, in run
    assert got == expected, f"role-less queue answered {got!r}"
           ^^^^^^^^^^^^^^^
AssertionError: role-less queue answered ('NotImplemented', 'operation AssumeQueueRoleForRead is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the deadline emulator
Call AssumeQueueRoleForRead and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
