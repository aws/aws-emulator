Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling.delete-notification-config-nonexistent-rejected autoscaling.notification-configuration-lifecycle
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling.delete-notification-config-nonexistent-rejected|run#17|AssertionError|assert error.get("Code") == "ValidationError", (
Issue-Key: auto-scaling|DeleteNotificationConfiguration|NotImplemented
Issue-Commit: 7011ad447146056dadde7fecb62590edf9d79855

# Implement autoscaling DeleteNotificationConfiguration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `7011ad447146056dadde7fecb62590edf9d79855`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteNotificationConfiguration

## Description
`DeleteNotificationConfiguration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/delete_notification_configuration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `DeleteNotificationConfiguration` call, so each one unblocks only when `DeleteNotificationConfiguration` works: `autoscaling.delete-notification-config-nonexistent-rejected`, `autoscaling.notification-configuration-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/delete_notification_configuration.rs`. Replaying the corpus, the emulator refused the `DeleteNotificationConfiguration` call: File "run.py", line 445, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_autoscaling_delete_notification_config_nonexistent_rejected.py", line 68, in run
    assert error.get("Code") == "ValidationError", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ValidationError for a never-configured topic, got {'Type': 'Receiver', 'Code': 'NotImplemented', 'Message': 'operation DeleteNotificationConfiguration is scaffolded but not implemented in this emulator'}

## Steps to Reproduce
Boot the autoscaling emulator
Call DeleteNotificationConfiguration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
