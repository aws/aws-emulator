Issue-Source: trace-sweep-divergence
Issue-Trace: chime-sdk-voice-voice-tone-analysis-request-validation
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chime-sdk-voice-voice-tone-analysis-request-validation
Issue-Shape-Id: com.amazonaws.chimesdkvoice#ChimeSDKTelephonyService
Issue-Key: chime-sdk-voice|StartVoiceToneAnalysisTask|4xx->4xx|$.message
Issue-Key: chime-sdk-voice|StopVoiceToneAnalysisTask|4xx->4xx|$.message
Issue-Commit: d175d2ebd7b20f74cd38c4d9513abc55db3772cc

# Validation refusals echo the framework's elided value instead of the submitted value

> _Note: this issue was identified against the code base at commit `d175d2ebd7b20f74cd38c4d9513abc55db3772cc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartVoiceToneAnalysisTask

## Description
Two causes, one root: this service's `override_refusal` restates the framework's boundary violations without restoring the value the caller submitted, so any value longer than the framework's elision cap is echoed as a byte-bounded prefix plus the marker `... [<n> bytes]` — wire text AWS never emits. It fires for every member whose `@length` maximum (256 here) is at or above that cap, which is why both over-long-string refusals of this service diverge in the same way.

Message anatomy for this error class: the FIXED text is `<n> validation error(s) detected: Value '<value>' at '<memberPath>' failed to satisfy constraint: Member must have length less than or equal to <max>`. The VARIABLE parts are the violation count, the value exactly as the request submitted it, the member path as the API spells it, and the declared maximum from the model. Only the elided echo diverges; the count, the path, the clause and the HTTP status all match.

Cause 1 of 2: StartVoiceToneAnalysisTask refusing an over-long `TransactionId`.
AWS: Smithy: `StartVoiceToneAnalysisTask.TransactionId` is `@required`, targets `NonEmptyString256` with `@length(min: 1, max: 256)` and `@pattern(\\S)`; the operation declares `BadRequestException` (HTTP 400). A 257-character value is refused with HTTP 400 and the message `1 validation error detected: Value '<the whole submitted value>' at 'transactionId' failed to satisfy constraint: Member must have length less than or equal to 256` — AWS echoes the value in full, which is the point of the echo: it is the only place the server reports the offending input back.
Emulator: answers HTTP 400 with the correct clause but a TRUNCATED echo — the value is cut at the framework's cap and the marker `... [257 bytes]` is appended in place of the remaining bytes, so the quoted value is not the one the caller sent.
Why: a client (or an SDK test) that compares the echoed value against what it sent sees a value it never submitted, and a message carrying `... [257 bytes]` is text no AWS response contains, so any assertion on the refusal message fails against the emulator while passing against AWS.
Fix: this service's refusal seam — `override_refusal` in src/service.rs must take its violations through the shared echo-restoring step (`service_common::validation`'s `with_submitted_values` / `refusing_violations`, whose module header documents exactly this gap) before handing them to the envelope renderer, instead of passing the raw `Refusal::Invalid` through `in_sorted_enum_order` only.
Traces: chime-sdk-voice-voice-tone-analysis-request-validation#task#6.

Cause 2 of 2: StopVoiceToneAnalysisTask refusing an over-long `VoiceToneAnalysisTaskId`.
AWS: Smithy: `StopVoiceToneAnalysisTask.VoiceToneAnalysisTaskId` is `@required`, `@httpLabel`, targets `NonEmptyString256` with `@length(min: 1, max: 256)`; the operation declares `BadRequestException` (HTTP 400). A 257-character label is refused with HTTP 400 and `1 validation error detected: Value '<the whole submitted value>' at 'voiceToneAnalysisTaskId' failed to satisfy constraint: Member must have length less than or equal to 256`.
Emulator: HTTP 400 with the same clause but the value elided to the cap plus `... [257 bytes]`. Same seam as Cause 1, reached from a URI-bound member rather than a body member.
Why: as Cause 1 — the refusal no longer reports back the id the caller asked about, so a client cannot tell which of several ids it refused from the message alone.
Fix: the same refusal seam; once the echo is restored from the built input, a URI label is covered with the body members.
Traces: chime-sdk-voice-voice-tone-analysis-request-validation#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout. Status, error class and clause text match; only the quoted value diverges.

Cause 1 — fixture chime-sdk-voice-voice-tone-analysis-request-validation, step task#6, StartVoiceToneAnalysisTask: expected 400, actual 400. `$.message` value_mismatch —
  expected: "1 validation error detected: Value '<257 't' characters, in full>' at 'transactionId' failed to satisfy constraint: Member must have length less than or equal to 256"
  actual:   "1 validation error detected: Value '<'t' characters truncated at the elision cap>... [257 bytes]' at 'transactionId' failed to satisfy constraint: Member must have length less than or equal to 256"

Cause 2 — same fixture, step task#8, StopVoiceToneAnalysisTask: expected 400, actual 400. `$.message` value_mismatch —
  expected: "1 validation error detected: Value '<257 'k' characters, in full>' at 'voiceToneAnalysisTaskId' failed to satisfy constraint: Member must have length less than or equal to 256"
  actual:   the same sentence with the value truncated and `... [257 bytes]` appended.

The fixed text of the class is identical in both; the 257-character strings themselves are the requests' own values and are not part of the requirement.

Model evidence: smithy_query(query="operation", operation="StartVoiceToneAnalysisTask,StopVoiceToneAnalysisTask") — `TransactionId` and `VoiceToneAnalysisTaskId` both `@required` on `NonEmptyString256`, length 1..256; both operations declare `BadRequestException` at HTTP 400.
Code evidence: services/service-com-amazonaws-chimesdkvoice-chimesdktelephonyservice/src/service.rs:229-240 — `override_refusal` maps the violations through `in_sorted_enum_order` and calls `aws_validation_refusal_with` directly, with no echo restoration. services/common/src/validation.rs:11-27 documents that the framework's walk stores an elided rendering ("a byte-bounded prefix followed by `... [<n> bytes]`", "wire text AWS never emits") and that `with_submitted_values` / `refusing_violations` is the step that closes it.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 corpus.
2. Cause 1: replay fixture chime-sdk-voice-voice-tone-analysis-request-validation step task#6 — POST /voice-connectors/{VoiceConnectorId}/voice-tone-analysis-tasks with a `TransactionId` of 257 characters and a valid `LanguageCode`. The 400 body's `message` quotes the value truncated and suffixed `... [257 bytes]` instead of the whole submitted value.
3. Cause 2: replay the same fixture's step task#8 — POST /voice-connectors/{VoiceConnectorId}/voice-tone-analysis-tasks/{257-character id}?operation=stop. The 400 body's `message` quotes the id the same elided way.
4. In both cases compare the quoted value against the value sent: they must be byte-identical.
