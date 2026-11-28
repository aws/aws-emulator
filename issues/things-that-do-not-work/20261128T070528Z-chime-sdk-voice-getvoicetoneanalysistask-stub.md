Issue-Source: trace-sweep-divergence
Issue-Trace: chime-sdk-voice-voice-tone-analysis-request-validation
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chime-sdk-voice-voice-tone-analysis-request-validation
Issue-Shape-Id: com.amazonaws.chimesdkvoice#ChimeSDKTelephonyService
Issue-Key: chime-sdk-voice|stub:GetVoiceToneAnalysisTask
Issue-Commit: d175d2ebd7b20f74cd38c4d9513abc55db3772cc

# Implement chime-sdk-voice GetVoiceToneAnalysisTask: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d175d2ebd7b20f74cd38c4d9513abc55db3772cc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetVoiceToneAnalysisTask

## Description
`GetVoiceToneAnalysisTask` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-chimesdkvoice-chimesdktelephonyservice/src/ops/get_voice_tone_analysis_task.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetVoiceToneAnalysisTask` call, so each one unblocks only when `GetVoiceToneAnalysisTask` works: `chime-sdk-voice-voice-tone-analysis-request-validation#task#9`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-chimesdkvoice-chimesdktelephonyservice/src/ops/get_voice_tone_analysis_task.rs`. Replaying the corpus, the emulator refused the `GetVoiceToneAnalysisTask` call: status: expected 400, got 501
$.message: value_mismatch (expected "1 validation error detected: Value null at 'isCaller' failed to satisfy constraint: Member must not be null", got 'operation GET /voice-connectors/b71c2f7f381e345467615/voice-tone-analysis-tasks/sim2real-task-a76f6efd5ba4 is not implemented in this emulator')
$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'NotImplemented')

## Steps to Reproduce
Boot the chime-sdk-voice emulator
Call GetVoiceToneAnalysisTask and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
