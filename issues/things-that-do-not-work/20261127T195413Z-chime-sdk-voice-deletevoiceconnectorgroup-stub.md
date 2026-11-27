Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: chime-sdk-voice.voice-connector-group-with-members-delete-rejected
Issue-Shape-Id: com.amazonaws.chimesdkvoice#ChimeSDKTelephonyService
Issue-Key: chime-sdk-voice.voice-connector-group-with-members-delete-rejected|run#14|AssertionError|assert got == want, f"deleting a non-empty group answered {got!r}, expected {want!r}"
Issue-Commit: 736b28b40b20576fe554bad4abfa7fe4ba2cddb1

# Implement chime-sdk-voice DeleteVoiceConnectorGroup: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `736b28b40b20576fe554bad4abfa7fe4ba2cddb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteVoiceConnectorGroup

## Description
`DeleteVoiceConnectorGroup` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-chimesdkvoice-chimesdktelephonyservice/src/ops/delete_voice_connector_group.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteVoiceConnectorGroup` call, so each one unblocks only when `DeleteVoiceConnectorGroup` works: `chime-sdk-voice.voice-connector-group-with-members-delete-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-chimesdkvoice-chimesdktelephonyservice/src/ops/delete_voice_connector_group.rs`. Replaying the corpus, the emulator refused the `DeleteVoiceConnectorGroup` call: File "run.py", line 461, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_chime_sdk_voice_voice_connector_group_with_members_delete_rejected.py", line 117, in run
    assert got == want, f"deleting a non-empty group answered {got!r}, expected {want!r}"
           ^^^^^^^^^^^
AssertionError: deleting a non-empty group answered ('NotImplemented', 'operation DeleteVoiceConnectorGroup is scaffolded but not implemented in this emulator'), expected ('BadRequestException', "Cannot delete VoiceConnectorGroup '1618cab408c3ae9a54afa' with associated voice connectors.")

## Steps to Reproduce
Boot the chime-sdk-voice emulator
Call DeleteVoiceConnectorGroup and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
