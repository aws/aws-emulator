Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pinpoint-sms-voice.delete-configuration-set-event-destination-nonexistent-denied
Issue-Shape-Id: com.amazonaws.pinpointsmsvoice#PinpointSMSVoice
Issue-Key: pinpoint-sms-voice|DeleteConfigurationSetEventDestination|NotImplemented
Issue-Commit: 7c3ecfa1a25692ce41ad054c5d4a0fbbc15f7c4b

# Implement pinpoint-sms-voice DeleteConfigurationSetEventDestination: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `7c3ecfa1a25692ce41ad054c5d4a0fbbc15f7c4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteConfigurationSetEventDestination

## Description
`DeleteConfigurationSetEventDestination` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-pinpointsmsvoice-pinpointsmsvoice/src/ops/delete_configuration_set_event_destination.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteConfigurationSetEventDestination` call, so each one unblocks only when `DeleteConfigurationSetEventDestination` works: `pinpoint-sms-voice.delete-configuration-set-event-destination-nonexistent-denied`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-pinpointsmsvoice-pinpointsmsvoice/src/ops/delete_configuration_set_event_destination.rs`. Replaying the corpus, the emulator refused the `DeleteConfigurationSetEventDestination` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_pinpoint_sms_voice_delete_configuration_set_event_destination_nonexistent_denied.py", line 34, in run
    assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected AccessDeniedException, got 'NotImplemented'

## Steps to Reproduce
Boot the pinpoint-sms-voice emulator
Call DeleteConfigurationSetEventDestination and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
