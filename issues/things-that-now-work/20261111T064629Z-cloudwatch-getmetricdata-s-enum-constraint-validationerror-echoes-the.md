Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.metric-data-timestamp-ordering cloudwatch.mstrx-e2e-find-metric-streams-excluding-namespace-lifecycle
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.metric-data-timestamp-ordering|run#20|AssertionError|assert error == ("ValidationError", (
Issue-Key: cloudwatch.mstrx-e2e-find-metric-streams-excluding-namespace-lifecycle|_drain#6|Timeout|time.sleep(2)
Issue-Commit: 4315fe29349a8ccf086f6caac897ebb4dc04700d

# cloudwatch: GetMetricData's enum-constraint ValidationError echoes the offending ScanBy value, and a CFN two-MetricStream lifecycle never converges (2 causes)

> _Note: this issue was identified against the code base at commit `4315fe29349a8ccf086f6caac897ebb4dc04700d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricData

## Description
Two net-new causes in the cloudwatch surface. The first shares a root with the already-open issue 20261028T043126Z-cloudwatch-enum-constraint-validationerror-messages-are-rendered-from.md (which carries other checks' replay keys but not this one, so it does not cover this check); the second is a non-convergence with no existing issue.

Cause 1 of 2 — the GetMetricData enum-constraint ValidationError message inserts the rejected value, which AWS's message for this member omits.
AWS: Smithy: GetMetricData.ScanBy targets the ScanBy enum (values TimestampDescending, TimestampAscending); an out-of-enum value is refused with code ValidationError (HTTP 400, awsQuery envelope). Recorded behavior: the message is the fixed sentence "1 validation error detected: Value at 'scanBy' failed to satisfy constraint: Member must satisfy enum value set: [TimestampAscending, TimestampDescending]" — its fixed text is "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must satisfy enum value set: [", "]"; its only variable parts are the member path taken from the request (scanBy) and the admitted value set, which AWS renders in the order [TimestampAscending, TimestampDescending]. For this member AWS does NOT echo the submitted value.
Emulator: the refusal is correctly ValidationError but the message reads "1 validation error detected: Value 'NotARealScanByValue' at 'scanBy' failed to satisfy constraint: Member must satisfy enum value set: [TimestampAscending, TimestampDescending]" — it splices the request's rejected value in after "Value " (the literal NotARealScanByValue above is this run's request value, not part of the template).
Why: a client that matches on the error message (log assertions, parity tests, error-classification middleware keyed to AWS's exact sentence) sees a message AWS never serves, and one that varies with the caller's input where AWS's is fixed, so an equality or hash comparison against the real message always fails.
Fix: the GetMetricData request-validation/error-rendering component for enum members — the enum-constraint sentence for ScanBy must be emitted without the offending value interpolated, matching AWS's per-member wording rather than the generic value-echoing template.

Cause 2 of 2 — a CloudFormation stack carrying two AWS::CloudWatch::MetricStream resources never completes its lifecycle; the check is killed at its 60 s budget.
AWS: recorded behavior only. The recording deploys a stack with an include-filter stream and an exclude-filter stream over a Firehose, then reads GetMetricStream (ExcludeFilters landing in template order, FirehoseArn, State=running), walks ListMetricStreams + GetMetricStream, stops both streams (State=stopped), drains the bucket and deletes the stack; every step answers inside the recording's budget.
Emulator: the run never gets through the sequence — the harness terminates it with "Check timed out after 60.0s", so some call in the deploy/GetMetricStream/StopMetricStreams walk neither answers nor fails. No per-call status or error code was produced to quote.
Why: a client scripting metric-stream provisioning hangs instead of receiving an answer or a modelled refusal. A hang is worse than a wrong answer: SDK retries pile up and the caller has no error to branch on.
Fix: the CloudWatch metric-stream control path (CreateMetricStream/GetMetricStream/ListMetricStreams/StopMetricStreams) together with the CloudFormation AWS::CloudWatch::MetricStream resource provisioning — each call in the lifecycle must terminate with a response or a modelled error rather than blocking.

Traces: cloudwatch.metric-data-timestamp-ordering (Cause 1), cloudwatch.mstrx-e2e-find-metric-streams-excluding-namespace-lifecycle (Cause 2).

## Evidence
Cause 1 — check cloudwatch.metric-data-timestamp-ordering [mismatch]:
AssertionError: expected the ValidationError for invalid ScanBy, got ('ValidationError', "1 validation error detected: Value 'NotARealScanByValue' at 'scanBy' failed to satisfy constraint: Member must satisfy enum value set: [TimestampAscending, TimestampDescending]")
Expected (code, message): ('ValidationError', "1 validation error detected: Value at 'scanBy' failed to satisfy constraint: Member must satisfy enum value set: [TimestampAscending, TimestampDescending]")
Actual: same code, message with "Value 'NotARealScanByValue' at 'scanBy'" — the request's value spliced into fixed text.

Cause 2 — check cloudwatch.mstrx-e2e-find-metric-streams-excluding-namespace-lifecycle [mismatch]:
Error: Check timed out after 60.0s — the deploy/GetMetricStream/ListMetricStreams/StopMetricStreams lifecycle over two AWS::CloudWatch::MetricStream resources never completed; no status or error code was returned to compare.

## Steps to Reproduce
Cause 1:
1. cloudwatch GetMetricData with a valid MetricDataQueries/StartTime/EndTime and ScanBy="NotARealScanByValue".
2. AWS answers HTTP 400 ValidationError with "1 validation error detected: Value at 'scanBy' failed to satisfy constraint: Member must satisfy enum value set: [TimestampAscending, TimestampDescending]".
3. The emulator answers the same code but inserts the rejected value after "Value ".

Cause 2:
1. Deploy a CloudFormation stack with two AWS::CloudWatch::MetricStream resources (one IncludeFilters, one ExcludeFilters) targeting a Firehose delivery stream into an S3 bucket.
2. GetMetricStream on each, then ListMetricStreams + GetMetricStream to select the exclude stream, then StopMetricStreams on both, then delete the stack.
3. The sequence never completes; the run is killed at the 60 s budget with no answer.

## Resolution

Cause 1 — FIXED. The enum-constraint refusal states the member path and the admitted value
set and withholds the submitted value for the enumerated type the scan direction is
(`services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/service.rs`,
`scalar_enum_refusal` / `VALUE_WITHHELD_ENUM_TYPES`, rendered through
`services/common/src/validation_message.rs`); the check this issue is keyed on replays green,
and the wording is pinned over a table of enumerated types, operations and depths by
`services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/tests/all/enum_refusal_value_half.rs`.

Cause 2 — the emulator-side defect behind it is FIXED: a running metric stream now exports
every metric update the account's metric store records whose namespace its filters admit,
whatever wrote it, instead of only the updates arriving through `PutMetricData` — the service's
own API call count (`src/state/usage.rs`) is streamed like any other metric of the account, so
an estate nothing publishes into no longer delivers nothing at all. Every call of the recorded
lifecycle (deploy, the two reads, the paginated ListMetricStreams walk, StopMetricStreams,
emptying the destination, deleting the stack) is driven end to end by
`core/router/tests/all/cloudformation_metric_stream_estate_is_drainable.rs` and answers.
STILL OPEN: the recorded check itself continues to exhaust its 60 s client-side budget inside
its own drain loop; no call of the lifecycle was found that neither answers nor fails, and the
residual gap is that budget against a destination whose first delivery is owed only once the
delivery stream's configured buffering interval has passed.
