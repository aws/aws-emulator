Issue-Source: trace-sweep-divergence
Issue-Trace: cwq-query-protocol-alarm-round-trip-and-errors
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cwq-query-protocol-alarm-round-trip-and-errors
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: monitoring|DescribeAlarms|2xx->2xx|$.DescribeAlarmsResponse.DescribeAlarmsResult.MetricAlarms.member.Threshold
Issue-Key: monitoring|PutMetricAlarm|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Commit: 23d7c3ca3e68cb70ca93342274fc19d1394baa26

# CloudWatch awsQuery XML serializer renders doubles and error codes off-contract

> _Note: this issue was identified against the code base at commit `23d7c3ca3e68cb70ca93342274fc19d1394baa26`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
2 causes, one shared fault: the CloudWatch awsQuery (form-encoded in, text/xml out) wire is now served, but its XML serializers emit values that do not match the service's query-protocol contract — modeled `double` members lose their decimal form, and the error envelope carries the JSON/Coral exception name instead of the query error code. The open issue 20261029T203422Z-monitoring-cloudwatch-awsquery-form-encoded-protocol-is-not-served.md covers the earlier state in which the query wire was not served at all (400 + application/x-amz-json-1.0); these two causes are a distinct, net-new defect in the serializers that now answer those requests, and neither is listed as owned by it.

SHARED AWS CONTRACT — Smithy: com.amazonaws.cloudwatch#GraniteServiceVersion20100801, protocols [awsJson1_0, awsQuery, rpcv2Cbor], protocol modifier awsQueryCompatible, xmlNamespace uri 'http://monitoring.amazonaws.com/doc/2010-08-01/'. On the awsQuery wire a success is '<OperationNameResponse><OperationNameResult>…</OperationNameResult><ResponseMetadata><RequestId>…</RequestId></ResponseMetadata></…>' and a failure is '<ErrorResponse><Error><Type>Sender</Type><Code>…</Code><Message>…</Message></Error><RequestId>…</RequestId></ErrorResponse>', where Code is the QUERY error code (the same token the awsQueryCompatible JSON wire puts in the x-amzn-query-error header, e.g. 'ValidationError;Sender'), not the Coral/JSON shape name.

SHARED EMULATOR BEHAVIOUR: the request is parsed and the operation runs (status matches), but individual serialized values diverge: a double is written in integral form and the error Code is written as the exception shape name.

SHARED WHY: a query-protocol client reads these fields literally. A Code it does not recognise is classified as an unknown/unmodelled error, so validation faults stop being retriable-vs-fatal-classifiable and error-code branching (asserting 'ValidationError') never matches; a threshold that comes back without its fractional form round-trips differently than AWS and breaks exact-echo comparisons and strict decimal parsers.

SHARED FIX: the CloudWatch crate's awsQuery/XML response serializer and its ErrorResponse serializer (value formatting for double-typed members; query-error-code lookup for the error envelope). The operation handlers and their JSON serializers are already correct.

Cause 1 of 2: DescribeAlarms over awsQuery serializes MetricAlarm.Threshold without its decimal form. AWS: Smithy DescribeAlarms output MetricAlarms -> MetricAlarm.Threshold targets `Threshold`, type double; on the query wire the XML text of a double member keeps a decimal point (the value the client sent as a double is echoed in double form, e.g. a threshold of five is rendered '5.0'). Emulator: the same element is rendered in integral form ('5'), i.e. the double is formatted as if it were an integer whenever its fractional part is zero. Why: a client comparing the alarm it just created against the alarm it reads back sees a different textual value for the same threshold, and strict XML-to-decimal binding sees an integer where the model declares a double. Fix: double formatting in the awsQuery XML response serializer for the CloudWatch crate (applies to every double-typed member, not just Threshold). Traces: cwq-query-protocol-alarm-round-trip-and-errors#task#2 (also #task#4).

Cause 2 of 2: PutMetricAlarm over awsQuery returns the Coral exception name in ErrorResponse.Error.Code. AWS: a request-validation refusal on the query wire is 400 with '<ErrorResponse><Error><Type>Sender</Type><Code>ValidationError</Code><Message>…</Message></Error>…' — Code is the fixed query token 'ValidationError' and Type the fixed token 'Sender'; the Message is the validation text whose fixed part is "1 validation error detected: Value '<value>' at '<memberPath>' failed to satisfy constraint: <constraint text>" with the offending value and the member path taken from the request. The awsQueryCompatible modifier on this service makes the same token the one carried in x-amzn-query-error ('ValidationError;Sender') on the JSON wire, which the corpus's JSON clusters for this service also expect. Emulator: Code is 'ValidationException' — the JSON/Coral shape name — on an otherwise correct 400 ErrorResponse. Why: a query-protocol client branching on Error.Code 'ValidationError' (SDK error shape matching, CLI error handling, operator assertions) never matches and reports an unknown service error instead of a client validation fault. Fix: the ErrorResponse serializer / error-to-query-code mapping in the CloudWatch crate's awsQuery error path — map each error to its awsQueryError code (and the service-common validation error to 'ValidationError'), not to the exception shape name. Traces: cwq-query-protocol-alarm-round-trip-and-errors#task#5.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (score 83.4%, 44 divergence clusters; these two are the only clusters not already owned by an open issue or blocked by a stub).

Cause 1 — fixture cwq-query-protocol-alarm-round-trip-and-errors, step task#2, DescribeAlarms, status 200 expected and 200 received:
  $.DescribeAlarmsResponse.DescribeAlarmsResult.MetricAlarms.member.Threshold: value_mismatch (expected '5.0', got '5')
  cluster members: task#2, task#4 (affected_count 2)

Cause 2 — fixture cwq-query-protocol-alarm-round-trip-and-errors, step task#5, PutMetricAlarm, status 400 expected and 400 received:
  $.ErrorResponse.Error.Code: value_mismatch (expected 'ValidationError', got 'ValidationException')
  cluster members: task#5 (affected_count 1)

Model citations: Smithy PutMetricAlarm.Threshold / MetricAlarm.Threshold -> `Threshold` (double); service protocols include awsQuery with protocol_modifiers [awsQueryCompatible]; the corpus's awsQueryCompatible JSON responses for this service expect header x-amzn-query-error 'ValidationError;Sender' for the same validation class, which is the token the XML Error.Code must carry.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout; replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: POST a SigV4-signed application/x-www-form-urlencoded body 'Action=PutMetricAlarm&Version=2010-08-01&AlarmName=<name>&Namespace=<ns>&MetricName=<metric>&ComparisonOperator=GreaterThanThreshold&EvaluationPeriods=1&Period=60&Statistic=Average&Threshold=5' to the monitoring endpoint (fixture cwq-query-protocol-alarm-round-trip-and-errors step task#1), then POST 'Action=DescribeAlarms&Version=2010-08-01&AlarmNames.member.1=<name>' (step task#2). Read the Threshold element of the returned MetricAlarms.member: the emulator writes '5' where AWS writes '5.0'.
3. Cause 2: POST 'Action=PutMetricAlarm&Version=2010-08-01&...&ComparisonOperator=<value outside the enum>' (step task#5). The response is 400 text/xml, but <ErrorResponse><Error><Code> is 'ValidationException' instead of 'ValidationError'.
4. Both steps are replayed by replay_wire_corpus as the only two unowned monitoring clusters.
