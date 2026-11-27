Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: fis__kinesis.expired-iterator-action-duration-outside-one-to-720-minutes-rejected fis__kinesis.expired-iterator-fault-refuses-get-records-on-the-targeted-stream-only
Issue-Key: fis__kinesis.expired-iterator-action-duration-outside-one-to-720-minutes-rejected|_error#20|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: fis__kinesis.expired-iterator-fault-refuses-get-records-on-the-targeted-stream-only|run#14|AssertionError|raise AssertionError(f"GetRecords on the targeted stream was never refused; experiment is {status!r}")
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# fis__kinesis: 2 net-new causes — CreateExperimentTemplate accepts an out-of-range 'duration' for aws:kinesis:stream-expired-iterator-exception, and a running expired-iterator experiment never injects ExpiredIteratorException into GetRecords

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExperimentTemplate

## Description
Two independent defects in the FIS Kinesis fault-injection surface. Both are grounded in replayed behavioral checks; neither is covered by an open issue (the only open FIS issue, 20261125T030729Z-fis-listactions-returns-paraphrased-built-in-action-descriptions.md, is about ListActions catalog text and does not cover either cause).

Cause 1 of 2: CreateExperimentTemplate does not validate the 'duration' action parameter of aws:kinesis:stream-expired-iterator-exception against its 1-to-720-minute window.
AWS: FIS validates each action's parameters against the action definition in the FIS actions reference (https://docs.aws.amazon.com/fis/latest/userguide/fis-actions-reference.html). For aws:kinesis:stream-expired-iterator-exception the 'duration' parameter is an ISO-8601 duration that must be within 1 to 720 minutes inclusive. Smithy: CreateExperimentTemplate.actions targets CreateExperimentTemplateActionInputMap whose parameters member is a plain string→string map, so the model itself states no range — the range is documented behavior only. The modeled refusal is ValidationException (errorKind client, httpError 400). The recorded refusal is HTTP 400, Code 'ValidationException', Message "'duration' must be within a range of 1 to 720 minutes." — that whole sentence is fixed text, with the quoted parameter name ('duration') taken from the offending action parameter; no request value, id or limit is interpolated. A refused CreateExperimentTemplate must also store nothing: the template must not appear in ListExperimentTemplates.
Emulator: PT59S (one second under the floor) and PT12H1M (one minute over the ceiling) are both accepted. The call returns HTTP 200 with a fully-formed experimentTemplate (an id/arn is minted and the template is persisted and listed), with the out-of-range duration echoed verbatim in actions.expire.parameters.duration. The in-range values PT1M and PT12H are accepted correctly, so only the boundary rejection is missing.
Why: a client that relies on FIS refusing a malformed action parameter gets a template that it believes is valid and that AWS would never have created. Templates built by IaC or by a validation-driven UI pass against the emulator and then fail at CreateExperimentTemplate against real FIS; code paths that assert on the ValidationException (error handling, form validation, negative tests) are never exercised; and the emulator leaks persisted garbage templates that real FIS never stores.
Fix: the CreateExperimentTemplate handler's action-parameter validation in the FIS service crate — the per-actionId parameter schema (the duration/percentage parameter definitions for the Kinesis actions) must be applied before any state mutation, so that an out-of-range duration is refused with ValidationException and no template is created. UpdateExperimentTemplate needs the same check for the same parameter.
Traces: fis__kinesis.expired-iterator-action-duration-outside-one-to-720-minutes-rejected.

