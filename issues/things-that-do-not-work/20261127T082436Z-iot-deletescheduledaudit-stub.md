Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.scheduled-audit-lifecycle-triggers-audit-task
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|DeleteScheduledAudit|NotImplemented
Issue-Commit: 8faad1de300cdf3d9bfb0403786f5996fdab75da

# Implement iot DeleteScheduledAudit: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `8faad1de300cdf3d9bfb0403786f5996fdab75da`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteScheduledAudit

## Description
`DeleteScheduledAudit` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iot-awsiotservice/src/ops/delete_scheduled_audit.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteScheduledAudit` call, so each one unblocks only when `DeleteScheduledAudit` works: `iot.scheduled-audit-lifecycle-triggers-audit-task`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iot-awsiotservice/src/ops/delete_scheduled_audit.rs`. Replaying the corpus, the emulator refused the `DeleteScheduledAudit` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteScheduledAudit operation: operation DeleteScheduledAudit is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the iot emulator
Call DeleteScheduledAudit and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
