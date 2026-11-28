Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatchomni.stop-session-rejects-an-unknown-id-but-repeats-on-its-own-session
Issue-Shape-Id: com.amazonaws.cloudwatchomni#CloudWatchOmniFrontend
Issue-Key: cloudwatchomni.stop-session-rejects-an-unknown-id-but-repeats-on-its-own-session|run#8|AssertionError|assert got == ("ResourceNotFoundException", "The specified resource does not exist."), (
Issue-Commit: d7b90cb0555dc69b04a63d1b15d7fc089e6fcfe3

# Implement cloudwatchomni StopTelemetryQuerySession: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d7b90cb0555dc69b04a63d1b15d7fc089e6fcfe3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopTelemetryQuerySession

## Description
`StopTelemetryQuerySession` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/src/ops/stop_telemetry_query_session.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `StopTelemetryQuerySession` call, so each one unblocks only when `StopTelemetryQuerySession` works: `cloudwatchomni.stop-session-rejects-an-unknown-id-but-repeats-on-its-own-session`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/src/ops/stop_telemetry_query_session.rs`. Replaying the corpus, the emulator refused the `StopTelemetryQuerySession` call: File "run.py", line 461, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_cloudwatchomni_stop_session_rejects_an_unknown_id_but_repeats_on_its_own_session.py", line 88, in run
    assert got == ("ResourceNotFoundException", "The specified resource does not exist."), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: stopping a session id never opened answered ('NotImplemented', 'operation StopTelemetryQuerySession is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the cloudwatchomni emulator
Call StopTelemetryQuerySession and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
