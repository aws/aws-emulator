Issue-Source: trace-sweep-divergence
Issue-Trace: chime-sdk-media-pipelines-delete-media-pipeline-unknown-and-malformed-id
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chime-sdk-media-pipelines-delete-media-pipeline-unknown-and-malformed-id chime-sdk-media-pipelines-voice-analytics-task-input-validation-rejected
Issue-Shape-Id: com.amazonaws.chimesdkmediapipelines#ChimeSDKMediaPipelinesService
Issue-Key: chime-sdk-media-pipelines|DeleteMediaPipeline|4xx->4xx|$.message
Issue-Key: chime-sdk-media-pipelines|StartSpeakerSearchTask|4xx->4xx|$.message
Issue-Key: chime-sdk-media-pipelines|StopSpeakerSearchTask|4xx->4xx|$.message
Issue-Commit: 736b28b40b20576fe554bad4abfa7fe4ba2cddb1

# chime-sdk-media-pipelines: pattern-constraint validation messages quote a re-anchored regex and drop the offending value

> _Note: this issue was identified against the code base at commit `736b28b40b20576fe554bad4abfa7fe4ba2cddb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteMediaPipeline

## Description
The emulator's `@pattern` constraint-violation message for Chime SDK Media Pipelines is built by rendering the emulator's own compiled, normalized regex instead of the pattern string the service reports, and it suppresses the offending value for members marked `@sensitive`. HTTP status (400) and the error shape are already correct in all three cases; only the `message` text diverges. 3 causes, all in the same shared validation/message-rendering path.

Shared fault: the message template is `1 validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>`. The fixed text of that template is correct; the emulator gets the `<pattern>` substitution wrong (it emits a re-anchored form of the regex: a leading `^` is always present and a trailing `$` is always stripped) and, for one member, gets the `<value>` substitution wrong (it is omitted entirely). `<value>`, `<memberName>` and `<pattern>` are variable parts taken from the request and from the member's model constraint; the surrounding sentence is fixed text.

Cause 1 of 3: DeleteMediaPipeline, `mediaPipelineId` (GuidString).
AWS: Smithy: DeleteMediaPipeline.MediaPipelineId -> GuidString, @length min 36/max 36, @pattern "^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}$"; modeled error BadRequestException, httpError 400. The recorded response is HTTP 400 with message "1 validation error detected: Value 'sim2real-pipeline-id-that-is-no-guid' at 'mediaPipelineId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}" — i.e. for this member the service reports the pattern with BOTH anchors removed.
Emulator: returns HTTP 400 but with "... regular expression pattern: ^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}" — a leading `^` that must not be there (the trailing `$` is correctly absent, but by accident of the same stripping rule).
Why: a client that asserts on, logs, or surfaces the constraint text to an end user sees a pattern string the real service never emits, and any test or client-side validator that re-derives the accepted id format from the reported pattern derives a different (anchored) expression than AWS reports.
Fix: the shared constraint-violation message rendering used by this service's request validation — it must substitute the member's reported pattern text verbatim rather than the emulator's compiled/normalized regex.
Traces: chime-sdk-media-pipelines-delete-media-pipeline-unknown-and-malformed-id#task#2.

Cause 2 of 3: StartSpeakerSearchTask, `voiceProfileDomainArn` (Arn).
AWS: Smithy: StartSpeakerSearchTask.VoiceProfileDomainArn -> Arn, required, @length min 1/max 1024, @pattern "^arn[\\/\\:\\-\\_\\.a-zA-Z0-9]+$", @sensitive; modeled error BadRequestException, httpError 400. The recorded response is HTTP 400 with message "1 validation error detected: Value 'arn:voice profile domain' at 'voiceProfileDomainArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn[\\/\\:\\-\\_\\.a-zA-Z0-9]+$" — so here the service reports the pattern WITH both anchors, and it DOES include the rejected value even though the member is @sensitive.
Emulator: returns HTTP 400 with "1 validation error detected: Value at 'voiceProfileDomainArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn[\\/\\:\\-\\_\\.a-zA-Z0-9]+" — two substitutions wrong: the `Value '<value>'` segment collapses to a bare `Value` (the rejected ARN string is dropped), and the trailing `$` of the reported pattern is stripped. Hypothesis: the value is suppressed because the member carries @sensitive, but the recorded service response shows the value is echoed for this constraint violation.
Why: the client loses the only indication of WHICH value was rejected, so a caller validating a batch of ARNs cannot tell which input failed; and the sentence "Value at 'x'" is not AWS's wording for this error class at all, so message-matching clients and human operators both get a different error text.
Fix: same constraint-violation message rendering plus the sensitivity redaction rule applied to it — the value must be interpolated for this constraint message, and the reported pattern must be the member's pattern text unmodified.
Traces: chime-sdk-media-pipelines-voice-analytics-task-input-validation-rejected#task#2.

Cause 3 of 3: StopSpeakerSearchTask, `speakerSearchTaskId` (GuidString).
AWS: Smithy: StopSpeakerSearchTask.SpeakerSearchTaskId -> GuidString, httpLabel, required, @length min 36/max 36, @pattern "^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}$"; modeled error BadRequestException, httpError 400. The recorded response is HTTP 400 with message "1 validation error detected: Value 'zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz' at 'speakerSearchTaskId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}" — again the unanchored form for a GuidString member.
Emulator: returns HTTP 400 with the same message except the pattern is reported as "^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}" (spurious leading `^`).
Why: same as cause 1 — the constraint text is a requirement of the error response, and a client that reads it gets a pattern the service does not report. Because this is the task-id path label, it is the message a caller sees for every malformed speaker-search task id.
Fix: same shared constraint-violation message rendering for this service's validation layer; fixing the pattern substitution closes causes 1 and 3 together, and the value/sensitivity substitution closes cause 2.
Traces: chime-sdk-media-pipelines-voice-analytics-task-input-validation-rejected#task#3.

Note: no existing open issue covers this; the only other open chime-sdk-media-pipelines-tagged issue is the CloudFormation "AWS::Chime::MediaInsightsPipelineConfiguration is not a registered resource type" issue, which is a different service and defect.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, 10 steps, 5 passed, 3 divergence clusters — all chime-sdk-media-pipelines, all status 400 -> 400 with key $.message mismatching).

Cause 1 — fixture chime-sdk-media-pipelines-delete-media-pipeline-unknown-and-malformed-id, step task#2, DeleteMediaPipeline:
  expected $.message: "1 validation error detected: Value 'sim2real-pipeline-id-that-is-no-guid' at 'mediaPipelineId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}"
  actual   $.message: "1 validation error detected: Value 'sim2real-pipeline-id-that-is-no-guid' at 'mediaPipelineId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}"
  delta: spurious leading '^' in the reported pattern.

Cause 2 — fixture chime-sdk-media-pipelines-voice-analytics-task-input-validation-rejected, step task#2, StartSpeakerSearchTask:
  expected $.message: "1 validation error detected: Value 'arn:voice profile domain' at 'voiceProfileDomainArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn[\\/\\:\\-\\_\\.a-zA-Z0-9]+$"
  actual   $.message: "1 validation error detected: Value at 'voiceProfileDomainArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn[\\/\\:\\-\\_\\.a-zA-Z0-9]+"
  delta: the "Value '<rejected value>'" segment is missing, and the trailing '$' of the reported pattern is stripped.

Cause 3 — fixture chime-sdk-media-pipelines-voice-analytics-task-input-validation-rejected, step task#3, StopSpeakerSearchTask:
  expected $.message: "1 validation error detected: Value 'zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz' at 'speakerSearchTaskId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}"
  actual   $.message: "... regular expression pattern: ^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}"
  delta: spurious leading '^' in the reported pattern.

Model grounding (services/service-com-amazonaws-chimesdkmediapipelines-chimesdkmediapipelinesservice/smithy-model/...json): GuidString @pattern "^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}$" @length 36..36; Arn @pattern "^arn[\\/\\:\\-\\_\\.a-zA-Z0-9]+$" @length 1..1024 @sensitive; all three operations model BadRequestException at httpError 400.

The quoted values ('sim2real-pipeline-id-that-is-no-guid', 'arn:voice profile domain', 'zzzzzzzz-...') are the request inputs of these particular recordings — they fill the template's <value> slot and are not themselves the requirement; the requirement is that the slot is filled from the request at all, and that the <pattern> slot carries the pattern text the service reports for that member.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: DELETE /sdk-media-pipelines/sim2real-pipeline-id-that-is-no-guid (any value that fails the GuidString pattern). Observe HTTP 400 and compare $.message against fixture chime-sdk-media-pipelines-delete-media-pipeline-unknown-and-malformed-id step task#2 — the reported regular-expression pattern carries a leading '^' the service does not report.
3. Cause 2: POST /media-insights-pipelines/{Identifier}/speaker-search-tasks?operation=start with VoiceProfileDomainArn set to a string that fails the Arn pattern (e.g. one containing spaces). Observe HTTP 400 and compare $.message against fixture chime-sdk-media-pipelines-voice-analytics-task-input-validation-rejected step task#2 — the rejected value is omitted after "Value" and the pattern's trailing '$' is stripped.
4. Cause 3: POST /media-insights-pipelines/{Identifier}/speaker-search-tasks/zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz?operation=stop. Observe HTTP 400 and compare $.message against the same fixture's step task#3 — same spurious leading '^' as cause 1.
5. Re-replay all three fixture steps (the representative plus both trace_ids bindings) to confirm every cause is closed.
