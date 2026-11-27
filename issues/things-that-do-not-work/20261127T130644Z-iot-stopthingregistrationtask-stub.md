Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.stop-completed-thing-registration-task-rejected
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot.stop-completed-thing-registration-task-rejected|run#28|AssertionError|assert (code, message) == ("InvalidRequestException", "Not Able to cancel"), (
Issue-Commit: eef1405ac0806ef267777bba6aefe2b3b0754197

# Implement iot StopThingRegistrationTask: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `eef1405ac0806ef267777bba6aefe2b3b0754197`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopThingRegistrationTask

## Description
`StopThingRegistrationTask` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iot-awsiotservice/src/ops/stop_thing_registration_task.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `StopThingRegistrationTask` call, so each one unblocks only when `StopThingRegistrationTask` works: `iot.stop-completed-thing-registration-task-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iot-awsiotservice/src/ops/stop_thing_registration_task.rs`. Replaying the corpus, the emulator refused the `StopThingRegistrationTask` call: File "run.py", line 461, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_iot_stop_completed_thing_registration_task_rejected.py", line 212, in run
    assert (code, message) == ("InvalidRequestException", "Not Able to cancel"), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: stopping the completed task gave 'NotImplemented': 'operation StopThingRegistrationTask is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the iot emulator
Call StopThingRegistrationTask and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