Cause 2 of 2: a running aws:kinesis:stream-expired-iterator-exception experiment injects no fault — Kinesis GetRecords on the targeted stream is never refused, and the experiment simply completes.
AWS: while the expired-iterator fault is active, GetRecords against a targeted Kinesis data stream is refused for the configured duration and percentage (https://docs.aws.amazon.com/streams/latest/dev/kinesis-fis-expired-iterator.html). The refusal is on GetRecords (GetShardIterator still succeeds, so a fresh iterator can be obtained), Smithy kinesis: ExpiredIteratorException, errorKind client, httpError 400, doc "The provided iterator exceeds the maximum age allowed."; the recorded refusal is HTTP 400, Code 'ExpiredIteratorException', Message "Iterator expired." — fixed text, no interpolated values. The fault is scoped to the ARNs resolved by the experiment target: a second, untargeted stream keeps serving records throughout. After the experiment reaches 'completed', GetRecords on the targeted stream serves its records again.
Emulator: the experiment is accepted and started and runs through to status 'completed', but GetRecords on the targeted stream is never refused at any point during the experiment window — the check polls until the experiment is terminal and fails with "GetRecords on the targeted stream was never refused; experiment is 'completed'". The experiment lifecycle is emulated while the data-plane action is a no-op.
Why: a client exercising fault injection gets a green experiment that injected nothing. Resilience tests that assert their reader handles ExpiredIteratorException (re-obtaining a shard iterator and retrying) silently pass without the error path ever running, which is the exact opposite of what the user asked FIS to do; and a successful 'completed' experiment is reported as evidence that the fault was delivered.
Fix: the FIS experiment-execution component that runs an experiment's actions must apply the Kinesis expired-iterator action to the resolved target ARNs for the action's duration, and the Kinesis GetRecords handler must consult that active-fault state and refuse matching reads with ExpiredIteratorException while it is live, scoped to the targeted stream ARNs only and cleared when the experiment reaches a terminal state.
Traces: fis__kinesis.expired-iterator-fault-refuses-get-records-on-the-targeted-stream-only.

## Evidence
Cause 1 — fis__kinesis.expired-iterator-action-duration-outside-one-to-720-minutes-rejected:
Expected (AWS): CreateExperimentTemplate with actions={'expire': {actionId: 'aws:kinesis:stream-expired-iterator-exception', parameters: {duration: 'PT59S', percentage: '100'}, ...}} is refused with HTTP 400, Error.Code 'ValidationException', Error.Message "'duration' must be within a range of 1 to 720 minutes." Same for duration 'PT12H1M'. Neither template is created or listed.
Actual (emulator): the call succeeded. Replay output:
  AssertionError: expected a refusal, but the call succeeded: {'id': 'EXT000000000003', 'arn': 'arn:aws:fis:...:experiment-template/EXT000000000003', 'description': 'sim2real duration PT59S', 'actions': {'expire': {'actionId': 'aws:kinesis:stream-expired-iterator-exception', 'parameters': {'duration': 'PT59S', 'percentage': '100'}, ...}}, ...}
(the id/arn/timestamps above are per-run values, not requirements; the requirement is the 400/ValidationException refusal and that nothing is stored).
In-range PT1M and PT12H were accepted and read back unchanged, as AWS does.

Cause 2 — fis__kinesis.expired-iterator-fault-refuses-get-records-on-the-targeted-stream-only:
Expected (AWS): after StartExperiment on a template holding aws:kinesis:stream-expired-iterator-exception (duration PT1M, percentage 100) targeting one stream, GetRecords on that stream is refused with HTTP 400 / ExpiredIteratorException / "Iterator expired.", while GetRecords on an untargeted stream still serves its record; after the experiment is 'completed', the targeted stream serves records again.
Actual (emulator): GetRecords on the targeted stream always succeeded; the poll loop ran until the experiment went terminal. Replay output:
  AssertionError: GetRecords on the targeted stream was never refused; experiment is 'completed'

## Steps to Reproduce
Cause 1:
1. CreateStream a Kinesis stream and create an IAM role trusted by fis.amazonaws.com.
2. fis.CreateExperimentTemplate with targets={'Streams': {resourceType: 'aws:kinesis:stream', resourceArns: [<stream arn>], selectionMode: 'ALL'}} and actions={'expire': {actionId: 'aws:kinesis:stream-expired-iterator-exception', parameters: {duration: 'PT59S', percentage: '100'}, targets: {KinesisStreams: 'Streams'}}}.
3. Observe HTTP 200 and a created template; AWS returns HTTP 400 ValidationException "'duration' must be within a range of 1 to 720 minutes." Repeat with duration 'PT12H1M' for the ceiling, and with 'PT1M'/'PT12H' to confirm the in-range cases still pass. Confirm with ListExperimentTemplates that only the in-range templates exist.

Cause 2:
1. Create two Kinesis streams, put one record into each, and read each back once.
2. Create an IAM role trusted by fis.amazonaws.com allowing kinesis:InjectApiError and kinesis:DescribeStreamSummary.
3. fis.CreateExperimentTemplate with the expired-iterator action (duration PT1M, percentage 100) targeting only the first stream, then fis.StartExperiment.
4. Poll GetShardIterator+GetRecords on the targeted stream while the experiment is non-terminal: every call succeeds. AWS refuses GetRecords with HTTP 400 / ExpiredIteratorException / "Iterator expired." while the fault is live, keeps the second stream serving, and lets the targeted stream recover once the experiment is 'completed'.
