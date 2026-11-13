Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.object-lock-event-hold-requires-mode-duration-and-floor-date-rejected
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutObject|NotImplemented
Issue-Commit: f2f8f36b8a9d5e45c8e23580863863f823670049

# s3: PutObject with x-amz-object-lock-event-hold is refused NotImplemented, so the whole Object Lock event-hold surface is unreachable

> _Note: this issue was identified against the code base at commit `f2f8f36b8a9d5e45c8e23580863863f823670049`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
REOPENS 20261029T211510Z-s3-browser-edge-surface-presigned-get-refused-403.md: its fix left the call refused. That resolved issue claimed the SSE-C and object-lock headers no longer answer NotImplemented, but PutObject carrying x-amz-object-lock-event-hold is still refused with a NotImplemented scaffold message, so none of the event-hold behavior below can be reached. This is 1 cause.

Cause 1 of 1: the Object Lock event-hold headers are a scaffold stub on PutObject.
AWS: PutObject accepts the event-hold headers and enforces their rules. Smithy (s3): PutObjectRequest members ObjectLockEventHold (x-amz-object-lock-event-hold, enum), ObjectLockEventHoldDurationDays (x-amz-object-lock-event-hold-duration-days, integer), ObjectLockEventHoldDurationYears (x-amz-object-lock-event-hold-duration-years, integer), alongside ObjectLockMode / ObjectLockRetainUntilDate / ObjectLockLegalHoldStatus; PutObject's modeled errors include InvalidRequest (400). A conforming write (mode GOVERNANCE + retain-until covering the hold + event hold ON + exactly one duration) succeeds and HeadObject/GetObject echo ObjectLockMode, ObjectLockEventHold, ObjectLockEventHoldDurationDays and ObjectLockRetainUntilDate; PutObjectRetention with EventHold OFF releases the hold while keeping the mode. The refusals are: 400 InvalidRequest 'x-amz-object-lock-mode is required when x-amz-object-lock-event-hold is supplied.' when a hold is supplied with no mode; 400 InvalidRequest 'x-amz-object-lock-event-hold:ON must be supplied with exactly one event hold duration (days or years).' when no (or more than one) duration accompanies ON; and 400 InvalidArgument 'The retain until date must be equal to or later than the current time plus the event hold duration.' with ArgumentName x-amz-object-lock-retain-until-date and ArgumentValue the supplied date rendered as %Y-%m-%dT%H:%M:%SZ when the retain-until date is under the hold floor. Each message is fixed text; only the ArgumentValue is taken from the request. The message texts are recorded AWS behavior.
Emulator: the first conforming PutObject is refused with HTTP error code NotImplemented and the message 'operation PutObject with x-amz-object-lock-event-hold is scaffolded but not implemented in this emulator', so nothing further — round-trip, release, or any of the three refusals — happens.
Why: a client cannot place an object under an event hold at all, and the error it gets is one AWS never returns for PutObject (NotImplemented is not among PutObject's modeled errors), so SDK error handling cannot classify it. Compliance flows that depend on event holds are unusable, and the validation rules that would protect a caller from a bad retain-until date never run.
Fix: the PutObject handler and its object-lock validation/state in the s3 service (accept the three event-hold headers, persist them on the version so HeadObject/GetObject and PutObjectRetention can read and release them, and answer the three modeled refusals above).
Traces: s3.object-lock-event-hold-requires-mode-duration-and-floor-date-rejected.

## Evidence
Cause 1: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutObject operation: operation PutObject with x-amz-object-lock-event-hold is scaffolded but not implemented in this emulator — raised on PutObject(ObjectLockMode='GOVERNANCE', ObjectLockRetainUntilDate=now+2d10m, ObjectLockEventHold='ON', ObjectLockEventHoldDurationDays=1) against an Object-Lock-enabled bucket.

## Steps to Reproduce
Cause 1: CreateBucket with ObjectLockEnabledForBucket=True; PutObject with ObjectLockMode='GOVERNANCE', ObjectLockRetainUntilDate=<now+2 days>, ObjectLockEventHold='ON', ObjectLockEventHoldDurationDays=1; then HeadObject/GetObject for the echoed hold fields, PutObjectRetention with EventHold='OFF' to release, and the three refusal cases (hold with no mode; hold ON with no duration; retain-until under the hold floor).
